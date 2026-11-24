//! Loads a repo's workflow (`factory.yaml`) and the modules it names, and
//! checks the chain before anything runs.

use anyhow::{bail, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Modules shipped inside the binary. A repo may override one by adding
/// `modules/<kind>/<name>/` with the same file names.
const BUILTIN: &[(&str, &str, &str)] = &[
    ("agent/dev", "module.yaml", include_str!("../../modules/agent/dev/module.yaml")),
    ("agent/dev", "prompt.md", include_str!("../../modules/agent/dev/prompt.md")),
    ("agent/dev", "report.schema.json", include_str!("../../modules/agent/dev/report.schema.json")),
    ("agent/review", "module.yaml", include_str!("../../modules/agent/review/module.yaml")),
    ("agent/review", "prompt.md", include_str!("../../modules/agent/review/prompt.md")),
    ("agent/review", "verdict.schema.json", include_str!("../../modules/agent/review/verdict.schema.json")),
    ("check/verify", "module.yaml", include_str!("../../modules/check/verify/module.yaml")),
];

/// What the engine itself provides before the first step.
const PROVIDED: &[&str] = &["issue", "worktree", "base_sha"];

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
    Dev,
    Review,
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
    pub access: Option<Access>,
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
}

#[derive(Debug, Clone)]
pub struct Workflow {
    pub steps: Vec<Step>,
    /// Where a failed step returns to. Checked now, used once fix loops exist.
    #[allow(dead_code)]
    pub on_fail: BTreeMap<String, String>,
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
        match def.kind {
            Kind::Agent => {
                match spec.vendor.as_deref().map(|v| (v, Vendor::parse(v))) {
                    Some((_, Some(found))) => vendor = Some(found),
                    Some((given, None)) => problem(format!("unknown vendor '{given}' (use codex or claude)")),
                    None => problem("an agent step needs a vendor".into()),
                }
                if def.role.is_none() {
                    problem("an agent module needs a role (dev or review)".into());
                }
                if def.access.is_none() {
                    problem("an agent module needs access (read-only or workspace-write)".into());
                }
                if def.role == Some(Role::Review) && def.access != Some(Access::ReadOnly) {
                    problem("a review module must be read-only".into());
                }
                for (field, name_of, slot) in [
                    ("prompt", &def.prompt, &mut prompt),
                    ("output_schema", &def.output_schema, &mut schema),
                ] {
                    match name_of {
                        None => problem(format!("an agent module needs {field}")),
                        Some(file_name) => match read(&spec.module, file_name) {
                            Some(text) => *slot = Some(text),
                            None => problem(format!("{field} file '{file_name}' does not exist")),
                        },
                    }
                }
                if let Some(text) = &schema {
                    if serde_json::from_str::<serde_json::Value>(text).is_err() {
                        problem("output_schema is not valid JSON".into());
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

        steps.push(Step { name: name.clone(), module: spec.module.clone(), def, vendor, model: spec.model.clone(), prompt, schema });
    }

    for (from, to) in &file.on_fail {
        for step in [from, to] {
            if !names.contains(step) {
                errors.push(format!("on_fail refers to '{step}', which is not a step"));
            }
        }
    }

    // The engine commits the developer's work, so a dev step must come first
    // and the reviewer must be a different vendor.
    let dev: Vec<&Step> = steps.iter().filter(|s| s.def.role == Some(Role::Dev)).collect();
    if errors.is_empty() {
        match dev.as_slice() {
            [only] => {
                if steps.first().map(|s| &s.name) != Some(&only.name) {
                    errors.push("the dev step must be the first step".into());
                }
                for review in steps.iter().filter(|s| s.def.role == Some(Role::Review)) {
                    if review.vendor == only.vendor {
                        errors.push(format!(
                            "step '{}' must use a different vendor from the dev step '{}'",
                            review.name, only.name
                        ));
                    }
                }
            }
            [] => errors.push("the workflow needs one dev step".into()),
            _ => errors.push("the workflow has more than one dev step".into()),
        }
    }

    if !errors.is_empty() {
        bail!("factory.yaml is not valid:\n  - {}", errors.join("\n  - "));
    }
    Ok(Workflow { steps, on_fail: file.on_fail })
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
