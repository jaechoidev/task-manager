//! The agent subscriptions this machine has, in `<home>/providers.yaml`.
//!
//! A provider is one logged-in command-line agent: which program runs it,
//! which models it offers by tier, and how to select its login. A role is an
//! ordered list of provider and tier, first available wins.
//!
//! The file holds no secret. A provider points at a login that already
//! exists, by an env file or a config folder the person set up themselves.
//! This module only reads and reports. Nothing here calls an agent. The
//! report does ask Z.ai how much of a plan is used, which spends nothing.

use super::agent::Runner;
use super::ledger::{self, Ledger};
use super::qualify::{self, Checked};
use super::workflow::Vendor;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A starter file, printed when there is none.
pub const EXAMPLE: &str = r#"# Agent subscriptions on this machine. No secrets in this file.
providers:
  chatgpt:
    cli: codex
    family: openai
    tiers:
      high: gpt-6.2-astra
      standard: gpt-6.2-sol
  claude:
    cli: claude
    family: anthropic
    tiers:
      high: opus
      standard: sonnet
      fast: haiku
  glm:
    cli: claude
    family: zhipu
    # Sets the endpoint and key for this login. Written by you, read by the shell.
    env_file: ~/.zai/env.sh
    quota: zai
    tiers:
      high: opus
      standard: sonnet
      fast: haiku
  antigravity:
    cli: agy
    # One provider can serve several model families, so a tier may name its own.
    tiers:
      high: { model: gemini-4-hydrogen-high, family: google }
      fast: { model: gemini-3.8-flash-medium, family: google }

# For each role, who to try, in order. A role with no list uses the
# workflow's vendor with that program's default model on this machine.
roles:
  acceptance:
    - { use: claude, tier: high }
    - { use: glm, tier: high }
  develop:
    - { use: chatgpt, tier: standard }
    - { use: glm, tier: high }
  review:
    - { use: claude, tier: high }
    - { use: glm, tier: high }
  summary:
    - { use: glm, tier: fast }
    - { use: claude, tier: fast }
"#;

/// The program that runs a provider. Each has its own flags, so the engine
/// has to know it by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cli {
    Codex,
    Claude,
    Agy,
}

impl Cli {
    pub fn program(self) -> &'static str {
        match self {
            Cli::Codex => "codex",
            Cli::Claude => "claude",
            Cli::Agy => "agy",
        }
    }
}

/// Where a provider's remaining quota can be read without spending any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quota {
    /// Z.ai's monitor endpoint, read with the key in the env file.
    Zai,
}

/// A tier is a model name, or a model name with its own family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Tier {
    Model(String),
    Detailed {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        family: Option<String>,
    },
}

impl Tier {
    pub fn model(&self) -> &str {
        match self {
            Tier::Model(model) | Tier::Detailed { model, .. } => model,
        }
    }

    fn own_family(&self) -> Option<&str> {
        match self {
            Tier::Model(_) => None,
            Tier::Detailed { family, .. } => family.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub cli: Cli,
    /// Who made the models, such as `openai`. Two steps that must be
    /// independent need different families. A tier may override it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// A shell file that selects this login, sourced before the program runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_file: Option<String>,
    /// A config folder that selects this login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<Quota>,
    #[serde(default)]
    pub tiers: BTreeMap<String, Tier>,
}

impl Provider {
    /// The family of a tier: its own, or the provider's.
    pub fn family_of(&self, tier: &str) -> Option<&str> {
        self.tiers.get(tier)?.own_family().or(self.family.as_deref())
    }

    /// How a call to one of its tiers is started.
    pub fn runner(&self, tier: &str) -> Option<Runner> {
        let model = self.tiers.get(tier)?.model().to_string();
        Some(Runner { cli: self.cli, model: Some(model), env_file: self.env_file.as_deref().map(expand), config_dir: self.config_dir.as_deref().map(expand) })
    }
}

/// One way to fill a role: a provider, the tier asked of it, and how to run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub provider: String,
    pub tier: String,
    /// Who made the model. Two steps that must be independent need different ones.
    pub family: String,
    pub runner: Runner,
    /// Whether it must have passed `factory providers check`. A workflow
    /// step's own vendor, used when no role list exists, does not.
    pub listed: bool,
    /// Where its remaining quota can be read, if anywhere.
    pub quota: Option<Quota>,
}

impl Candidate {
    /// What a workflow step names when this machine has no list for its role.
    pub fn of_step(vendor: Vendor, model: Option<&str>) -> Candidate {
        let family = match vendor {
            Vendor::Codex => "openai",
            Vendor::Claude => "anthropic",
        };
        Candidate { provider: vendor.name().into(), tier: "default".into(), family: family.into(), runner: Runner::of(vendor, model), listed: false, quota: None }
    }

