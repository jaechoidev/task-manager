//! Who is out of quota, and until when: `<home>/availability.json`.
//!
//! Every run on the machine reads and writes this one file, so a provider
//! that one issue found out of quota is not tried by the next.
//!
//! Three sources. A call that fails with a usage limit sets the provider and
//! model aside for a while, longer each time it happens again. Z.ai's plan
//! can be read directly, without spending anything, so GLM is set aside
//! until the reset time Z.ai names. Codex and Claude each say how much of
//! the plan is used as part of every call, so the reading taken after the
//! last call is kept too.

use super::lock;
use super::util;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How long a provider is set aside after a usage limit, in minutes: the
/// first time, the second time in a row, and so on. The last one repeats.
pub const WAITS: &[u64] = &[1, 5, 15, 30, 60];

/// A quota reading is trusted this long before it is read again.
const QUOTA_FRESH_SECONDS: u64 = 60;

/// A reading taken from a call can only be renewed by another call. So it
/// sets a provider aside for an hour at most, even when the reset it names is
/// days away: after that one call is tried, and that call brings a new
/// reading. Otherwise one wrong reading could keep a provider out for a week.
const CALL_READING_HOLDS_SECONDS: u64 = 3600;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// By `provider/model`.
    #[serde(default)]
    pub out: BTreeMap<String, Entry>,
    /// Quota readings, by provider.
    #[serde(default)]
    pub quota: BTreeMap<String, Reading>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Seconds since 1970. Past this, it may be tried again.
    pub until: u64,
    /// Limits hit in a row. A good reply ends the row.
    pub hits: u32,
    /// What the provider said.
    pub reason: String,
    /// When it was set aside.
    pub at: u64,
}

/// What a provider's own quota report said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    pub at: u64,
    /// One line per window, such as `5-hour window: 12% used`.
    pub windows: Vec<String>,
    /// Set when a window is used up: when it resets.
    pub out_until: Option<u64>,
}

pub fn file(home: &Path) -> PathBuf {
    home.join("availability.json")
}

