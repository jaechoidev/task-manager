//! What `factory project add` looks at in a checkout before registering it.
//! A checkout that can't pass a single issue is refused. One whose commits
//! would carry another name than the agent's gets a note on how to change it.

use super::git;
use crate::gh;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Default)]
pub struct Findings {
    /// Each one stops the checkout from being registered.
    pub refusals: Vec<String>,
    /// Worth knowing, and nothing stops.
    pub notes: Vec<String>,
}

/// The value a template's `verify.sh` gives a setting it writes as
/// `NAME="${NAME:-value}"`. None when the file doesn't set it that way, as
/// a `verify.sh` of another kind of project need not.
pub fn default_of(script: &str, name: &str) -> Option<String> {
    let start = format!("{name}=\"${{{name}:-");
    script.lines().find_map(|line| line.trim_start().strip_prefix(&start)?.split_once("}\"").map(|(value, _)| value.trim().to_string()))
}

/// What keeps a checkout's `verify.sh` from passing any issue.
pub fn verify_problems(script: Option<&str>, executable: bool) -> Vec<String> {
    let Some(script) = script else {
        return vec!["it has no verify.sh. Every project needs one: it is the gate each issue passes, the build and then every flow".to_string()];
    };
    let mut problems = Vec::new();
    if !executable {
        problems.push("verify.sh is not executable. Run `chmod +x verify.sh` and commit it".to_string());
    }
    for (name, example) in [("SCHEME", "MyApp"), ("PROJECT", "MyApp.xcodeproj")] {
        if default_of(script, name).is_some_and(|value| value.is_empty()) {
            problems.push(format!("verify.sh leaves {name} empty. Set it in the file, as in {name}=\"${{{name}:-{example}}}\", and commit it"));
        }
    }
    problems
}

/// A note when the engine's commits in this checkout would carry another
/// name than the account `gh` works as. Commits take the checkout's git
/// identity, and the engine sets none of its own.
pub fn identity_note(dir: &str, name: Option<&str>, login: &str, id: u64) -> Option<String> {
    if name == Some(login) {
        return None;
    }
    let carried = name.map(|name| format!("the name '{name}'")).unwrap_or_else(|| "no name".to_string());
    Some(format!(
        "commits the engine makes here carry {carried}, while gh works as {login}. To give them the agent's name:\n  git -C {dir} config user.name {login}\n  git -C {dir} config user.email {id}+{login}@users.noreply.github.com"
    ))
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.exists()
    }
}

/// Looks at a checkout. Asks GitHub only which account `gh` works as.
pub fn check(dir: &Path) -> Result<Findings> {
    let mut findings = Findings::default();
    let verify = dir.join("verify.sh");
    let script = std::fs::read_to_string(&verify).ok();
    findings.refusals = verify_problems(script.as_deref(), executable(&verify));
    let name = git::git(dir, &["config", "user.name"]).ok().map(|name| name.trim().to_string()).filter(|name| !name.is_empty());
    match gh::api("GET", "user", None) {
        Ok(user) => {
            let login = user.get("login").and_then(Value::as_str).unwrap_or_default();
            let id = user.get("id").and_then(Value::as_u64).unwrap_or_default();
            if let Some(note) = identity_note(&dir.display().to_string(), name.as_deref(), login, id) {
                findings.notes.push(note);
            }
        }
        Err(e) => findings.notes.push(format!("could not ask gh which account it works as, so the commit name was not compared: {e}")),
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_templates_settings_are_read_from_verify_sh() {
        let script = "#!/bin/sh\nSCHEME=\"${SCHEME:-CounterTrial}\"\n  PROJECT=\"${PROJECT:-}\"     # for example MyApp.xcodeproj\n";
        assert_eq!(default_of(script, "SCHEME").as_deref(), Some("CounterTrial"));
        assert_eq!(default_of(script, "PROJECT").as_deref(), Some(""));
        // A verify.sh of another kind sets neither, and that is fine.
        assert_eq!(default_of("#!/bin/sh\ncargo test\n", "SCHEME"), None);
    }

    #[test]
    fn a_verify_sh_that_cant_pass_anything_is_refused() {
        assert!(verify_problems(None, false)[0].contains("no verify.sh"));
        let empty = "SCHEME=\"${SCHEME:-}\"\nPROJECT=\"${PROJECT:-}\"\n";
        let problems = verify_problems(Some(empty), true);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("SCHEME") && problems[1].contains("PROJECT"), "{problems:?}");
        assert!(verify_problems(Some("SCHEME=\"${SCHEME:-App}\"\n"), false)[0].contains("chmod +x"));
        assert!(verify_problems(Some("SCHEME=\"${SCHEME:-App}\"\nPROJECT=\"${PROJECT:-App.xcodeproj}\"\n"), true).is_empty());
        assert!(verify_problems(Some("#!/bin/sh\nnpm test\n"), true).is_empty());
    }

    #[test]
    fn commits_that_would_not_carry_the_agents_name_get_a_note() {
        assert_eq!(identity_note("/code/app", Some("neostudiodot"), "neostudiodot", 1), None);
        let note = identity_note("/code/app", Some("Jae Choi"), "neostudiodot", 360024705).unwrap();
        assert!(note.contains("'Jae Choi'") && note.contains("gh works as neostudiodot"), "{note}");
        assert!(note.contains("git -C /code/app config user.email 360024705+neostudiodot@users.noreply.github.com"), "{note}");
        assert!(identity_note("/code/app", None, "neostudiodot", 1).unwrap().contains("carry no name"));
    }
}
