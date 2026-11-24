//! Loads a repo's workflow (`factory.yaml`) and the modules it names, and
//! checks the chain before anything runs.

use super::github;
use super::judgment::{DEFAULT_JUDGMENTS, MAX_JUDGMENTS};
use super::rounds::{DEFAULT_FIX_ROUNDS, DEFAULT_GITHUB_FIX_ROUNDS, MAX_FIX_ROUNDS};
use anyhow::{bail, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Modules shipped inside the binary. A repo may override one by adding
/// `modules/<kind>/<name>/` with the same file names.
const BUILTIN: &[(&str, &str, &str)] = &[
    ("agent/acceptance", "module.yaml", include_str!("../../modules/agent/acceptance/module.yaml")),
    ("agent/acceptance", "prompt.md", include_str!("../../modules/agent/acceptance/prompt.md")),
    ("agent/acceptance", "report.schema.json", include_str!("../../modules/agent/acceptance/report.schema.json")),
    ("agent/acceptance", "ruling.md", include_str!("../../modules/agent/acceptance/ruling.md")),
    ("agent/acceptance", "ruling.schema.json", include_str!("../../modules/agent/acceptance/ruling.schema.json")),
    ("check/acceptance-red", "module.yaml", include_str!("../../modules/check/acceptance-red/module.yaml")),
    ("agent/dev", "module.yaml", include_str!("../../modules/agent/dev/module.yaml")),
    ("agent/dev", "prompt.md", include_str!("../../modules/agent/dev/prompt.md")),
    ("agent/dev", "fix.md", include_str!("../../modules/agent/dev/fix.md")),
    ("agent/dev", "report.schema.json", include_str!("../../modules/agent/dev/report.schema.json")),
    ("agent/review", "module.yaml", include_str!("../../modules/agent/review/module.yaml")),
    ("agent/review", "prompt.md", include_str!("../../modules/agent/review/prompt.md")),
    ("agent/review", "verdict.schema.json", include_str!("../../modules/agent/review/verdict.schema.json")),
    ("check/verify", "module.yaml", include_str!("../../modules/check/verify/module.yaml")),
    ("agent/research", "module.yaml", include_str!("../../modules/agent/research/module.yaml")),
    ("agent/research", "prompt.md", include_str!("../../modules/agent/research/prompt.md")),
    ("agent/research", "report.schema.json", include_str!("../../modules/agent/research/report.schema.json")),
    ("agent/summary", "module.yaml", include_str!("../../modules/agent/summary/module.yaml")),
    ("agent/summary", "prompt.md", include_str!("../../modules/agent/summary/prompt.md")),
    ("agent/summary", "summary.schema.json", include_str!("../../modules/agent/summary/summary.schema.json")),
    ("judgment/stuck", "module.yaml", include_str!("../../modules/judgment/stuck/module.yaml")),
    ("judgment/stuck", "prompt.md", include_str!("../../modules/judgment/stuck/prompt.md")),
    ("judgment/stuck", "decision.schema.json", include_str!("../../modules/judgment/stuck/decision.schema.json")),
];

/// What the engine itself provides before the first step.
const PROVIDED: &[&str] = &["issue", "worktree", "base_sha"];

/// The names of the modules that ship inside the binary.
pub fn builtin_modules() -> Vec<&'static str> {
    let mut names: Vec<&str> = BUILTIN.iter().map(|(module, _, _)| *module).collect();
    names.dedup();
    names
}

/// Every file of a built-in module: its name and its text.
pub fn builtin_files(module: &str) -> Vec<(&'static str, &'static str)> {
    BUILTIN.iter().filter(|(m, _, _)| *m == module).map(|(_, file, text)| (*file, *text)).collect()
}