/// A missing or unreadable file means nobody is known to be out.
pub fn load(home: &Path) -> Ledger {
    std::fs::read_to_string(file(home)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Reads, changes and writes the file under a lock, so two runs don't lose
/// each other's entries.
fn change(home: &Path, edit: impl FnOnce(&mut Ledger)) -> Result<Ledger> {
    let _held = lock::acquire(home, "availability", &|| {})?;
    let mut ledger = load(home);
    edit(&mut ledger);
    std::fs::create_dir_all(home)?;
    std::fs::write(file(home), serde_json::to_string_pretty(&ledger)?).with_context(|| format!("writing {}", file(home).display()))?;
    Ok(ledger)
}

/// How long to set aside for after this many limits in a row.
pub fn wait_seconds(hits: u32) -> u64 {
    if let Some(seconds) = std::env::var("FACTORY_RATE_WAIT_SECONDS").ok().and_then(|v| v.parse::<u64>().ok()) {
        return seconds;
    }
    WAITS[(hits.saturating_sub(1) as usize).min(WAITS.len() - 1)] * 60
}

/// The entry after one more limit, given what was known before.
fn after_limit(before: Option<&Entry>, reason: &str, now: u64, wait_for: &dyn Fn(u32) -> u64) -> Entry {
    let hits = before.map_or(0, |entry| entry.hits) + 1;
    Entry { until: now + wait_for(hits), hits, reason: reason.chars().take(240).collect(), at: now }
}

/// Records that a call hit a usage limit. Returns until when the provider
/// and model are set aside.
pub fn hit(home: &Path, key: &str, reason: &str) -> Result<u64> {
    let now = util::now_secs();
    let ledger = change(home, |ledger| {
        let entry = after_limit(ledger.out.get(key), reason, now, &wait_seconds);
        ledger.out.insert(key.to_string(), entry);
    })?;
    Ok(ledger.out.get(key).map_or(now, |entry| entry.until))
}

/// Records a good reply: the provider and model are back, and the next
/// limit counts as the first again.
pub fn back(home: &Path, key: &str) -> Result<()> {
    if load(home).out.contains_key(key) {
        change(home, |ledger| {
            ledger.out.remove(key);
        })?;
    }
    Ok(())
}

impl Ledger {
    /// Until when a provider and model are out, if they still are.
    pub fn out_until(&self, provider: &str, key: &str, now: u64) -> Option<u64> {
        let limited = self.out.get(key).map(|entry| entry.until);
        let used_up = self.quota.get(provider).and_then(|reading| reading.out_until);
        limited.into_iter().chain(used_up).filter(|until| *until > now).max()
    }
}

/// What a window of this many minutes is called.
fn window_of_minutes(minutes: u64) -> String {
    match minutes {
        300 => "5-hour window".to_string(),
        10_080 => "weekly window".to_string(),
        m if m % 1440 == 0 => format!("{}-day window", m / 1440),
        m if m % 60 == 0 => format!("{}-hour window", m / 60),
        m => format!("{m}-minute window"),
    }
}

/// Until when a used-up window read from a call sets the provider aside.
fn held_until(resets: Option<u64>, now: u64) -> u64 {
    let limit = now + CALL_READING_HOLDS_SECONDS;
    resets.filter(|at| *at > now).map_or(limit, |at| at.min(limit))
}

/// What Codex writes into its own session file after each reply: how much
/// of each window of the plan is used. The format is Codex's own and is not
/// documented, so anything unexpected gives no reading.
pub fn read_codex(limits: &Value, now: u64) -> Option<Reading> {
    let mut windows = Vec::new();
    let mut out_until: Option<u64> = None;
    let reached = limits.get("rate_limit_reached_type").is_some_and(|kind| !kind.is_null());
    for name in ["primary", "secondary"] {
        let Some(window) = limits.get(name).filter(|window| window.is_object()) else { continue };
        let Some(percent) = window.get("used_percent").and_then(Value::as_f64) else { continue };
        let resets = window.get("resets_at").and_then(Value::as_u64).or_else(|| window.get("resets_in_seconds").and_then(Value::as_u64).map(|seconds| now + seconds));
        let title = window.get("window_minutes").and_then(Value::as_u64).map_or_else(|| format!("{name} window"), window_of_minutes);
        let reset = resets.map_or(String::new(), |at| format!(", resets {}", util::clock(at)));
        windows.push(format!("{title}: {percent:.0}% used{reset}"));
        if percent >= 100.0 {
            let until = held_until(resets, now);
            out_until = Some(out_until.map_or(until, |known| known.max(until)));
        }
    }
    if windows.is_empty() {
        return None;
    }
    if reached && out_until.is_none() {
        out_until = Some(held_until(None, now));
    }
    if let Some(plan) = limits.get("plan_type").and_then(Value::as_str) {
        windows.push(format!("plan: {plan}"));
    }
    Some(Reading { at: now, windows, out_until })
}

/// The `rate_limit_info` of the event Claude prints with each reply in its
/// streaming format. `utilization` is a fraction of the window, and `status`
/// is `allowed`, `allowed_warning` or `rejected`. Only a rejected 5-hour or
/// weekly window puts the whole provider out: a window for one model leaves
/// the others usable, and the failed call itself sets that model aside.
pub fn read_claude(info: &Value, now: u64) -> Option<Reading> {
    let title = |kind: &str| match kind {
        "five_hour" => "5-hour window".to_string(),
        "seven_day" => "weekly window".to_string(),
        "seven_day_opus" => "weekly window for opus".to_string(),
        "seven_day_sonnet" => "weekly window for sonnet".to_string(),
        other => format!("{} window", other.replace('_', " ")),
    };
    let mut windows = Vec::new();
    let mut out_until: Option<u64> = None;
    let mut add = |kind: &str, window: &Value| {
        let status = window.get("status").and_then(Value::as_str).unwrap_or("");
        let resets = window.get("resetsAt").and_then(Value::as_u64);
        let used = match window.get("utilization").and_then(Value::as_f64) {
            Some(fraction) => format!("{:.0}% used", fraction * 100.0),
            None if status == "rejected" => "used up".to_string(),
            None if status == "allowed_warning" => "close to its limit".to_string(),
            None => "in use".to_string(),
        };
        let rejected = if status == "rejected" && !used.ends_with("used up") { ", used up" } else { "" };
        let reset = resets.map_or(String::new(), |at| format!(", resets {}", util::clock(at)));
        windows.push(format!("{}: {used}{rejected}{reset}", title(kind)));
        if status == "rejected" && matches!(kind, "five_hour" | "seven_day") {
            let until = held_until(resets, now);
            out_until = Some(out_until.map_or(until, |known| known.max(until)));
        }
    };
    match info.get("unifiedWindows").and_then(Value::as_object) {
        // Every window of the plan at once, when the program gives them.
        Some(all) if !all.is_empty() => all.iter().for_each(|(kind, window)| add(kind, window)),
        // Otherwise the one window the event is about.
        _ => add(info.get("rateLimitType").and_then(Value::as_str)?, info),
    }
    Some(Reading { at: now, windows, out_until })
}

/// Keeps the reading a call brought back.
pub fn note(home: &Path, provider: &str, reading: Reading) -> Result<()> {
    change(home, |ledger| {
        ledger.quota.insert(provider.to_string(), reading);
    })?;
    Ok(())
}

/// Z.ai's quota report, turned into a reading. Token windows only: the
/// monthly tool quota does not stop a coding call.
pub fn read_zai(body: &Value, now: u64) -> Option<Reading> {
    let limits = body.get("data")?.get("limits")?.as_array()?;
    let mut windows = Vec::new();
    let mut out_until: Option<u64> = None;
    for limit in limits.iter().filter(|limit| limit.get("type").and_then(Value::as_str) == Some("TOKENS_LIMIT")) {
        let percent = limit.get("percentage").and_then(Value::as_f64).unwrap_or(0.0);
        let resets = limit.get("nextResetTime").and_then(Value::as_u64).map(|ms| ms / 1000);
        let name = match limit.get("unit").and_then(Value::as_u64) {
            Some(3) => "5-hour window".to_string(),
            Some(6) => "weekly window".to_string(),
            other => format!("window of unit {}", other.map_or("?".to_string(), |unit| unit.to_string())),
        };
        let reset = resets.map_or(String::new(), |at| format!(", resets {}", util::clock(at)));
        windows.push(format!("{name}: {percent:.0}% used{reset}"));
        if percent >= 100.0 {
            // No reset time given: look again in a quarter of an hour.
            let until = resets.filter(|at| *at > now).unwrap_or(now + 900);
            out_until = Some(out_until.map_or(until, |known| known.max(until)));
        }
    }
    if windows.is_empty() {
        return None;
    }
    Some(Reading { at: now, windows, out_until })
}

/// Asks Z.ai how much of the plan is used. The shell reads the key from the
/// env file and hands it to curl on its standard input, so the key is never
/// in the engine and never on a command line.
const ASK_ZAI: &str = r#"[ -r "$0" ] || exit 97; set -a; . "$0"; set +a; printf 'header = "Authorization: Bearer %s"\n' "$ANTHROPIC_AUTH_TOKEN" | curl -s --max-time 20 -K - https://api.z.ai/api/monitor/usage/quota/limit"#;

fn ask_zai(env_file: &Path) -> Option<Value> {
    let output = Command::new("sh").arg("-c").arg(ASK_ZAI).arg(env_file).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Brings a provider's quota reading up to date, when it is older than a
/// minute. A reading that can't be had changes nothing: a usage limit will
/// still be noticed when a call hits it.
pub fn refresh_zai(home: &Path, provider: &str, env_file: &Path) -> Result<()> {
    let now = util::now_secs();
    if load(home).quota.get(provider).is_some_and(|reading| now.saturating_sub(reading.at) < QUOTA_FRESH_SECONDS) {
        return Ok(());
    }
    read_zai_now(home, provider, env_file).map(|_| ())
}

/// Asks Z.ai now, however fresh the last reading is. Returns the reading,
/// or None when Z.ai gave nothing that can be read.
pub fn read_zai_now(home: &Path, provider: &str, env_file: &Path) -> Result<Option<Reading>> {
    let Some(reading) = ask_zai(env_file).and_then(|body| read_zai(&body, util::now_secs())) else {
        return Ok(None);
    };
    note(home, provider, reading.clone())?;
    Ok(Some(reading))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_limit_in_a_row_sets_aside_for_longer() {
        let minutes = |hits: u32| WAITS[(hits as usize - 1).min(WAITS.len() - 1)] * 60;
        let first = after_limit(None, "usage limit", 1000, &minutes);
        assert_eq!((first.until, first.hits), (1060, 1));
        let second = after_limit(Some(&first), "usage limit", 1100, &minutes);
        assert_eq!((second.until, second.hits), (1100 + 300, 2));
        // The last wait repeats.
        let late = Entry { hits: 40, ..second };
        assert_eq!(after_limit(Some(&late), "usage limit", 0, &minutes).until, 3600);
    }

    #[test]
    fn a_provider_is_out_until_its_latest_reason_ends() {
        let mut ledger = Ledger::default();
        ledger.out.insert("glm/glm-5.3".into(), Entry { until: 500, hits: 1, reason: "429".into(), at: 440 });
        assert_eq!(ledger.out_until("glm", "glm/glm-5.3", 499), Some(500));
        assert_eq!(ledger.out_until("glm", "glm/glm-5.3", 500), None);
        // Another model of the same provider is not out because of it.
        assert_eq!(ledger.out_until("glm", "glm/glm-5.3-flash", 100), None);
        // A used-up plan puts every model of the provider out.
        ledger.quota.insert("glm".into(), Reading { at: 100, windows: Vec::new(), out_until: Some(900) });
        assert_eq!(ledger.out_until("glm", "glm/glm-5.3-flash", 100), Some(900));
        assert_eq!(ledger.out_until("glm", "glm/glm-5.3", 100), Some(900));
        assert_eq!(ledger.out_until("claude", "claude/opus", 100), None);
    }

    #[test]
    fn a_zai_report_says_how_full_each_window_is_and_when_it_resets() {
        let body = serde_json::json!({"code": 200, "data": {"level": "max", "limits": [
            {"type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 12, "nextResetTime": 1_795_582_878_265u64},
            {"type": "TOKENS_LIMIT", "unit": 6, "number": 1, "percentage": 3, "nextResetTime": 1_796_169_663_983u64},
            {"type": "TIME_LIMIT", "unit": 5, "percentage": 100, "nextResetTime": 1_797_206_463_998u64}
        ]}});
        let reading = read_zai(&body, 1_795_570_000).unwrap();
        assert_eq!(reading.out_until, None);
        assert_eq!(reading.windows, ["5-hour window: 12% used, resets 2026-11-25 05:01 UTC", "weekly window: 3% used, resets 2026-12-02 00:01 UTC"]);

        // A full window puts the provider out until that window resets. The
        // tool quota, a different kind of limit, never does.
        let full = serde_json::json!({"data": {"limits": [
            {"type": "TOKENS_LIMIT", "unit": 3, "percentage": 100, "nextResetTime": 1_795_582_878_265u64},
            {"type": "TOKENS_LIMIT", "unit": 6, "percentage": 40, "nextResetTime": 1_796_169_663_983u64}
        ]}});
        assert_eq!(read_zai(&full, 1_795_570_000).unwrap().out_until, Some(1_795_582_878));
        // With no reset time, look again in a quarter of an hour.
        let no_time = serde_json::json!({"data": {"limits": [{"type": "TOKENS_LIMIT", "unit": 3, "percentage": 100}]}});
        assert_eq!(read_zai(&no_time, 1000).unwrap().out_until, Some(1900));
        assert_eq!(read_zai(&serde_json::json!({"code": 401}), 0), None);
    }

    #[test]
    fn a_codex_reading_says_how_full_each_window_is() {
        // What Codex 0.169 wrote into a session file after a real call.
        let real = serde_json::json!({"limit_id": "codex", "limit_name": null, "primary": {"used_percent": 2.0, "window_minutes": 10080, "resets_at": 1_796_115_245u64}, "secondary": null,
            "credits": {"has_credits": false, "unlimited": false, "balance": "0"}, "individual_limit": null, "spend_control_reached": null, "plan_type": "prolite", "rate_limit_reached_type": null});
        let reading = read_codex(&real, 1_795_583_000).unwrap();
        assert_eq!(reading.windows, ["weekly window: 2% used, resets 2026-12-01 08:54 UTC", "plan: prolite"]);
        assert_eq!((reading.at, reading.out_until), (1_795_583_000, None));

        // Two windows, one of them full. It resets in two hours, but a reading
        // from a call holds for an hour at most: then a call is tried again.
        let full = serde_json::json!({"primary": {"used_percent": 100.0, "window_minutes": 300, "resets_in_seconds": 7200}, "secondary": {"used_percent": 41.5, "window_minutes": 10080, "resets_at": 5000}});
        let reading = read_codex(&full, 1000).unwrap();
        assert_eq!(reading.windows[0], "5-hour window: 100% used, resets 1970-01-01 02:16 UTC");
        assert_eq!(reading.windows[1], "weekly window: 42% used, resets 1970-01-01 01:23 UTC");
        assert_eq!(reading.out_until, Some(1000 + 3600));
        // A reset that is nearer than an hour is used as it is.
        let soon = serde_json::json!({"primary": {"used_percent": 100.0, "window_minutes": 300, "resets_at": 1600}});
        assert_eq!(read_codex(&soon, 1000).unwrap().out_until, Some(1600));
        // Codex says a limit was reached without a full window: still out.
        let reached = serde_json::json!({"primary": {"used_percent": 97.0, "window_minutes": 45}, "rate_limit_reached_type": "primary"});
        let reading = read_codex(&reached, 1000).unwrap();
        assert_eq!((reading.windows[0].as_str(), reading.out_until), ("45-minute window: 97% used", Some(4600)));
        // Nothing it can read is no reading, not a wrong one.
        assert_eq!(read_codex(&serde_json::json!({"primary": null, "secondary": null}), 0), None);
        assert_eq!(read_codex(&serde_json::json!({"primary": {"window_minutes": 300}}), 0), None);
    }

    #[test]
    fn a_claude_reading_takes_fractions_and_only_a_plan_wide_rejection_puts_it_out() {
        // What Claude Code 2.1.330 printed with a real reply.
        let real = serde_json::json!({"status": "allowed", "resetsAt": 1_795_590_000u64, "rateLimitType": "five_hour", "overageStatus": "rejected", "isUsingOverage": false,
            "unifiedWindows": {"five_hour": {"utilization": 0.36, "resetsAt": 1_795_590_000u64, "status": "allowed"}, "seven_day": {"utilization": 0.36, "resetsAt": 1_795_960_800u64, "status": "allowed"}}});
        let reading = read_claude(&real, 1_795_583_000).unwrap();
        assert_eq!(reading.windows, ["5-hour window: 36% used, resets 2026-11-25 07:00 UTC", "weekly window: 36% used, resets 2026-11-29 14:00 UTC"]);
        assert_eq!(reading.out_until, None);

        // The older shape: one window, the one the event is about.
        let single = serde_json::json!({"status": "allowed_warning", "resetsAt": 5000, "rateLimitType": "seven_day", "utilization": 0.915});
        assert_eq!(read_claude(&single, 1000).unwrap().windows, ["weekly window: 92% used, resets 1970-01-01 01:23 UTC"]);
        let bare = serde_json::json!({"status": "allowed_warning", "rateLimitType": "five_hour"});
        assert_eq!(read_claude(&bare, 1000).unwrap().windows, ["5-hour window: close to its limit"]);

        // A rejected 5-hour window puts the provider out until it resets.
        let rejected = serde_json::json!({"status": "rejected", "resetsAt": 2500, "rateLimitType": "five_hour"});
        let reading = read_claude(&rejected, 1000).unwrap();
        assert_eq!((reading.windows[0].as_str(), reading.out_until), ("5-hour window: used up, resets 1970-01-01 00:41 UTC", Some(2500)));
        // A window for one model does not: the other models are still usable.
        let opus = serde_json::json!({"status": "rejected", "unifiedWindows": {"five_hour": {"utilization": 0.5, "status": "allowed"}, "seven_day_opus": {"utilization": 1.0, "resetsAt": 9000, "status": "rejected"}}});
        let reading = read_claude(&opus, 1000).unwrap();
        assert_eq!(reading.windows, ["5-hour window: 50% used", "weekly window for opus: 100% used, used up, resets 1970-01-01 02:30 UTC"]);
        assert_eq!(reading.out_until, None);
        assert_eq!(read_claude(&serde_json::json!({"status": "allowed"}), 0), None);
    }

    #[test]
    fn a_reading_from_a_call_is_kept_and_replaced_by_the_next() {
        let home = std::env::temp_dir().join(format!("factory-ledger-note-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        note(&home, "chatgpt", Reading { at: 10, windows: vec!["weekly window: 2% used".into()], out_until: None }).unwrap();
        note(&home, "chatgpt", Reading { at: 20, windows: vec!["weekly window: 100% used".into()], out_until: Some(900) }).unwrap();
        let ledger = load(&home);
        assert_eq!(ledger.quota["chatgpt"].at, 20);
        // Every model of the provider is out until then.
        assert_eq!(ledger.out_until("chatgpt", "chatgpt/gpt-x", 100), Some(900));
        assert_eq!(ledger.out_until("chatgpt", "chatgpt/gpt-x", 900), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn hits_and_good_replies_are_kept_across_processes() {
        let home = std::env::temp_dir().join(format!("factory-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(load(&home), Ledger::default());
        let until = hit(&home, "chatgpt/gpt-x", "You have hit your usage limit").unwrap();
        let now = util::now_secs();
        assert!(until >= now && load(&home).out["chatgpt/gpt-x"].hits == 1);
        hit(&home, "chatgpt/gpt-x", "again").unwrap();
        assert_eq!(load(&home).out["chatgpt/gpt-x"].hits, 2);
        back(&home, "chatgpt/gpt-x").unwrap();
        assert!(load(&home).out.is_empty());
        back(&home, "never/seen").unwrap();
        let _ = std::fs::remove_dir_all(&home);
    }
}