    pub fn model(&self) -> &str {
        self.runner.model.as_deref().unwrap_or("default")
    }

    /// What a usage limit belongs to: a subscription and one of its models.
    pub fn limit_key(&self) -> String {
        format!("{}/{}", self.provider, self.model())
    }

    /// For a person: `glm high (glm-5.3, zhipu)`.
    pub fn label(&self) -> String {
        if self.listed {
            format!("{} {} ({}, {})", self.provider, self.tier, self.model(), self.family)
        } else {
            self.provider.clone()
        }
    }
}

/// Why a candidate was passed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// It has not passed `factory providers check` with the model it names.
    NotQualified,
    /// Its family did other work on this issue that this role must be
    /// independent of, or was ruled out for this attempt.
    Family,
    /// It is out of quota until this time, in seconds since 1970.
    Until(u64),
}

/// The first candidate in the order that may be used now, or why none may.
pub fn choose(order: &[Candidate], qualified: &dyn Fn(&Candidate) -> bool, not_families: &[String], out_until: &dyn Fn(&Candidate) -> Option<u64>) -> Result<Candidate, Vec<(Candidate, Skipped)>> {
    let mut skipped = Vec::new();
    for candidate in order {
        // What a wait can't fix is reported before what it can.
        let why = if candidate.listed && !qualified(candidate) {
            Skipped::NotQualified
        } else if not_families.contains(&candidate.family) {
            Skipped::Family
        } else if let Some(until) = out_until(candidate) {
            Skipped::Until(until)
        } else {
            return Ok(candidate.clone());
        };
        skipped.push((candidate.clone(), why));
    }
    Err(skipped)
}

/// When waiting would help: the earliest time a candidate that is only out
/// of quota comes back. None means no wait changes anything.
pub fn back_at(skipped: &[(Candidate, Skipped)]) -> Option<u64> {
    skipped.iter().filter_map(|(_, why)| if let Skipped::Until(until) = why { Some(*until) } else { None }).min()
}

/// Why nobody could be chosen, for a person.
pub fn explain(role: &str, skipped: &[(Candidate, Skipped)]) -> String {
    if skipped.is_empty() {
        return format!("nobody is listed for the role {role}");
    }
    let lines: Vec<String> = skipped
        .iter()
        .map(|(candidate, why)| {
            let reason = match why {
                Skipped::NotQualified => "has not passed `factory providers check`".to_string(),
                Skipped::Family => format!("its family, {}, must stay independent here", candidate.family),
                Skipped::Until(until) => format!("out of quota until {}", super::util::clock(*until)),
            };
            format!("{}: {reason}", candidate.label())
        })
        .collect();
    format!("nobody can take the role {role}. {}", lines.join(". "))
}

/// One entry in a role's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    #[serde(rename = "use")]
    pub provider: String,
    pub tier: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Providers {
    #[serde(default)]
    pub providers: BTreeMap<String, Provider>,
    #[serde(default)]
    pub roles: BTreeMap<String, Vec<Choice>>,
}

