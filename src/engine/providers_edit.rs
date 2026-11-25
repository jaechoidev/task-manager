//! Changing `providers.yaml` from a client: which model a tier names, and
//! who is tried for each job, in what order.
//!
//! The file is the user's, with their comments, so it is not written out
//! again from scratch. A tier's line and the `roles:` block are rewritten in
//! place and everything else is left as it is. The result is then read back
//! and compared with what was meant: if the file is laid out in a way this
//! can't follow, nothing is written and the answer says so.
//!
//! What the engine may use does not change here. A tier whose model changes
//! has no check for that model yet, and the engine only uses what passed.

use super::providers::{self, Choice, Providers, Tier};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The jobs a list can be given for, in the order a run does them.
pub const ROLES: &[&str] = &["research", "acceptance", "develop", "review", "summary", "judgment"];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierChange {
    pub provider: String,
    pub tier: String,
    pub model: String,
}

/// What a client asks for. `roles`, when given, is every list there is to
/// be: a job left out of it goes back to the workflow's own vendor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    #[serde(default)]
    pub tiers: Vec<TierChange>,
    #[serde(default)]
    pub roles: Option<BTreeMap<String, Vec<Choice>>>,
}

fn plain(text: &str, first: impl Fn(char) -> bool, rest: impl Fn(char) -> bool) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(first) && chars.all(rest) && text.len() <= 100
}

