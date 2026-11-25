//! The state of record: one SQLite database, plus a JSONL request log.
//!
//! GitHub labels only mirror this for visibility. After a crash the database
//! says where each issue stopped.

use super::util;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS issues (
    repo        TEXT NOT NULL,
    number      INTEGER NOT NULL,
    plan        TEXT NOT NULL,
    key         TEXT NOT NULL,
    title       TEXT NOT NULL,
    run_id      TEXT NOT NULL,
    stage       TEXT NOT NULL,
    status      TEXT NOT NULL,
    branch      TEXT,
    worktree    TEXT,
    base_sha    TEXT,
    head_sha    TEXT,
    pr_number   INTEGER,
    pr_url      TEXT,
    failure     TEXT,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (repo, number)
);
CREATE TABLE IF NOT EXISTS steps (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    repo         TEXT NOT NULL,
    number       INTEGER NOT NULL,
    run_id       TEXT NOT NULL,
    step         TEXT NOT NULL,
    kind         TEXT NOT NULL,
    module       TEXT NOT NULL,
    vendor       TEXT,
    model        TEXT,
    tokens_in    INTEGER,
    tokens_cached INTEGER,
    tokens_out   INTEGER,
    round        INTEGER NOT NULL DEFAULT 0,
    attempt      INTEGER NOT NULL,
    sha          TEXT,
    exit_code    INTEGER,
    timed_out    INTEGER NOT NULL,
    verdict      TEXT NOT NULL,
    duration_ms  INTEGER NOT NULL,
    input_path   TEXT,
    output_path  TEXT,
    session_id   TEXT,
    finished_at  TEXT NOT NULL
);
";

/// One line of the request log, and one row of `steps`.
#[derive(Debug, Clone, Serialize)]
pub struct StepRecord {
    pub ts: String,
    pub repo: String,
    pub issue: u64,
    pub run_id: String,
    pub step: String,
    pub kind: String,
    pub module: String,
    pub vendor: Option<String>,
    pub model: Option<String>,
    /// Which account ran the call. Only one per vendor exists so far.
    pub subscription: Option<String>,
    /// 0 for the first pass, then one per fix round.
    pub round: u32,
    pub attempt: u32,
    pub sha: Option<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// `pass`, `fail`, or an agent's own verdict such as `approve`.
    pub verdict: String,
    pub duration_ms: u64,
    pub input: Option<String>,
    pub output: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub session_id: Option<String>,
    /// What an agent call used, as its program counted it: all input, the
    /// part of it read from a cache, and the output.
    pub tokens_in: Option<u64>,
    pub tokens_cached: Option<u64>,
    pub tokens_out: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct IssueRow {
    pub repo: String,
    pub number: u64,
    pub key: String,
    pub title: String,
    pub run_id: String,
    pub stage: String,
    pub status: String,
    pub head_sha: Option<String>,
    pub pr_url: Option<String>,
    pub failure: Option<String>,
    pub updated_at: String,
    /// Set while the issue is parked: when to pick it up again, in seconds
    /// since 1970.
    pub wake_at: Option<u64>,
}

/// A step that ran, as the database keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ran {
    pub step: String,
    /// Who answered, for an agent step: the provider, or the vendor.
    pub vendor: Option<String>,
    /// The model it was started with. Empty for steps from before this was kept.
    pub model: Option<String>,
    pub attempt: u32,
    pub verdict: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// 0 for the first pass.
    pub round: u32,
    /// Tokens the call used: all input, the cached part of it, and output.
    /// Empty for a step that is not an agent call, or that counted nothing.
    pub tokens: Option<(u64, u64, u64)>,
}

/// Reads the token counts of agent calls made before the database kept
/// them, from each call's own output file next to its reply.
fn backfill_tokens(db: &Connection) -> Result<()> {
    let calls: Vec<(i64, String)> = {
        let mut statement = db.prepare("SELECT id, output_path FROM steps WHERE vendor IS NOT NULL AND output_path IS NOT NULL")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, output) in calls {
        let Some(stem) = output.strip_suffix(".out.json") else { continue };
        let Some(found) = std::fs::read_to_string(format!("{stem}.stdout.log")).ok().and_then(|text| super::agent::tokens(&text)) else { continue };
        db.execute("UPDATE steps SET tokens_in = ?2, tokens_cached = ?3, tokens_out = ?4 WHERE id = ?1", params![id, found.input, found.cached, found.output])?;
    }
    Ok(())
}