impl Providers {
    pub fn file(home: &Path) -> PathBuf {
        home.join("providers.yaml")
    }

    /// Who may fill a role, in the order to try. None when the file has no
    /// list for it, and the workflow step's own vendor is used.
    pub fn order(&self, role: &str) -> Option<Vec<Candidate>> {
        let order = self.roles.get(role)?;
        Some(
            order
                .iter()
                .filter_map(|choice| {
                    let provider = self.providers.get(&choice.provider)?;
                    Some(Candidate {
                        provider: choice.provider.clone(),
                        tier: choice.tier.clone(),
                        family: provider.family_of(&choice.tier)?.to_string(),
                        runner: provider.runner(&choice.tier)?,
                        listed: true,
                        quota: provider.quota,
                    })
                })
                .collect(),
        )
    }

    /// None when there is no file. A file that can't be read as one is an
    /// error, as guessing who may do what would be worse.
    pub fn load(home: &Path) -> Result<Option<Providers>> {
        let file = Providers::file(home);
        match std::fs::read_to_string(&file) {
            Ok(text) => parse(&text).map(Some).map_err(|e| anyhow::anyhow!("{}: {e}", file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
        }
    }

    /// Everything wrong with the file that can be told without calling an
    /// agent. An empty list means the file is consistent, not that any
    /// provider works.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, provider) in &self.providers {
            if provider.tiers.is_empty() {
                problems.push(format!("provider {name}: no tiers"));
            }
            for (tier, entry) in &provider.tiers {
                if entry.model().trim().is_empty() {
                    problems.push(format!("provider {name}: tier {tier} has no model"));
                }
                if provider.family_of(tier).is_none_or(|family| family.trim().is_empty()) {
                    problems.push(format!("provider {name}: tier {tier} has no family, on the tier or on the provider"));
                }
            }
            if provider.env_file.is_some() && provider.config_dir.is_some() {
                problems.push(format!("provider {name}: env_file and config_dir both set, use one"));
            }
            if provider.quota == Some(Quota::Zai) && provider.env_file.is_none() {
                problems.push(format!("provider {name}: quota zai needs env_file, which holds the key"));
            }
        }
        for (role, order) in &self.roles {
            if order.is_empty() {
                problems.push(format!("role {role}: no providers"));
            }
            for (at, choice) in order.iter().enumerate() {
                match self.providers.get(&choice.provider) {
                    None => problems.push(format!("role {role}: unknown provider {}", choice.provider)),
                    Some(provider) if !provider.tiers.contains_key(&choice.tier) => {
                        problems.push(format!("role {role}: provider {} has no tier {}", choice.provider, choice.tier))
                    }
                    Some(_) => {}
                }
                if order[..at].contains(choice) {
                    problems.push(format!("role {role}: {} {} is listed twice", choice.provider, choice.tier));
                }
            }
        }
        problems
    }
}

pub fn parse(text: &str) -> Result<Providers, String> {
    if text.trim().is_empty() {
        return Ok(Providers::default());
    }
    crate::yaml::from_str(text)
}