pub fn builtin(module: &str, file: &str) -> Option<String> {
    BUILTIN.iter().find(|(m, f, _)| *m == module && *f == file).map(|(_, _, text)| text.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Agent,
    Check,
    Gate,
    Judgment,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Agent => "agent",
            Kind::Check => "check",
            Kind::Gate => "gate",
            Kind::Judgment => "judgment",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Answers open questions before anything else, when the issue needs it.
    Research,
    /// Writes the acceptance flows before development.
    Acceptance,
    Dev,
    Review,
    /// Writes the pull request's summary. Not a step of the sequence.
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Codex,
    Claude,
}

impl Vendor {
    pub fn parse(name: &str) -> Option<Vendor> {
        match name {
            "codex" => Some(Vendor::Codex),
            "claude" => Some(Vendor::Claude),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Vendor::Codex => "codex",
            Vendor::Claude => "claude",
        }
    }
}

/// The module contract, `module.yaml`.
// Some fields are part of the contract before the engine acts on them.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleDef {
    pub name: String,
    pub kind: Kind,
    /// What an agent module is for. The engine treats the two roles differently.
    pub role: Option<Role>,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub produces: Vec<String>,
    /// check: the command to run in the worktree.
    pub run: Option<String>,
    pub pass_when: Option<String>,
    /// agent: file names next to module.yaml.
    pub prompt: Option<String>,
    pub output_schema: Option<String>,
    /// dev: the prompt for a fix round. Without it, a fix round reuses
    /// `prompt` with the failure added at the end.
    pub fix_prompt: Option<String>,
    /// acceptance: the prompt and schema for ruling on a flow change request.
    pub ruling_prompt: Option<String>,
    pub ruling_schema: Option<String>,
    pub access: Option<Access>,
    /// agent: whether it may search and read the web.
    pub web: Option<bool>,
    pub timeout_minutes: Option<u64>,
    /// Shared resources. Declared now, enforced once issues run in parallel.
    #[serde(default)]
    pub locks: Vec<String>,
    /// Reserved for routing a step to a macOS runner.
    pub runs_on: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepSpec {
    module: String,
    vendor: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FactoryFile {
    workflow: Vec<BTreeMap<String, StepSpec>>,
    #[serde(default)]
    on_fail: BTreeMap<String, String>,
    #[serde(default)]
    limits: Limits,
    /// Present when the engine should follow the pull request on GitHub and
    /// merge it. Absent, a run ends with the pull request open.
    github: Option<GithubSpec>,
    /// Who decides what happens to an issue that is stuck. Absent, a stuck
    /// issue goes straight to a person.
    judgment: Option<StepSpec>,
    /// Who writes the pull request's summary. Absent, the body holds only
    /// what the engine records.
    summary: Option<StepSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubSpec {
    wait_minutes: Option<u64>,
    #[serde(default)]
    checks: Vec<String>,
    review_check: Option<String>,
    merge: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    /// How many times a failure may send the issue back to the developer.
    fix_rounds: Option<u32>,
    /// The same, for failures found on GitHub after the pull request opens.
    github_fix_rounds: Option<u32>,
    /// How often a judgment may start one issue over.
    judgments: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub name: String,
    /// For example `agent/dev`.
    pub module: String,
    pub def: ModuleDef,
    pub vendor: Option<Vendor>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub schema: Option<String>,
    pub fix_prompt: Option<String>,
    pub ruling_prompt: Option<String>,
    pub ruling_schema: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Workflow {
    pub steps: Vec<Step>,
    /// Where a failed step returns to.
    pub on_fail: BTreeMap<String, String>,
    /// How many times a failure may send the issue back to the developer.
    pub fix_rounds: u32,
    pub github_fix_rounds: u32,
    pub github: Option<github::Settings>,
    /// The module that decides for a stuck issue, and how often it may start
    /// the issue over.
    pub judgment: Option<Step>,
    pub judgments: u32,
    /// The module that writes the pull request's summary.
    pub summary: Option<Step>,
}

/// `read(module, file)` returns a module file's text, or `None` when the
/// module or file does not exist.
pub fn parse(text: &str, read: &dyn Fn(&str, &str) -> Option<String>) -> Result<Workflow> {
    let file: FactoryFile = match crate::yaml::from_str(text) {
        Ok(file) => file,
        Err(e) => bail!("factory.yaml: {e}"),
    };
    let mut errors: Vec<String> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut available: BTreeSet<String> = PROVIDED.iter().map(|s| s.to_string()).collect();

    if file.workflow.is_empty() {
        errors.push("workflow has no steps".into());
    }
    for entry in &file.workflow {
        let Some((name, spec)) = entry.iter().next().filter(|_| entry.len() == 1) else {
            errors.push("each workflow entry must be one `name: { module: ... }` pair".into());
            continue;
        };
        if !names.insert(name.clone()) {
            errors.push(format!("step '{name}' is defined twice"));
            continue;
        }
        let Some(module_text) = read(&spec.module, "module.yaml") else {
            errors.push(format!("step '{name}': module '{}' does not exist", spec.module));
            continue;
        };
        let def: ModuleDef = match crate::yaml::from_str(&module_text) {
            Ok(def) => def,
            Err(e) => {
                errors.push(format!("module '{}': {e}", spec.module));
                continue;
            }
        };
        let mut problem = |message: String| errors.push(format!("step '{name}' ({}): {message}", spec.module));

        for need in &def.needs {
            if !available.contains(need) {
                problem(format!("needs '{need}', which no earlier step produces"));
            }
        }
        available.extend(def.produces.iter().cloned());

        let mut vendor = None;
        let mut prompt = None;
        let mut schema = None;
        let mut fix_prompt = None;
        let mut ruling_prompt = None;
        let mut ruling_schema = None;
        match def.kind {
            Kind::Agent => {
                match spec.vendor.as_deref().map(|v| (v, Vendor::parse(v))) {
                    Some((_, Some(found))) => vendor = Some(found),
                    Some((given, None)) => problem(format!("unknown vendor '{given}' (use codex or claude)")),
                    None => problem("an agent step needs a vendor".into()),
                }
                if def.role.is_none() {
                    problem("an agent module needs a role (research, acceptance, dev or review)".into());
                }
                if def.access.is_none() {
                    problem("an agent module needs access (read-only or workspace-write)".into());
                }
                if def.role == Some(Role::Review) && def.access != Some(Access::ReadOnly) {
                    problem("a review module must be read-only".into());
                }
                if def.role == Some(Role::Summary) {
                    problem("a summary module goes in the `summary:` block, not in the workflow".into());
                }
                if def.role == Some(Role::Research) && def.access != Some(Access::WorkspaceWrite) {
                    problem("a research module must be workspace-write, since it writes its notes".into());
                }
                if def.role == Some(Role::Acceptance) && def.access != Some(Access::WorkspaceWrite) {
                    problem("an acceptance module must be workspace-write, since it writes flow files".into());
                }
                if def.ruling_prompt.is_some() != def.ruling_schema.is_some() {
                    problem("ruling_prompt and ruling_schema go together".into());
                }
                for (field, name_of, slot, required) in [
                    ("prompt", &def.prompt, &mut prompt, true),
                    ("output_schema", &def.output_schema, &mut schema, true),
                    ("fix_prompt", &def.fix_prompt, &mut fix_prompt, false),
                    ("ruling_prompt", &def.ruling_prompt, &mut ruling_prompt, false),
                    ("ruling_schema", &def.ruling_schema, &mut ruling_schema, false),
                ] {
                    match name_of {
                        None if required => problem(format!("an agent module needs {field}")),
                        None => {}
                        Some(file_name) => match read(&spec.module, file_name) {
                            Some(text) => *slot = Some(text),
                            None => problem(format!("{field} file '{file_name}' does not exist")),
                        },
                    }
                }
                for (field, text) in [("output_schema", &schema), ("ruling_schema", &ruling_schema)] {
                    if text.as_ref().is_some_and(|t| serde_json::from_str::<serde_json::Value>(t).is_err()) {
                        problem(format!("{field} is not valid JSON"));
                    }
                }
            }
            Kind::Check => {
                if def.run.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    problem("a check module needs run".into());
                }
                if def.pass_when.as_deref().map(str::trim) != Some("exit_code == 0") {
                    problem("pass_when must be `exit_code == 0`, the only form supported so far".into());
                }
            }
            Kind::Gate | Kind::Judgment => {
                problem(format!("{} modules are not supported yet", def.kind.name()));
            }
        }

        steps.push(Step {
            name: name.clone(),
            module: spec.module.clone(),
            def,
            vendor,
            model: spec.model.clone(),
            prompt,
            schema,
            fix_prompt,
            ruling_prompt,
            ruling_schema,
        });
    }

    let position_of = |name: &String| file.workflow.iter().position(|entry| entry.keys().next() == Some(name));
    for (from, to) in &file.on_fail {
        for step in [from, to] {
            if !names.contains(step) {
                errors.push(format!("on_fail refers to '{step}', which is not a step"));
            }
        }
        // A failure goes back, never forward: forward would skip steps.
        if let (Some(a), Some(b)) = (position_of(from), position_of(to)) {
            if b >= a {
                errors.push(format!("on_fail sends '{from}' to '{to}', which does not come before it"));
            }
        }
    }
    // Every failure has somewhere to go. A check or a review that fails
    // with no on_fail could only ever stop the run.
    for step in &steps {
        let can_fail = step.def.kind == Kind::Check || step.def.role == Some(Role::Review);
        if can_fail && !file.on_fail.contains_key(&step.name) {
            errors.push(format!("step '{}' can fail and has no on_fail: say which earlier step its failure goes back to", step.name));
        }
    }

    // Whoever writes the tests and whoever judges the code must not be the
    // one who wrote the code: both use a different vendor from the developer.
    // Only an acceptance step and its checks may run before development.
    let position = |role: Role| -> Vec<usize> {
        steps.iter().enumerate().filter(|(_, s)| s.def.role == Some(role)).map(|(i, _)| i).collect()
    };
    // Checked whenever every step could be read, so that one mistake does
    // not hide the others from someone editing the file.
    if steps.len() == file.workflow.len() {
        match position(Role::Dev).as_slice() {
            [dev] => {
                let dev_step = &steps[*dev];
                for other in steps.iter().filter(|s| matches!(s.def.role, Some(Role::Review | Role::Acceptance))) {
                    if other.vendor == dev_step.vendor {
                        errors.push(format!(
                            "step '{}' must use a different vendor from the dev step '{}'",
                            other.name, dev_step.name
                        ));
                    }
                }
                match position(Role::Acceptance).as_slice() {
                    [] => {}
                    [acceptance] if acceptance < dev => {}
                    [_] => errors.push("the acceptance step must come before the dev step".into()),
                    _ => errors.push("the workflow has more than one acceptance step".into()),
                }
                // Research comes before anyone writes flows or code from it.
                let first_user = position(Role::Acceptance).first().copied().unwrap_or(*dev).min(*dev);
                match position(Role::Research).as_slice() {
                    [] => {}
                    [research] if *research < first_user => {}
                    [_] => errors.push("the research step must come before the acceptance and dev steps".into()),
                    _ => errors.push("the workflow has more than one research step".into()),
                }
                if position(Role::Review).iter().any(|review| review < dev) {
                    errors.push("a review step can't come before the dev step".into());
                }
            }
            [] => errors.push("the workflow needs one dev step".into()),
            _ => errors.push("the workflow has more than one dev step".into()),
        }
    }

    let fix_rounds = file.limits.fix_rounds.unwrap_or(DEFAULT_FIX_ROUNDS);
    if fix_rounds > MAX_FIX_ROUNDS {
        errors.push(format!("limits.fix_rounds is {fix_rounds}, and the most the engine allows is {MAX_FIX_ROUNDS}"));
    }

    let github_fix_rounds = file.limits.github_fix_rounds.unwrap_or(DEFAULT_GITHUB_FIX_ROUNDS);
    if github_fix_rounds > MAX_FIX_ROUNDS {
        errors.push(format!("limits.github_fix_rounds is {github_fix_rounds}, and the most the engine allows is {MAX_FIX_ROUNDS}"));
    }
    let github = file.github.map(|spec| github::Settings {
        wait_minutes: spec.wait_minutes.unwrap_or(20),
        checks: spec.checks,
        review_check: spec.review_check.filter(|name| !name.trim().is_empty()),
        merge: spec.merge.unwrap_or_else(|| "squash".to_string()),
    });
    if let Some(settings) = &github {
        if !["squash", "merge", "rebase"].contains(&settings.merge.as_str()) {
            errors.push(format!("github.merge is '{}' (use squash, merge or rebase)", settings.merge));
        }
        if settings.wait_minutes == 0 || settings.wait_minutes > 180 {
            errors.push("github.wait_minutes must be between 1 and 180".to_string());
        }
        for reserved in ["sync", "pr", "github", "merge"] {
            if names.contains(reserved) {
                errors.push(format!("'{reserved}' is a step the engine adds itself and can't be a step name"));
            }
        }
    }

    let judgments = file.limits.judgments.unwrap_or(DEFAULT_JUDGMENTS);
    if judgments > MAX_JUDGMENTS {
        errors.push(format!("limits.judgments is {judgments}, and the most the engine allows is {MAX_JUDGMENTS}"));
    }
    let mut side = |block: &str, spec: &Option<StepSpec>, kind: Kind, role: Option<Role>| match spec {
        None => None,
        Some(spec) => match side_step(block, spec, kind, role, read) {
            Ok(step) => Some(step),
            Err(problem) => {
                errors.push(format!("{block} ({}): {problem}", spec.module));
                None
            }
        },
    };
    let judgment = side("judgment", &file.judgment, Kind::Judgment, None);
    let summary = side("summary", &file.summary, Kind::Agent, Some(Role::Summary));

    if !errors.is_empty() {
        bail!("factory.yaml is not valid:\n  - {}", errors.join("\n  - "));
    }
    Ok(Workflow { steps, on_fail: file.on_fail, fix_rounds, github_fix_rounds, github, judgment, judgments, summary })
}

/// Loads a module the engine calls itself, outside the sequence of steps:
/// the judgment for a stuck run, or the summary of a pull request.
fn side_step(block: &str, spec: &StepSpec, kind: Kind, role: Option<Role>, read: &dyn Fn(&str, &str) -> Option<String>) -> std::result::Result<Step, String> {
    let text = read(&spec.module, "module.yaml").ok_or("the module does not exist")?;
    let def: ModuleDef = crate::yaml::from_str(&text)?;
    if def.kind != kind || def.role != role {
        return Err(format!("it is a {} module, not a {block} module", def.kind.name()));
    }
    if def.access != Some(Access::ReadOnly) {
        return Err(format!("a {block} module must be read-only"));
    }
    let vendor = match spec.vendor.as_deref() {
        Some(name) => Vendor::parse(name).ok_or(format!("unknown vendor '{name}' (use codex or claude)"))?,
        None => return Err("it needs a vendor".into()),
    };
    let file = |field: &str, name: &Option<String>| -> std::result::Result<String, String> {
        let name = name.as_deref().ok_or(format!("it needs {field}"))?;
        read(&spec.module, name).ok_or(format!("{field} file '{name}' does not exist"))
    };
    let prompt = file("prompt", &def.prompt)?;
    let schema = file("output_schema", &def.output_schema)?;
    if serde_json::from_str::<serde_json::Value>(&schema).is_err() {
        return Err("output_schema is not valid JSON".into());
    }
    Ok(Step {
        name: block.to_string(),
        module: spec.module.clone(),
        def,
        vendor: Some(vendor),
        model: spec.model.clone(),
        prompt: Some(prompt),
        schema: Some(schema),
        fix_prompt: None,
        ruling_prompt: None,
        ruling_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "
workflow:
  - develop: { module: agent/dev, vendor: codex }
  - verify:  { module: check/verify }
  - review:  { module: agent/review, vendor: claude }
on_fail:
  verify: develop
  review: develop
";

    fn errors(text: &str) -> String {
        parse(text, &builtin).unwrap_err().to_string()
    }

    #[test]
    fn builtin_workflow_loads() {
        let workflow = parse(GOOD, &builtin).unwrap();
        let names: Vec<&str> = workflow.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["develop", "verify", "review"]);
        assert_eq!(workflow.steps[0].vendor, Some(Vendor::Codex));
        assert!(workflow.steps[2].prompt.as_deref().unwrap().contains("{{diff}}"));
        assert_eq!(workflow.steps[1].def.locks, vec!["simulator"]);
    }

    #[test]
    fn reviewer_must_be_a_different_vendor() {
        let text = GOOD.replace("vendor: claude", "vendor: codex");
        assert!(errors(&text).contains("must use a different vendor"));
    }

    #[test]
    fn needs_must_be_produced_earlier() {
        let text = "
workflow:
  - develop: { module: agent/dev, vendor: codex }
  - review:  { module: agent/review, vendor: claude }
";
        assert!(errors(text).contains("needs 'verify_log', which no earlier step produces"));
    }

    #[test]
    fn unknown_module_vendor_and_on_fail_target_are_reported_together() {
        let text = "
workflow:
  - develop: { module: agent/dev, vendor: gemini }
  - build:   { module: check/xcodebuild }
on_fail:
  build: nowhere
";
        let found = errors(text);
        assert!(found.contains("unknown vendor 'gemini'"), "{found}");
        assert!(found.contains("module 'check/xcodebuild' does not exist"), "{found}");
        assert!(found.contains("on_fail refers to 'nowhere'"), "{found}");
    }

    const WITH_ACCEPTANCE: &str = "
workflow:
  - acceptance: { module: agent/acceptance, vendor: claude }
  - red:        { module: check/acceptance-red }
  - develop:    { module: agent/dev, vendor: codex }
  - verify:     { module: check/verify }
  - review:     { module: agent/review, vendor: claude }
on_fail:
  red: acceptance
  verify: develop
  review: develop
";

    #[test]
    fn acceptance_workflow_loads() {
        let workflow = parse(WITH_ACCEPTANCE, &builtin).unwrap();
        let names: Vec<&str> = workflow.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["acceptance", "red", "develop", "verify", "review"]);
        let acceptance = &workflow.steps[0];
        assert_eq!(acceptance.def.role, Some(Role::Acceptance));
        assert!(acceptance.ruling_prompt.as_deref().unwrap().contains("{{request}}"));
        assert!(acceptance.ruling_schema.as_deref().unwrap().contains("decision"));
        assert!(workflow.steps[1].def.run.as_deref().unwrap().contains("{{new_flows}}"));
    }

    #[test]
    fn fix_rounds_have_a_default_and_a_ceiling() {
        let workflow = parse(WITH_ACCEPTANCE, &builtin).unwrap();
        assert_eq!(workflow.fix_rounds, 3);
        assert_eq!(workflow.on_fail.get("verify").map(String::as_str), Some("develop"));
        assert!(workflow.steps[2].fix_prompt.as_deref().unwrap().contains("{{feedback}}"));

        let none = format!("{WITH_ACCEPTANCE}limits:\n  fix_rounds: 0\n");
        assert_eq!(parse(&none, &builtin).unwrap().fix_rounds, 0);
        let many = format!("{WITH_ACCEPTANCE}limits:\n  fix_rounds: 9\n");
        assert!(errors(&many).contains("the most the engine allows is 5"));
    }

    #[test]
    fn the_github_block_is_optional_and_checked() {
        assert!(parse(WITH_ACCEPTANCE, &builtin).unwrap().github.is_none());
        let with = format!("{WITH_ACCEPTANCE}github:\n  review_check: review\nlimits:\n  github_fix_rounds: 1\n");
        let workflow = parse(&with, &builtin).unwrap();
        let github = workflow.github.unwrap();
        assert_eq!((github.wait_minutes, github.merge.as_str(), github.review_check.as_deref()), (20, "squash", Some("review")));
        assert_eq!((workflow.fix_rounds, workflow.github_fix_rounds), (3, 1));

        let bad = format!("{WITH_ACCEPTANCE}github:\n  merge: fast\n  wait_minutes: 0\n");
        let found = errors(&bad);
        assert!(found.contains("github.merge is 'fast'"), "{found}");
        assert!(found.contains("wait_minutes must be between 1 and 180"), "{found}");
        let reserved = format!("{}github: {{}}\n", WITH_ACCEPTANCE.replace("verify:     {", "merge:      {").replace("verify: develop", "merge: develop"));
        assert!(errors(&reserved).contains("'merge' is a step the engine adds itself"), "{}", errors(&reserved));
    }

    #[test]
    fn the_judgment_block_names_a_judgment_module() {
        assert!(parse(GOOD, &builtin).unwrap().judgment.is_none());
        let with = format!("{GOOD}judgment: {{ module: judgment/stuck, vendor: claude }}\nlimits:\n  judgments: 1\n");
        let workflow = parse(&with, &builtin).unwrap();
        let step = workflow.judgment.unwrap();
        assert_eq!((step.name.as_str(), step.vendor, workflow.judgments), ("judgment", Some(Vendor::Claude), 1));
        assert!(step.prompt.unwrap().contains("{{available}}"));
        assert_eq!(parse(GOOD, &builtin).unwrap().judgments, 2);

        let wrong_kind = format!("{GOOD}judgment: {{ module: agent/review, vendor: claude }}\n");
        assert!(errors(&wrong_kind).contains("it is a agent module, not a judgment module"));
        let too_many = format!("{GOOD}limits:\n  judgments: 9\n");
        assert!(errors(&too_many).contains("limits.judgments is 9"));
        // A judgment is never a step of the sequence.
        let as_step = GOOD.replace("- verify:  { module: check/verify }", "- verify:  { module: judgment/stuck, vendor: claude }");
        assert!(errors(&as_step).contains("judgment modules are not supported"), "{}", errors(&as_step));
    }

    #[test]
    fn research_and_summary_have_their_places() {
        let with = format!("{}summary: {{ module: agent/summary, vendor: claude }}\n", WITH_ACCEPTANCE.replace("workflow:\n", "workflow:\n  - research:   { module: agent/research, vendor: claude }\n"));
        let workflow = parse(&with, &builtin).unwrap();
        assert_eq!(workflow.steps[0].def.role, Some(Role::Research));
        assert_eq!(workflow.steps[0].def.web, Some(true));
        assert_eq!(workflow.summary.unwrap().name, "summary");

        // Research after the flows are written is too late.
        let late = WITH_ACCEPTANCE.replace("  - red:        {", "  - research:   { module: agent/research, vendor: claude }\n  - red:        {");
        assert!(errors(&late).contains("the research step must come before the acceptance and dev steps"), "{}", errors(&late));
        // A summary is not a step, and the summary block takes only a summary module.
        let as_step = GOOD.replace("on_fail:", "  - sum: { module: agent/summary, vendor: claude }\non_fail:");
        assert!(errors(&as_step).contains("goes in the `summary:` block"), "{}", errors(&as_step));
        let wrong = format!("{GOOD}summary: {{ module: agent/review, vendor: claude }}\n");
        assert!(errors(&wrong).contains("not a summary module"), "{}", errors(&wrong));
    }

    #[test]
    fn every_failure_goes_back_to_an_earlier_step() {
        let no_way_back = GOOD.replace("  verify: develop\n", "");
        assert!(errors(&no_way_back).contains("step 'verify' can fail and has no on_fail"), "{}", errors(&no_way_back));
        let forward = GOOD.replace("  verify: develop", "  verify: review");
        assert!(errors(&forward).contains("on_fail sends 'verify' to 'review', which does not come before it"), "{}", errors(&forward));
        let to_itself = GOOD.replace("  review: develop", "  review: review");
        assert!(errors(&to_itself).contains("on_fail sends 'review' to 'review'"), "{}", errors(&to_itself));
    }

    #[test]
    fn built_in_modules_can_be_listed_and_copied() {
        let names = builtin_modules();
        assert!(names.contains(&"agent/dev") && names.contains(&"judgment/stuck") && names.contains(&"check/verify"), "{names:?}");
        assert_eq!(names.len(), names.iter().collect::<std::collections::BTreeSet<_>>().len(), "no name twice");
        let files: Vec<&str> = builtin_files("agent/dev").iter().map(|(file, _)| *file).collect();
        assert_eq!(files, vec!["module.yaml", "prompt.md", "fix.md", "report.schema.json"]);
        assert!(builtin_files("agent/nothing").is_empty());
    }

    #[test]
    fn acceptance_writer_must_differ_from_the_developer() {
        let text = WITH_ACCEPTANCE.replace("agent/acceptance, vendor: claude", "agent/acceptance, vendor: codex");
        assert!(errors(&text).contains("step 'acceptance' must use a different vendor"));
    }

    #[test]
    fn acceptance_must_come_before_development() {
        let text = "
workflow:
  - develop:    { module: agent/dev, vendor: codex }
  - acceptance: { module: agent/acceptance, vendor: claude }
  - verify:     { module: check/verify }
on_fail:
  verify: develop
";
        assert!(errors(text).contains("the acceptance step must come before the dev step"));
        let red_alone = "
workflow:
  - red:     { module: check/acceptance-red }
  - develop: { module: agent/dev, vendor: codex }
";
        assert!(errors(red_alone).contains("needs 'acceptance_flows'"));
    }

    #[test]
    fn dev_step_must_come_first() {
        let text = "
workflow:
  - verify:  { module: check/verify }
  - develop: { module: agent/dev, vendor: codex }
";
        assert!(errors(text).contains("needs 'head_sha'"));
    }
}
