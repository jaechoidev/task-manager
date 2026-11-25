//! Small helpers: where the engine keeps its files, and UTC timestamps.

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// `FACTORY_HOME`, or `~/.factory`. Holds the database, logs and worktrees.
pub fn home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("FACTORY_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set; set FACTORY_HOME"))?;
    Ok(PathBuf::from(home).join(".factory"))
}

/// Whether a process of this user still exists. Asks the system, and takes
/// nothing from the process.
pub fn alive(pid: u32) -> bool {
    use std::process::{Command, Stdio};
    Command::new("kill").args(["-0", &pid.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
}

/// `owner/name` as one path segment.
pub fn repo_dir_name(repo: &str) -> String {
    repo.replace('/', "__")
}

fn civil(secs: u64) -> (i64, u32, u32, u64, u64, u64) {
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rem / 3_600, rem % 3_600 / 60, rem % 60)
}

/// Seconds since 1970.
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A time in seconds since 1970 as `2026-11-24 09:15 UTC`.
pub fn clock(secs: u64) -> String {
    let (y, mo, d, h, mi, _) = civil(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC")
}

/// `2026-11-24T09:15:02Z`
pub fn now() -> String {
    let (y, mo, d, h, mi, s) = civil(now_secs());
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `20261124T091502Z`, safe in a file name.
pub fn run_id() -> String {
    let (y, mo, d, h, mi, s) = civil(now_secs());
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::civil;

    #[test]
    fn calendar_dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(civil(1_795_511_702), (2026, 11, 24, 9, 15, 2));
        assert_eq!(super::clock(1_795_511_702), "2026-11-24 09:15 UTC");
    }
}
