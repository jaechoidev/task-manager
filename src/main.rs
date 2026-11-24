mod engine;
mod gh;
mod labels;
mod plan;
mod repo;
mod yaml;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use plan::model::PlanSet;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Parser)]
#[command(name = "factory", version, about = "Deterministic task manager: plans, issues and repo bootstrap")]
struct Cli {
    #[command(subcommand)]
    command: Top,
}

#[derive(Subcommand)]
enum Top {
    /// Validate plan files and turn them into GitHub issues
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    /// Create and check a project repository
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
    /// Take one issue through develop, verify and review, then open a pull request
    Run {
        /// Issue number
        issue: u64,
        /// A checkout of the project repository
        #[arg(long, default_value = ".")]
        repo_dir: PathBuf,
        /// Use this workflow file, not factory.yaml on the base branch
        #[arg(long, value_name = "FILE")]
        workflow: Option<PathBuf>,
        /// Stop after review, without pushing or opening a pull request
        #[arg(long)]
        no_pr: bool,
    },
    /// Show where each issue stands, from the state database
    Status,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Markdown,
    Json,
}

#[derive(Subcommand)]
enum PlanCommand {
    /// Check plan files. Exits 1 when any plan fails
    Validate {
        /// Plan files, or directories holding them
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Show detail only for plans changed since this commit. Every plan is still checked
        #[arg(long, value_name = "SHA")]
        changed_since: Option<String>,
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// Where existing Maestro flows live
        #[arg(long, default_value = ".maestro")]
        flows_dir: PathBuf,
        /// Write the report here and print the text form to the terminal
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Create issues, parent links and blocked-by links. Safe to rerun
    Create {
        /// Plan files, or directories holding them
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// owner/name. Defaults to GITHUB_REPOSITORY, then the current checkout
        #[arg(long)]
        repo: Option<String>,
        /// Only create issues for plans changed since this commit
        #[arg(long, value_name = "SHA")]
        changed_since: Option<String>,
        /// Show what would be created without writing anything
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = ".maestro")]
        flows_dir: PathBuf,
        /// Write a markdown report here, for a pull request comment. Empty when nothing was created or linked
        #[arg(long, value_name = "FILE")]
        report: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum RepoCommand {
    /// Create a private repo from a template and apply its settings. Safe to rerun
    Init {
        /// owner/name of the new repository
        repo: String,
        /// owner/name of the template repository
        #[arg(long)]
        template: Option<String>,
        /// Account to give write access. Repeat for more than one
        #[arg(long = "collaborator", value_name = "LOGIN")]
        collaborators: Vec<String>,
        #[arg(long)]
        description: Option<String>,
        /// Secret the workflows need. It is checked, never written
        #[arg(long = "secret", value_name = "NAME", default_value = "CLAUDE_CODE_OAUTH_TOKEN")]
        secrets: Vec<String>,
    },
    /// Read every setting back from GitHub. Exits 1 when one is wrong
    Preflight {
        repo: String,
        #[arg(long = "collaborator", value_name = "LOGIN")]
        collaborators: Vec<String>,
        #[arg(long = "secret", value_name = "NAME", default_value = "CLAUDE_CODE_OAUTH_TOKEN")]
        secrets: Vec<String>,
    },
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Plan paths changed since `sha`, or `None` when that can't be worked out.
/// `None` means "treat every plan as changed", which is always safe.
fn changed_plans(sha: &str, set: &PlanSet) -> Option<BTreeSet<String>> {
    if sha.is_empty() || sha.chars().all(|c| c == '0') {
        return None;
    }
    let root = PathBuf::from(git(&["rev-parse", "--show-toplevel"])?);
    let Some(diff) = git(&["diff", "--name-only", sha, "HEAD"]) else {
        eprintln!("warning: could not diff against {sha}; treating every plan as changed");
        return None;
    };
    let changed: BTreeSet<PathBuf> = diff.lines().map(|line| root.join(line)).collect();
    let absolute = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = absolute(&root);
    Some(
        set.plans
            .iter()
            .filter(|plan| {
                let path = absolute(&plan.path);
                changed.iter().any(|c| root.join(c.strip_prefix(&root).unwrap_or(c)) == path || absolute(c) == path)
            })
            .map(|plan| plan.path.display().to_string())
            .collect(),
    )
}

fn run() -> Result<bool> {
    match Cli::parse().command {
        Top::Plan { command: PlanCommand::Validate { paths, changed_since, format, flows_dir, out } } => {
            let set = plan::model::load(&paths)?;
            let report = plan::validate::validate(&set, &plan::validate::Options { flows_dir });
            let only = changed_since.as_deref().and_then(|sha| changed_plans(sha, &set));
            let rendered = match format {
                Format::Text => plan::render::text(&report, only.as_ref()),
                Format::Markdown => plan::render::markdown(&report, only.as_ref()),
                Format::Json => plan::render::json(&report),
            };
            match out {
                Some(file) => {
                    std::fs::write(&file, rendered).with_context(|| format!("writing {}", file.display()))?;
                    print!("{}", plan::render::text(&report, only.as_ref()));
                }
                None => print!("{rendered}"),
            }
            if let Some(only) = &only {
                println!("changed plans: {}", only.len());
            }
            Ok(report.ok)
        }

        Top::Plan { command: PlanCommand::Create { paths, repo, changed_since, dry_run, flows_dir, report } } => {
            let set = plan::model::load(&paths)?;
            let checked = plan::validate::validate(&set, &plan::validate::Options { flows_dir });
            if !checked.ok {
                print!("{}", plan::render::text(&checked, None));
                eprintln!("nothing was created: fix the {} error(s) above first", checked.error_count());
                return Ok(false);
            }
            let only = changed_since.as_deref().and_then(|sha| changed_plans(sha, &set));
            let selected: Vec<usize> = (0..set.plans.len())
                .filter(|&i| only.as_ref().is_none_or(|o| o.contains(&set.plans[i].path.display().to_string())))
                .collect();
            let outcome = if selected.is_empty() {
                plan::create::Outcome { dry_run, warnings: Vec::new(), plans: Vec::new() }
            } else {
                let github = plan::create::GhIssues { repo: gh::resolve_repo(repo.as_deref())? };
                println!("repository: {}", github.repo);
                plan::create::create(&set, &checked, &selected, &github, dry_run)?
            };
            print!("{}", plan::create::text(&outcome));
            if let Some(file) = report {
                // Left empty when nothing changed, so a rerun doesn't comment again.
                let changed = outcome.plans.iter().any(|p| p.links_added > 0 || p.parent.created || p.issues.iter().any(|i| i.created));
                let body = if changed { plan::create::markdown(&outcome) } else { String::new() };
                std::fs::write(&file, body).with_context(|| format!("writing {}", file.display()))?;
            }
            Ok(true)
        }

        Top::Repo { command: RepoCommand::Init { repo, template, collaborators, description, secrets } } => {
            let opts = repo::Options { repo, template, collaborators, description, secrets };
            let checks = repo::init(&opts)?;
            print!("{}", repo::report(&opts.repo, &checks));
            Ok(repo::passed(&checks))
        }

        Top::Run { issue, repo_dir, workflow, no_pr } => {
            engine::run(&engine::RunOptions { repo_dir, issue, workflow_file: workflow, no_pr })
        }

        Top::Status => {
            engine::status()?;
            Ok(true)
        }

        Top::Repo { command: RepoCommand::Preflight { repo, collaborators, secrets } } => {
            let opts = repo::Options { repo, template: None, collaborators, description: None, secrets };
            let checks = repo::preflight(&opts)?;
            print!("{}", repo::report(&opts.repo, &checks));
            Ok(repo::passed(&checks))
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}