pub struct Store {
    db: Connection,
    log: PathBuf,
}

impl Store {
    pub fn open(home: &Path) -> Result<Store> {
        std::fs::create_dir_all(home).with_context(|| format!("creating {}", home.display()))?;
        let db = Connection::open(home.join("state.db")).context("opening the state database")?;
        // Several runs write here at once. Wait for another writer, don't fail.
        db.busy_timeout(std::time::Duration::from_secs(30))?;
        db.execute_batch(SCHEMA)?;
        // Databases from before fix rounds have no `round` column.
        let has_round = db.prepare("SELECT 1 FROM pragma_table_info('steps') WHERE name = 'round'")?.exists([])?;
        if !has_round {
            db.execute_batch("ALTER TABLE steps ADD COLUMN round INTEGER NOT NULL DEFAULT 0")?;
        }
        // Databases from before the model was kept have no `model` column.
        let has_model = db.prepare("SELECT 1 FROM pragma_table_info('steps') WHERE name = 'model'")?.exists([])?;
        if !has_model {
            db.execute_batch("ALTER TABLE steps ADD COLUMN model TEXT")?;
        }
        // Databases from before tokens were kept have no token columns. The
        // calls made until then still have their output on disk, so their
        // counts are read from there once.
        let has_tokens = db.prepare("SELECT 1 FROM pragma_table_info('steps') WHERE name = 'tokens_in'")?.exists([])?;
        if !has_tokens {
            db.execute_batch("ALTER TABLE steps ADD COLUMN tokens_in INTEGER; ALTER TABLE steps ADD COLUMN tokens_cached INTEGER; ALTER TABLE steps ADD COLUMN tokens_out INTEGER")?;
            backfill_tokens(&db)?;
        }
        // Databases from before parking have no `wake_at` column.
        let has_wake = db.prepare("SELECT 1 FROM pragma_table_info('issues') WHERE name = 'wake_at'")?.exists([])?;
        if !has_wake {
            db.execute_batch("ALTER TABLE issues ADD COLUMN wake_at INTEGER")?;
        }
        Ok(Store { db, log: home.join("requests.jsonl") })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_issue(
        &self,
        repo: &str,
        number: u64,
        plan: &str,
        key: &str,
        title: &str,
        run_id: &str,
        branch: &str,
        worktree: &str,
        base_sha: &str,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO issues (repo, number, plan, key, title, run_id, stage, status, branch, worktree, base_sha, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'start', 'running', ?7, ?8, ?9, ?10)
             ON CONFLICT (repo, number) DO UPDATE SET
                 plan = excluded.plan, key = excluded.key, title = excluded.title, run_id = excluded.run_id,
                 stage = 'start', status = 'running', branch = excluded.branch, worktree = excluded.worktree,
                 base_sha = excluded.base_sha, head_sha = NULL, pr_number = NULL, pr_url = NULL, failure = NULL,
                 updated_at = excluded.updated_at",
            params![repo, number, plan, key, title, run_id, branch, worktree, base_sha, util::now()],
        )?;
        Ok(())
    }

    pub fn set_stage(&self, repo: &str, number: u64, stage: &str, status: &str, failure: Option<&str>) -> Result<()> {
        self.db.execute(
            "UPDATE issues SET stage = ?3, status = ?4, failure = ?5, updated_at = ?6, wake_at = NULL WHERE repo = ?1 AND number = ?2",
            params![repo, number, stage, status, failure, util::now()],
        )?;
        Ok(())
    }