fn tier_name_ok(name: &str) -> bool {
    plain(name, |c| c.is_ascii_lowercase() || c.is_ascii_digit(), |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A model name is passed to a program as one argument and written into a
/// YAML file, so it is kept to what both take without quoting games.
fn model_ok(name: &str) -> bool {
    plain(name, |c| c.is_ascii_alphanumeric(), |c| c.is_ascii_alphanumeric() || "._/:@+[]-".contains(c))
}

/// The set of providers after the change, or everything wrong with it.
pub fn apply(set: &Providers, change: &Change) -> Result<Providers, Vec<String>> {
    let mut after = set.clone();
    let mut problems = Vec::new();
    for wanted in &change.tiers {
        let Some(provider) = after.providers.get_mut(&wanted.provider) else {
            problems.push(format!("there is no provider {}", wanted.provider));
            continue;
        };
        if !tier_name_ok(&wanted.tier) {
            problems.push(format!("'{}' can't be a tier's name: use small letters, digits, - and _", wanted.tier));
            continue;
        }
        if !model_ok(wanted.model.trim()) {
            problems.push(format!("'{}' can't be a model's name here: use letters, digits and . _ / : @ + [ ] -", wanted.model));
            continue;
        }
        let model = wanted.model.trim().to_string();
        let entry = match provider.tiers.get(&wanted.tier) {
            // A tier with a family of its own keeps it.
            Some(Tier::Detailed { family, .. }) => Tier::Detailed { model, family: family.clone() },
            _ => Tier::Model(model),
        };
        provider.tiers.insert(wanted.tier.clone(), entry);
    }
    if let Some(roles) = &change.roles {
        for role in roles.keys().filter(|role| !ROLES.contains(&role.as_str())) {
            problems.push(format!("'{role}' is not a job. The jobs are {}", ROLES.join(", ")));
        }
        // A job with nobody listed is a job with no list.
        after.roles = roles.iter().filter(|(_, order)| !order.is_empty()).map(|(role, order)| (role.clone(), order.clone())).collect();
    }
    problems.extend(after.problems());
    if problems.is_empty() {
        Ok(after)
    } else {
        Err(problems)
    }
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn is_content(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && !trimmed.starts_with('#')
}

/// The lines of the block that belongs to the key on line `at`: every line
/// after it up to, and not including, the next line of content that is not
/// indented deeper. Blank lines and comments at the end are left outside.
fn block_end(lines: &[String], at: usize) -> usize {
    let depth = indent(&lines[at]);
    let mut end = at + 1;
    for (index, line) in lines.iter().enumerate().skip(at + 1) {
        if is_content(line) {
            if indent(line) <= depth {
                break;
            }
            end = index + 1;
        }
    }
    end
}

/// The line inside `range` that sets `key`, at the depth of the first
/// line of content in it.
fn child(lines: &[String], range: std::ops::Range<usize>, key: &str) -> Option<usize> {
    let depth = lines[range.clone()].iter().find(|line| is_content(line)).map(|line| indent(line))?;
    range.into_iter().find(|index| {
        let line = &lines[*index];
        is_content(line) && indent(line) == depth && line.trim_start().strip_prefix(key).is_some_and(|rest| rest.starts_with(':'))
    })
}

fn written(model: &str) -> String {
    // Brackets, colons and the like would mean something to YAML unquoted.
    if model.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c)) {
        model.to_string()
    } else {
        format!("\"{model}\"")
    }
}

fn set_tier(lines: &mut Vec<String>, wanted: &TierChange) -> Result<(), String> {
    let cant = |why: &str| format!("provider {}, tier {}: {why}", wanted.provider, wanted.tier);
    let top = lines.iter().position(|line| indent(line) == 0 && line.trim_end() == "providers:").ok_or_else(|| cant("the file has no `providers:` block on a line of its own"))?;
    let provider = child(lines, top + 1..block_end(lines, top), &wanted.provider).ok_or_else(|| cant("its provider's line was not found"))?;
    let tiers = child(lines, provider + 1..block_end(lines, provider), "tiers").ok_or_else(|| cant("its provider has no `tiers:` line"))?;
    let beside = lines[tiers].trim().trim_start_matches("tiers:").trim();
    if !beside.is_empty() && !beside.starts_with('#') {
        return Err(cant("its tiers are written on one line"));
    }
    let end = block_end(lines, tiers);
    let model = written(wanted.model.trim());
    match child(lines, tiers + 1..end, &wanted.tier) {
        Some(at) => {
            let line = lines[at].clone();
            let colon = line.find(':').ok_or_else(|| cant("its line has no colon"))?;
            let (key, rest) = line.split_at(colon + 1);
            let value_start = rest.len() - rest.trim_start().len();
            let value = &rest[value_start..];
            let new_rest = if value.starts_with('{') {
                // `{ model: x, family: y }`: only the model changes.
                let at_model = value.find("model:").ok_or_else(|| cant("its line has no model"))?;
                let after = &value[at_model + "model:".len()..];
                let stop = after.find([',', '}']).ok_or_else(|| cant("its line does not end"))?;
                format!("{}{}model: {model}{}", &rest[..value_start], &value[..at_model], &after[stop..])
            } else {
                // The value ends at its closing quote, or at the first
                // space. What follows, spacing and comment, is kept.
                let stop = match value.strip_prefix('"').and_then(|inner| inner.find('"')) {
                    Some(close) => close + 2,
                    None => value.find(' ').unwrap_or(value.trim_end().len()),
                };
                format!("{}{model}{}", &rest[..value_start], &value[stop..])
            };
            lines[at] = format!("{key}{new_rest}");
        }
        None => {
            let depth = lines[tiers + 1..end].iter().find(|line| is_content(line)).map_or(indent(&lines[tiers]) + 2, |line| indent(line));
            lines.insert(end, format!("{}{}: {model}", " ".repeat(depth), wanted.tier));
        }
    }
    Ok(())
}

fn roles_block(roles: &BTreeMap<String, Vec<Choice>>) -> Vec<String> {
    let mut out = vec!["roles:".to_string()];
    for role in ROLES.iter().filter(|role| roles.contains_key(**role)) {
        out.push(format!("  {role}:"));
        for choice in &roles[*role] {
            out.push(format!("    - {{ use: {}, tier: {} }}", choice.provider, choice.tier));
        }
    }
    out
}

fn set_roles(lines: &mut Vec<String>, roles: &BTreeMap<String, Vec<Choice>>) {
    let block = if roles.is_empty() { Vec::new() } else { roles_block(roles) };
    match lines.iter().position(|line| indent(line) == 0 && line.trim_start().starts_with("roles:")) {
        Some(at) => {
            let end = block_end(lines, at);
            lines.splice(at..end, block);
        }
        None if block.is_empty() => {}
        None => {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.extend(block);
        }
    }
}

/// The file's text with the change made in place. Comments and everything
/// the change does not touch stay as they are.
pub fn rewrite(text: &str, change: &Change, after: &Providers) -> Result<String, String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for wanted in &change.tiers {
        set_tier(&mut lines, wanted)?;
    }
    if change.roles.is_some() {
        set_roles(&mut lines, &after.roles);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    // Read it back. What is on disk must be what was asked for, or nothing
    // is written.
    match providers::parse(&out) {
        Ok(read) if &read == after => Ok(out),
        Ok(_) => Err("the file is laid out in a way this can't change safely. Edit it by hand".to_string()),
        Err(e) => Err(format!("the change would leave a file that can't be read: {e}. Edit it by hand")),
    }
}

/// Which tiers name another model after the change than before.
pub fn changed_tiers(before: &Providers, after: &Providers) -> Vec<(String, String)> {
    let mut changed = Vec::new();
    for (name, provider) in &after.providers {
        for (tier, entry) in &provider.tiers {
            let was = before.providers.get(name).and_then(|old| old.tiers.get(tier)).map(Tier::model);
            if was != Some(entry.model()) {
                changed.push((name.clone(), tier.clone()));
            }
        }
    }
    changed
}

/// The tiers whose model changed, as provider and tier. Or everything
/// wrong with the change, for the person who asked.
pub type Saved = Result<Vec<(String, String)>, Vec<String>>;

/// Makes the change in `<home>/providers.yaml`. The file as it was is kept
/// beside it as `providers.yaml.bak`.
pub fn save(home: &Path, change: &Change) -> Result<Saved> {
    let file = Providers::file(home);
    let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let before = match providers::parse(&text) {
        Ok(set) => set,
        Err(e) => return Ok(Err(vec![format!("{} can't be read as it is: {e}", file.display())])),
    };
    let after = match apply(&before, change) {
        Ok(after) => after,
        Err(problems) => return Ok(Err(problems)),
    };
    if after == before {
        return Ok(Ok(Vec::new()));
    }
    let new_text = match rewrite(&text, change, &after) {
        Ok(new_text) => new_text,
        Err(why) => return Ok(Err(vec![why])),
    };
    std::fs::write(file.with_extension("yaml.bak"), &text).with_context(|| "keeping the file as it was".to_string())?;
    let partial = file.with_extension("yaml.partial");
    std::fs::write(&partial, new_text).with_context(|| format!("writing {}", partial.display()))?;
    std::fs::rename(&partial, &file).with_context(|| format!("writing {}", file.display()))?;
    Ok(Ok(changed_tiers(&before, &after)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# Agent subscriptions on this machine.\nproviders:\n  chatgpt:\n    cli: codex\n    family: openai\n    tiers:\n      sol: gpt-6.2-sol\n      astra: gpt-6.2-astra\n  glm:\n    cli: claude\n    family: zhipu\n    # Sets the endpoint and key for this login.\n    env_file: ~/.zai/env.sh\n    quota: zai\n    tiers:\n      high: opus        # mapped to glm-5.3\n      own: { model: sonnet, family: other }\n\n# For each job, who to try, in order.\nroles:\n  develop:\n    - { use: chatgpt, tier: sol }\n    - { use: glm, tier: high }\n\n# The end.\n";

    fn choice(provider: &str, tier: &str) -> Choice {
        Choice { provider: provider.into(), tier: tier.into() }
    }

    fn tier(provider: &str, tier: &str, model: &str) -> TierChange {
        TierChange { provider: provider.into(), tier: tier.into(), model: model.into() }
    }

    fn made(change: &Change) -> String {
        let before = providers::parse(FILE).unwrap();
        let after = apply(&before, change).unwrap();
        rewrite(FILE, change, &after).unwrap()
    }

    #[test]
    fn a_tier_s_model_changes_on_its_own_line_and_nothing_else_moves() {
        let text = made(&Change { tiers: vec![tier("chatgpt", "sol", "gpt-7"), tier("glm", "high", "opus[1m]"), tier("glm", "own", "haiku")], roles: None });
        // The comment after a value stays, and so does a tier's own family.
        assert!(text.contains("      sol: gpt-7\n      astra: gpt-6.2-astra\n"), "{text}");
        assert!(text.contains("      high: \"opus[1m]\"        # mapped to glm-5.3\n"), "{text}");
        assert!(text.contains("      own: { model: haiku, family: other }\n"), "{text}");
        // Every other line is as it was.
        let same = |text: &str| text.lines().filter(|line| !line.contains("sol:") && !line.contains("high:") && !line.contains("own:")).map(str::to_string).collect::<Vec<_>>();
        assert_eq!(same(&text), same(FILE));
    }

    #[test]
    fn a_new_tier_is_added_under_its_provider() {
        let text = made(&Change { tiers: vec![tier("chatgpt", "mini", "gpt-6-mini")], roles: None });
        assert!(text.contains("      astra: gpt-6.2-astra\n      mini: gpt-6-mini\n  glm:\n"), "{text}");
    }

    #[test]
    fn the_role_lists_are_replaced_as_a_block_and_the_comments_around_them_stay() {
        let roles = BTreeMap::from([
            ("review".to_string(), vec![choice("glm", "high")]),
            ("develop".to_string(), vec![choice("glm", "high"), choice("chatgpt", "astra")]),
            ("summary".to_string(), Vec::new()),
        ]);
        let text = made(&Change { tiers: Vec::new(), roles: Some(roles) });
        // In the order a run does the jobs. A job with nobody listed has no list.
        assert!(text.ends_with("# For each job, who to try, in order.\nroles:\n  develop:\n    - { use: glm, tier: high }\n    - { use: chatgpt, tier: astra }\n  review:\n    - { use: glm, tier: high }\n\n# The end.\n"), "{text}");
        // No lists at all: the block goes, the comments stay.
        let none = made(&Change { tiers: Vec::new(), roles: Some(BTreeMap::new()) });
        assert!(none.ends_with("# For each job, who to try, in order.\n\n# The end.\n") && !none.contains("roles:"), "{none}");
        // A file with no lists gets its first.
        let bare = "providers:\n  a:\n    cli: codex\n    family: f\n    tiers:\n      t: m\n";
        let change = Change { tiers: Vec::new(), roles: Some(BTreeMap::from([("develop".to_string(), vec![choice("a", "t")])])) };
        let after = apply(&providers::parse(bare).unwrap(), &change).unwrap();
        assert_eq!(rewrite(bare, &change, &after).unwrap(), format!("{bare}\nroles:\n  develop:\n    - {{ use: a, tier: t }}\n"));
    }

    #[test]
    fn a_change_that_makes_no_sense_is_refused_with_every_reason() {
        let before = providers::parse(FILE).unwrap();
        let bad = Change {
            tiers: vec![tier("nobody", "high", "m"), tier("glm", "Big Tier", "m"), tier("glm", "high", "two words"), tier("glm", "high", "")],
            roles: Some(BTreeMap::from([("cooking".to_string(), vec![choice("glm", "high")]), ("review".to_string(), vec![choice("glm", "none"), choice("ghost", "x")])])),
        };
        let problems = apply(&before, &bad).unwrap_err().join("\n");
        for expected in ["there is no provider nobody", "'Big Tier' can't be a tier's name", "'two words' can't be a model's name", "'' can't be a model's name", "'cooking' is not a job", "role review: provider glm has no tier none", "role review: unknown provider ghost"] {
            assert!(problems.contains(expected), "missing '{expected}' in:\n{problems}");
        }
    }

    #[test]
    fn a_file_this_can_t_follow_is_left_alone() {
        // Tiers on one line: the text edit can't find a tier's line.
        let inline = "providers:\n  a:\n    cli: codex\n    family: f\n    tiers: { t: m }\n";
        let change = Change { tiers: vec![tier("a", "t", "n")], roles: None };
        let after = apply(&providers::parse(inline).unwrap(), &change).unwrap();
        assert!(rewrite(inline, &change, &after).is_err());
    }

    #[test]
    fn saving_keeps_the_old_file_and_says_which_tiers_need_a_new_check() {
        let home = std::env::temp_dir().join(format!("factory-providers-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("providers.yaml"), FILE).unwrap();
        let change = Change { tiers: vec![tier("chatgpt", "sol", "gpt-7"), tier("chatgpt", "astra", "gpt-6.2-astra")], roles: None };
        // Only the tier whose model is another one now.
        assert_eq!(save(&home, &change).unwrap().unwrap(), [("chatgpt".to_string(), "sol".to_string())]);
        assert_eq!(std::fs::read_to_string(home.join("providers.yaml.bak")).unwrap(), FILE);
        assert!(std::fs::read_to_string(home.join("providers.yaml")).unwrap().contains("sol: gpt-7\n"));
        // A refused change writes nothing, and one that changes nothing neither.
        let now = std::fs::read_to_string(home.join("providers.yaml")).unwrap();
        assert!(save(&home, &Change { tiers: vec![tier("ghost", "t", "m")], roles: None }).unwrap().is_err());
        assert!(save(&home, &Change::default()).unwrap().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(home.join("providers.yaml")).unwrap(), now);
        let _ = std::fs::remove_dir_all(&home);
    }
}