/// What `factory providers` shows: the file, plus what this machine has.
#[derive(Debug, Serialize)]
pub struct Report {
    pub file: String,
    pub found: bool,
    pub providers: Vec<ProviderRow>,
    pub roles: BTreeMap<String, Vec<RoleRow>>,
    pub problems: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ProviderRow {
    pub name: String,
    pub cli: Cli,
    /// Where the program is, or None when it is not on the PATH.
    pub program: Option<String>,
    /// How its login is selected, and whether that file or folder exists.
    pub login: Login,
    pub quota: Option<Quota>,
    /// What the last reading of its plan said, one line per window.
    pub quota_windows: Vec<String>,
    /// When that was read, in seconds since 1970.
    pub quota_read_at: Option<u64>,
    pub tiers: Vec<TierRow>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Login {
    /// The program's own default login.
    Default,
    EnvFile { path: String, exists: bool },
    ConfigDir { path: String, exists: bool },
}

#[derive(Debug, Serialize)]
pub struct TierRow {
    pub tier: String,
    pub model: String,
    pub family: Option<String>,
    /// What `factory providers check` found, or None when it has not been
    /// tried with this model.
    pub check: Option<Checked>,
    /// Set while it is out of quota: until when, in seconds since 1970.
    pub out_until: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct RoleRow {
    pub provider: String,
    pub tier: String,
    pub model: Option<String>,
    pub family: Option<String>,
    /// Whether the engine may use it: it passed its check with this model.
    pub usable: bool,
}

pub fn report(home: &Path) -> Result<Report> {
    let loaded = Providers::load(home)?;
    // Where quota can be read, read it now. It costs nothing.
    for (name, provider) in loaded.iter().flat_map(|set| set.providers.iter()) {
        if let (Some(Quota::Zai), Some(env_file)) = (provider.quota, &provider.env_file) {
            ledger::refresh_zai(home, name, &expand(env_file))?;
        }
    }
    report_as_known(home)
}

/// The same report from what is already on disk, asking nobody. For a
/// caller that is asked often and must answer at once.
pub fn report_as_known(home: &Path) -> Result<Report> {
    let file = Providers::file(home);
    let loaded = Providers::load(home)?;
    Ok(build(&file, loaded.as_ref(), &qualify::load(home), &ledger::load(home), super::util::now_secs(), &|program| on_path(program), &|path| path.exists()))
}

fn build(file: &Path, loaded: Option<&Providers>, checks: &qualify::Store, known: &Ledger, now: u64, find: &dyn Fn(&str) -> Option<PathBuf>, exists: &dyn Fn(&Path) -> bool) -> Report {
    let empty = Providers::default();
    let set = loaded.unwrap_or(&empty);
    let mut problems = set.problems();
    let mut providers = Vec::new();
    for (name, provider) in &set.providers {
        let program = find(provider.cli.program());
        if program.is_none() {
            problems.push(format!("provider {name}: {} is not on the PATH", provider.cli.program()));
        }
        let login = match (&provider.env_file, &provider.config_dir) {
            (Some(path), _) => Login::EnvFile { path: path.clone(), exists: exists(&expand(path)) },
            (None, Some(path)) => Login::ConfigDir { path: path.clone(), exists: exists(&expand(path)) },
            (None, None) => Login::Default,
        };
        if let Login::EnvFile { path, exists: false } | Login::ConfigDir { path, exists: false } = &login {
            problems.push(format!("provider {name}: {path} does not exist"));
        }
        providers.push(ProviderRow {
            name: name.clone(),
            cli: provider.cli,
            program: program.map(|path| path.display().to_string()),
            login,
            quota: provider.quota,
            quota_windows: known.quota.get(name).map(|reading| reading.windows.clone()).unwrap_or_default(),
            quota_read_at: known.quota.get(name).map(|reading| reading.at),
            tiers: provider
                .tiers
                .iter()
                .map(|(tier, entry)| TierRow {
                    tier: tier.clone(),
                    model: entry.model().to_string(),
                    family: provider.family_of(tier).map(str::to_string),
                    check: qualify::current(checks, name, tier, entry.model()).cloned(),
                    out_until: known.out_until(name, &format!("{name}/{}", entry.model()), now),
                })
                .collect(),
        });
    }
    let roles = set
        .roles
        .iter()
        .map(|(role, order)| {
            let rows = order
                .iter()
                .map(|choice| {
                    let provider = set.providers.get(&choice.provider);
                    let model = provider.and_then(|p| p.tiers.get(&choice.tier)).map(|tier| tier.model().to_string());
                    let usable = model.as_deref().and_then(|model| qualify::current(checks, &choice.provider, &choice.tier, model)).is_some_and(|checked| checked.qualified && !checked.unreliable);
                    RoleRow {
                        provider: choice.provider.clone(),
                        tier: choice.tier.clone(),
                        model,
                        family: provider.and_then(|p| p.family_of(&choice.tier)).map(str::to_string),
                        usable,
                    }
                })
                .collect();
            (role.clone(), rows)
        })
        .collect();
    Report { file: file.display().to_string(), found: loaded.is_some(), providers, roles, problems }
}

pub fn print(report: &Report) {
    if !report.found {
        println!("no providers file at {}", report.file);
        println!("the engine uses each step's own vendor until there is one. A starting point:\n");
        print!("{EXAMPLE}");
        return;
    }
    println!("{}\n", report.file);
    println!("providers");
    for row in &report.providers {
        let program = row.program.as_deref().unwrap_or("NOT FOUND");
        let login = match &row.login {
            Login::Default => "default login".to_string(),
            Login::EnvFile { path, exists } => format!("env file {path}{}", if *exists { "" } else { " (MISSING)" }),
            Login::ConfigDir { path, exists } => format!("config folder {path}{}", if *exists { "" } else { " (MISSING)" }),
        };
        let quota = match (row.quota, row.cli) {
            (Some(Quota::Zai), _) => "quota readable (zai)",
            // Both say how much of the plan is used as part of every call.
            (None, Cli::Codex | Cli::Claude) => "quota read after each call",
            (None, Cli::Agy) => "quota known only from errors",
        };
        let read = row.quota_read_at.map(|at| format!(", last read {}", super::util::clock(at))).unwrap_or_default();
        println!("  {}  {} at {program}, {login}, {quota}{read}", row.name, row.cli.program());
        for window in &row.quota_windows {
            println!("    {window}");
        }
        for tier in &row.tiers {
            let check = tier.check.as_ref().map(Checked::summary).unwrap_or_else(|| "not checked".into());
            let out = tier.out_until.map(|until| format!(", OUT OF QUOTA until {}", super::util::clock(until))).unwrap_or_default();
            println!("    {:<10} {:<28} {:<10} {check}{out}", tier.tier, tier.model, tier.family.as_deref().unwrap_or("NO FAMILY"));
        }
    }
    println!("\nroles, in the order tried");
    for (role, order) in &report.roles {
        println!("  {role}");
        for (at, row) in order.iter().enumerate() {
            println!(
                "    {}. {} {}  {} ({}){}",
                at + 1,
                row.provider,
                row.tier,
                row.model.as_deref().unwrap_or("?"),
                row.family.as_deref().unwrap_or("?"),
                if row.usable { "" } else { "  SKIPPED: it has not passed its check" }
            );
        }
    }
    let unlisted = unlisted_roles(report);
    if !unlisted.is_empty() {
        println!("\nno list, so not pinned to a model: {}", unlisted.join(", "));
        println!("  the workflow's vendor answers, with that program's default model on this machine");
    }
    if report.problems.is_empty() {
        println!("\nthe file is consistent. `factory providers check <name>` tries a provider for real.");
    } else {
        println!("\nproblems");
        for problem in &report.problems {
            println!("  {problem}");
        }
    }
}

/// The jobs the file gives no list. Each falls back to the workflow's vendor
/// and to whatever model that program picks by default on this machine.
pub fn unlisted_roles(report: &Report) -> Vec<&'static str> {
    super::providers_edit::ROLES.iter().copied().filter(|role| !report.roles.contains_key(*role)).collect()
}

/// `~` and `~/` are the home directory. Nothing else is expanded.
pub(super) fn expand(path: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    match path.strip_prefix("~/") {
        Some(rest) => home().map(|home| home.join(rest)).unwrap_or_else(|| PathBuf::from(path)),
        None if path == "~" => home().unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(program)).find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_is_consistent() {
        let set = parse(EXAMPLE).unwrap();
        assert_eq!(set.problems(), Vec::<String>::new());
        assert_eq!(set.providers["glm"].cli, Cli::Claude);
        assert_eq!(set.providers["glm"].family_of("high"), Some("zhipu"));
        // A tier's own family wins, and a provider needs none of its own then.
        assert_eq!(set.providers["antigravity"].family_of("high"), Some("google"));
        assert_eq!(set.providers["antigravity"].family_of("missing"), None);
        assert_eq!(set.roles["develop"][0], Choice { provider: "chatgpt".into(), tier: "standard".into() });
        // It survives a round trip.
        assert_eq!(parse(&crate::yaml::to_string(&set).unwrap()).unwrap(), set);
    }

    #[test]
    fn a_tier_can_belong_to_another_family_than_its_provider() {
        let set = parse(
            "providers:\n  antigravity:\n    cli: agy\n    family: google\n    tiers:\n      high: gemini-4-hydrogen-high\n      opus: { model: claude-opus-4-6-thinking, family: anthropic }\n",
        )
        .unwrap();
        let provider = &set.providers["antigravity"];
        assert_eq!((provider.family_of("high"), provider.family_of("opus")), (Some("google"), Some("anthropic")));
    }

    #[test]
    fn a_role_goes_to_the_first_who_is_qualified_independent_and_in_quota() {
        let set = parse(EXAMPLE).unwrap();
        let review = set.order("review").unwrap();
        assert_eq!(review.iter().map(Candidate::limit_key).collect::<Vec<_>>(), ["claude/opus", "glm/opus"]);
        assert_eq!((review[1].family.as_str(), review[1].runner.cli, review[1].runner.env_file.is_some()), ("zhipu", Cli::Claude, true));
        assert!(set.order("judgment").is_none());
        let all = |_: &Candidate| true;
        let none_out = |_: &Candidate| None;

        // In order, when nothing stands in the way.
        assert_eq!(choose(&review, &all, &[], &none_out).unwrap().provider, "claude");
        // Claude developed this issue, so it may not review it.
        assert_eq!(choose(&review, &all, &["anthropic".to_string()], &none_out).unwrap().provider, "glm");
        // Claude is out of quota: the next one takes over.
        let claude_out = |c: &Candidate| (c.provider == "claude").then_some(500);
        assert_eq!(choose(&review, &all, &[], &claude_out).unwrap().provider, "glm");
        // Only what passed its check is used.
        let only_glm = |c: &Candidate| c.provider == "glm";
        assert_eq!(choose(&review, &only_glm, &[], &none_out).unwrap().provider, "glm");

        // Nobody left. A wait helps only where quota is the reason.
        let skipped = choose(&review, &only_glm, &["zhipu".to_string()], &claude_out).unwrap_err();
        assert_eq!(skipped.iter().map(|(_, why)| why.clone()).collect::<Vec<_>>(), [Skipped::NotQualified, Skipped::Family]);
        assert_eq!(back_at(&skipped), None);
        let told = explain("review", &skipped);
        assert!(told.contains("claude high (opus, anthropic): has not passed") && told.contains("its family, zhipu, must stay independent"), "{told}");
        let skipped = choose(&review, &all, &["zhipu".to_string()], &claude_out).unwrap_err();
        assert_eq!(back_at(&skipped), Some(500));

        // A workflow step's own vendor needs no check, and has a family too.
        let step = Candidate::of_step(Vendor::Codex, None);
        assert_eq!((step.family.as_str(), step.limit_key().as_str(), step.label().as_str()), ("openai", "codex/default", "codex"));
        assert_eq!(choose(std::slice::from_ref(&step), &|_| false, &[], &none_out).unwrap(), step);
    }

    #[test]
    fn problems_name_what_is_wrong() {
        let set = parse(
            "providers:\n  a:\n    cli: codex\n    tiers:\n      high: gpt\n  b:\n    cli: claude\n    family: zhipu\n    quota: zai\n    tiers: {}\n  c:\n    cli: claude\n    family: x\n    env_file: e\n    config_dir: d\n    tiers:\n      high: m\nroles:\n  develop:\n    - { use: a, tier: fast }\n    - { use: nobody, tier: high }\n    - { use: c, tier: high }\n    - { use: c, tier: high }\n  review: []\n",
        )
        .unwrap();
        let problems = set.problems().join("\n");
        for expected in [
            "provider a: tier high has no family",
            "provider b: no tiers",
            "provider b: quota zai needs env_file",
            "provider c: env_file and config_dir both set",
            "role develop: provider a has no tier fast",
            "role develop: unknown provider nobody",
            "role develop: c high is listed twice",
            "role review: no providers",
        ] {
            assert!(problems.contains(expected), "missing {expected:?} in:\n{problems}");
        }
    }

    #[test]
    fn a_misspelled_key_or_program_is_an_error() {
        assert!(parse("providers:\n  a:\n    cli: codex\n    familly: x\n").unwrap_err().contains("unknown field"));
        assert!(parse("providers:\n  a:\n    cli: gemini\n").is_err());
        assert!(parse("provider: {}\n").unwrap_err().contains("unknown field"));
        assert_eq!(parse("\n").unwrap(), Providers::default());
    }

    #[test]
    fn the_report_says_what_this_machine_is_missing() {
        let set = parse(EXAMPLE).unwrap();
        let file = PathBuf::from("/home/providers.yaml");
        // Only codex is installed, and no env file exists.
        // GLM's plan was read a moment ago, and Claude's large model hit a limit.
        let mut known = Ledger::default();
        known.quota.insert("glm".into(), ledger::Reading { at: 90, windows: vec!["5-hour window: 12% used".into()], out_until: None });
        known.out.insert("claude/opus".into(), ledger::Entry { until: 500, hits: 1, reason: "usage limit".into(), at: 80 });
        let report = build(&file, Some(&set), &qualify::Store::new(), &known, 100, &|program| (program == "codex").then(|| PathBuf::from("/bin/codex")), &|_| false);
        assert!(report.found);
        let problems = report.problems.join("\n");
        assert!(problems.contains("provider claude: claude is not on the PATH"));
        assert!(problems.contains("provider antigravity: agy is not on the PATH"));
        assert!(problems.contains("provider glm: ~/.zai/env.sh does not exist"));
        assert!(!problems.contains("provider chatgpt"));
        let glm = report.providers.iter().find(|row| row.name == "glm").unwrap();
        assert_eq!(glm.login, Login::EnvFile { path: "~/.zai/env.sh".into(), exists: false });
        assert_eq!((glm.quota_windows.as_slice(), glm.quota_read_at), (["5-hour window: 12% used".to_string()].as_slice(), Some(90)));
        let claude = report.providers.iter().find(|row| row.name == "claude").unwrap();
        let out: Vec<(&str, Option<u64>)> = claude.tiers.iter().map(|tier| (tier.tier.as_str(), tier.out_until)).collect();
        assert_eq!(out, [("fast", None), ("high", Some(500)), ("standard", None)]);
        let develop = &report.roles["develop"];
        assert_eq!((develop[1].model.as_deref(), develop[1].family.as_deref()), (Some("opus"), Some("zhipu")));
        // Nothing has been checked, so nothing may be used yet.
        assert!(!develop[0].usable && !develop[1].usable);
        // Jobs with no list answer with whatever model the program defaults to.
        assert_eq!(unlisted_roles(&report), ["research", "judgment"]);

        // No file is not a problem: the engine carries on as before.
        let none = build(&file, None, &qualify::Store::new(), &Ledger::default(), 100, &|_| None, &|_| false);
        assert!(!none.found && none.problems.is_empty() && none.providers.is_empty());
    }
}