    /// Marks an issue as parked at a step: nothing runs for it until
    /// `wake_at`, and whoever picks it up continues at that step.
    pub fn set_parked(&self, repo: &str, number: u64, stage: &str, wake_at: u64, reason: &str) -> Result<()> {
        self.db.execute(
            "UPDATE issues SET stage = ?3, status = 'waiting', failure = ?4, updated_at = ?5, wake_at = ?6 WHERE repo = ?1 AND number = ?2",
            params![repo, number, stage, reason, util::now(), wake_at],
        )?;
        Ok(())
    }

    pub fn set_head(&self, repo: &str, number: u64, sha: &str) -> Result<()> {
        self.db.execute(
            "UPDATE issues SET head_sha = ?3, updated_at = ?4 WHERE repo = ?1 AND number = ?2",
            params![repo, number, sha, util::now()],
        )?;
        Ok(())
    }

    pub fn set_pr(&self, repo: &str, number: u64, pr_number: u64, pr_url: &str) -> Result<()> {
        self.db.execute(
            "UPDATE issues SET pr_number = ?3, pr_url = ?4, updated_at = ?5 WHERE repo = ?1 AND number = ?2",
            params![repo, number, pr_number, pr_url, util::now()],
        )?;
        Ok(())
    }

    /// Writes the step to the database and appends it to the request log.
    pub fn record(&self, step: &StepRecord) -> Result<()> {
        self.db.execute(
            "INSERT INTO steps (repo, number, run_id, step, kind, module, vendor, attempt, sha, exit_code, timed_out,
                                verdict, duration_ms, input_path, output_path, session_id, finished_at, round, model,
                                tokens_in, tokens_cached, tokens_out)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                step.repo,
                step.issue,
                step.run_id,
                step.step,
                step.kind,
                step.module,
                step.vendor,
                step.attempt,
                step.sha,
                step.exit_code,
                step.timed_out,
                step.verdict,
                step.duration_ms,
                step.input,
                step.output,
                step.session_id,
                step.ts,
                step.round,
                step.model,
                step.tokens_in,
                step.tokens_cached,
                step.tokens_out
            ],
        )?;
        let mut file = OpenOptions::new().create(true).append(true).open(&self.log)?;
        writeln!(file, "{}", serde_json::to_string(step)?)?;
        Ok(())
    }

    /// The row of one issue, when it has been run before.
    pub fn issue(&self, repo: &str, number: u64) -> Result<Option<IssueRow>> {
        Ok(self.issues()?.into_iter().find(|row| row.repo == repo && row.number == number))
    }

    pub fn issues(&self) -> Result<Vec<IssueRow>> {
        let mut statement = self.db.prepare(
            "SELECT repo, number, key, title, run_id, stage, status, head_sha, pr_url, failure, updated_at, wake_at
             FROM issues ORDER BY updated_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(IssueRow {
                repo: row.get(0)?,
                number: row.get(1)?,
                key: row.get(2)?,
                title: row.get(3)?,
                run_id: row.get(4)?,
                stage: row.get(5)?,
                status: row.get(6)?,
                head_sha: row.get(7)?,
                pr_url: row.get(8)?,
                failure: row.get(9)?,
                updated_at: row.get(10)?,
                wake_at: row.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Steps of one run, oldest first.
    pub fn steps(&self, repo: &str, number: u64, run_id: &str) -> Result<Vec<Ran>> {
        let mut statement = self.db.prepare(
            "SELECT step, vendor, model, attempt, verdict, exit_code, duration_ms, round, tokens_in, tokens_cached, tokens_out FROM steps
             WHERE repo = ?1 AND number = ?2 AND run_id = ?3 ORDER BY id",
        )?;
        let rows = statement.query_map(params![repo, number, run_id], |row| {
            Ok(Ran {
                step: row.get(0)?,
                vendor: row.get(1)?,
                model: row.get(2)?,
                attempt: row.get(3)?,
                verdict: row.get(4)?,
                exit_code: row.get(5)?,
                duration_ms: row.get(6)?,
                round: row.get(7)?,
                tokens: match (row.get::<_, Option<u64>>(8)?, row.get::<_, Option<u64>>(9)?, row.get::<_, Option<u64>>(10)?) {
                    (Some(input), cached, Some(output)) => Some((input, cached.unwrap_or(0), output)),
                    _ => None,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_a_reopen() {
        let home = std::env::temp_dir().join(format!("factory-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        {
            let store = Store::open(&home).unwrap();
            store.start_issue("o/r", 4, "todo-v1", "shell", "Shell", "run1", "b", "/w", "abc").unwrap();
            store.set_stage("o/r", 4, "verify", "failed", Some("exit 65")).unwrap();
            store
                .record(&StepRecord {
                    ts: util::now(),
                    repo: "o/r".into(),
                    issue: 4,
                    run_id: "run1".into(),
                    step: "verify".into(),
                    kind: "check".into(),
                    module: "check/verify".into(),
                    vendor: None,
                    model: None,
                    subscription: None,
                    round: 2,
                    attempt: 1,
                    sha: Some("def".into()),
                    exit_code: Some(65),
                    timed_out: false,
                    verdict: "fail".into(),
                    duration_ms: 1200,
                    input: None,
                    output: None,
                    stdout: None,
                    stderr: None,
                    session_id: None,
                    tokens_in: None,
                    tokens_cached: None,
                    tokens_out: None,
                })
                .unwrap();
        }
        let store = Store::open(&home).unwrap();
        let issues = store.issues().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!((issues[0].stage.as_str(), issues[0].status.as_str()), ("verify", "failed"));
        assert_eq!(issues[0].failure.as_deref(), Some("exit 65"));
        let steps = store.steps("o/r", 4, "run1").unwrap();
        assert_eq!((steps[0].verdict.as_str(), steps[0].round, steps[0].model.as_deref()), ("fail", 2, None));
        let log = std::fs::read_to_string(home.join("requests.jsonl")).unwrap();
        assert!(log.contains("\"module\":\"check/verify\""));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_older_database_gains_the_round_column() {
        let home = std::env::temp_dir().join(format!("factory-state-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        {
            let db = Connection::open(home.join("state.db")).unwrap();
            let older = SCHEMA.replace("    round        INTEGER NOT NULL DEFAULT 0,\n", "").replace("    model        TEXT,\n", "").replace("    tokens_in    INTEGER,\n    tokens_cached INTEGER,\n    tokens_out   INTEGER,\n", "");
        assert!(!older.contains("tokens_in"));
        db.execute_batch(&older).unwrap();
        // An agent call from then, whose output is still on disk.
        std::fs::write(home.join("03-develop-1.stdout.log"), "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":500,\"cached_input_tokens\":300,\"output_tokens\":20}}\n").unwrap();
        db.execute(
            "INSERT INTO steps (repo, number, run_id, step, kind, module, vendor, attempt, timed_out, verdict, duration_ms, output_path, finished_at)
             VALUES ('o/r', 4, 'run0', 'review', 'agent', 'agent/review', 'codex', 1, 0, 'approve', 5, ?1, 't')",
            params![home.join("03-develop-1.out.json").display().to_string()],
        )
        .unwrap();
            db.execute_batch(
                "INSERT INTO steps (repo, number, run_id, step, kind, module, attempt, timed_out, verdict, duration_ms, finished_at)
                 VALUES ('o/r', 4, 'run0', 'develop', 'agent', 'agent/dev', 1, 0, 'replied', 5, 't')",
            )
            .unwrap();
        }
        let store = Store::open(&home).unwrap();
        let steps = store.steps("o/r", 4, "run0").unwrap();
        let develop = steps.iter().find(|ran| ran.step == "develop").unwrap();
        assert_eq!((develop.round, develop.model.as_deref()), (0, None));
        // The earlier call's tokens were read from its output file. A step
        // with no such file has none.
        let review = steps.iter().find(|ran| ran.step == "review").unwrap();
        assert_eq!((develop.tokens, review.tokens), (None, Some((500, 300, 20))));
        // Opening again must not try to add the column twice.
        drop(store);
        Store::open(&home).unwrap();
        let _ = std::fs::remove_dir_all(&home);
    }
}
