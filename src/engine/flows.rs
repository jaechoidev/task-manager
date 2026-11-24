//! What the engine checks in an acceptance flow without running it.
//!
//! Maestro itself judges whether a flow passes. These checks only keep the
//! gate deterministic and the accessibility IDs under contract.

use serde_json::Value;

/// Commands that may not appear in a flow. AI commands would put an LLM in
/// the verify gate, and scripts could do anything.
const FORBIDDEN: &[&str] = &["runScript", "evalScript"];

/// A plain file name inside `.maestro/`, safe to hand to a shell.
pub fn safe_name(file: &str) -> bool {
    let stem_ok = |name: &str| {
        !name.is_empty()
            && !name.starts_with('.')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    (file.ends_with(".yaml") || file.ends_with(".yml")) && stem_ok(file)
}

/// The path of a flow, relative to the repository root.
pub fn path(file: &str) -> String {
    format!(".maestro/{file}")
}

/// `<part>.<part>[.<part>...]`, lowercase, words inside a part joined by `-`.
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

fn forbidden(name: &str) -> bool {
    name.ends_with("WithAI") || FORBIDDEN.contains(&name)
}

fn walk(value: &Value, ids: &mut Vec<String>, banned: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if forbidden(key) && !banned.contains(key) {
                    banned.push(key.clone());
                }
                match inner {
                    Value::String(id) if key == "id" => {
                        if !ids.contains(id) {
                            ids.push(id.clone());
                        }
                    }
                    _ => walk(inner, ids, banned),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                // A command with no arguments is a bare string in the list.
                if let Value::String(command) = item {
                    if forbidden(command) && !banned.contains(command) {
                        banned.push(command.clone());
                    }
                }
                walk(item, ids, banned);
            }
        }
        _ => {}
    }
}

#[derive(Debug)]
pub struct Flow {
    /// Every accessibility ID the flow looks for, in order of first use.
    pub ids: Vec<String>,
    /// Forbidden commands the flow uses.
    pub banned: Vec<String>,
}

/// Reads a flow file: a config document, then a list of commands.
pub fn read(text: &str) -> Result<Flow, String> {
    let documents = crate::yaml::documents(text).map_err(|e| format!("not valid YAML: {e}"))?;
    let [config, commands] = documents.as_slice() else {
        return Err(format!("a flow has two YAML documents (config, then commands), but this file has {}", documents.len()));
    };
    if config.get("appId").and_then(Value::as_str).is_none_or(|id| id.trim().is_empty()) {
        return Err("the first document needs an appId".into());
    }
    if commands.as_array().is_none_or(|list| list.is_empty()) {
        return Err("the second document must be a non-empty list of commands".into());
    }
    let mut ids = Vec::new();
    let mut banned = Vec::new();
    walk(commands, &mut ids, &mut banned);
    Ok(Flow { ids, banned })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOW: &str = "
# Covers: adding a todo
appId: com.example.todotrial
---
- launchApp:
    clearState: true
- tapOn:
    id: \"todo.add.button\"
- inputText: \"Buy milk\"
- tapOn: { id: todo.add.save }
- assertVisible: \"Buy milk\"
- assertVisible:
    id: \"todo.add.button\"
";

    #[test]
    fn ids_are_collected_once_in_order() {
        let flow = read(FLOW).unwrap();
        assert_eq!(flow.ids, vec!["todo.add.button", "todo.add.save"]);
        assert!(flow.banned.is_empty());
    }

    #[test]
    fn ai_and_script_commands_are_found() {
        let text = format!("{FLOW}- assertWithAI:\n    assertion: the list looks right\n- assertNoDefectsWithAI\n- runScript: steal.js\n");
        let flow = read(&text).unwrap();
        assert_eq!(flow.banned, vec!["assertWithAI", "assertNoDefectsWithAI", "runScript"]);
    }

    #[test]
    fn malformed_flows_are_rejected() {
        assert!(read("- launchApp\n").unwrap_err().contains("two YAML documents"));
        assert!(read("name: x\n---\n- launchApp\n").unwrap_err().contains("appId"));
        assert!(read("appId: a.b\n---\n[]\n").unwrap_err().contains("non-empty list"));
        assert!(read("appId: [\n---\n- launchApp\n").unwrap_err().contains("not valid YAML"));
    }

    #[test]
    fn names_and_ids_follow_the_rules() {
        assert!(safe_name("add_todo.yaml"));
        assert!(!safe_name("../add.yaml"));
        assert!(!safe_name("add todo.yaml"));
        assert!(!safe_name("add.yaml; rm -rf x"));
        assert!(!safe_name("notes.md"));
        assert!(valid_id("todo.list"));
        assert!(valid_id("settings.theme.dark-toggle"));
        assert!(!valid_id("todo"));
        assert!(!valid_id("Todo.Add"));
        assert!(!valid_id("todo..add"));
        assert!(!valid_id("todo.add button"));
    }
}
