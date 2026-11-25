//! Locks that hold across processes, so several `factory run` processes can
//! work at once without stepping on each other.
//!
//! Each lock is a file under `<home>/locks/`, locked with the operating
//! system's advisory file lock. The system releases it when the process
//! exits, however it exits, so a crash never leaves a lock behind.

use super::util;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

/// A held lock. Dropping it releases the lock.
pub struct Held {
    _file: File,
    /// The note that says who holds it, when one was left.
    note: Option<PathBuf>,
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(note) = &self.note {
            let _ = std::fs::remove_file(note);
        }
    }
}

/// Who is asking for a slot, so that someone looking on can be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    /// `owner/name`.
    pub repo: String,
    pub issue: u64,
    /// The step, and for an agent call the model: `verify`, or `review, opus`.
    pub what: String,
}

/// The note next to a held slot: who has it, and since when. The lock is
/// what holds the slot. The note only describes it, and a note whose process
/// is gone is ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub pool: String,
    pub slot: u32,
    pub pid: u32,
    pub repo: String,
    pub issue: u64,
    pub what: String,
    /// Seconds since 1970.
    pub since: u64,
}

/// Every slot that is held now and says by whom, in the order of their names.
pub fn holders(home: &Path) -> Vec<Holder> {
    let mut found: Vec<Holder> = std::fs::read_dir(home.join("locks"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".holder"))
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str::<Holder>(&text).ok())
        .filter(|holder| util::alive(holder.pid))
        .collect();
    found.sort_by(|a, b| (&a.pool, a.slot).cmp(&(&b.pool, b.slot)));
    found
}

fn path(home: &Path, name: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    home.join("locks").join(format!("{safe}.lock"))
}

fn open(home: &Path, name: &str) -> Result<File> {
    let file = path(home, name);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    OpenOptions::new().create(true).truncate(false).write(true).open(&file).with_context(|| format!("opening {}", file.display()))
}

/// Takes the lock if nobody holds it. `None` means another process does.
pub fn try_acquire(home: &Path, name: &str) -> Result<Option<Held>> {
    let file = open(home, name)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Held { _file: file, note: None })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e).with_context(|| format!("locking {name}")),
    }
}

/// Takes the lock, waiting for as long as another process holds it.
/// `on_wait` is called once if there is a wait.
pub fn acquire(home: &Path, name: &str, on_wait: &dyn Fn()) -> Result<Held> {
    if let Some(held) = try_acquire(home, name)? {
        return Ok(held);
    }
    on_wait();
    let file = open(home, name)?;
    file.lock().with_context(|| format!("locking {name}"))?;
    Ok(Held { _file: file, note: None })
}

/// Takes one slot of a pool of `size`, waiting while every slot is in use.
/// Returns the slot's number, from 1, with its lock. Whoever holds slot 2 of
/// `simulator` is the only one using the second simulator.
///
/// With `who`, a note is left next to the slot for as long as it is held.
pub fn acquire_slot(home: &Path, name: &str, size: u32, who: Option<&Who>, on_wait: &dyn Fn()) -> Result<(Held, u32)> {
    let size = size.max(1);
    let mut waited = false;
    loop {
        for slot in 1..=size {
            if let Some(mut held) = try_acquire(home, &format!("{name}-{slot}"))? {
                if let Some(who) = who {
                    let holder = Holder { pool: name.to_string(), slot, pid: std::process::id(), repo: who.repo.clone(), issue: who.issue, what: who.what.clone(), since: util::now_secs() };
                    let note = path(home, &format!("{name}-{slot}")).with_extension("holder");
                    // A slot that can't say who has it is still a held slot.
                    if std::fs::write(&note, serde_json::to_string(&holder)?).is_ok() {
                        held.note = Some(note);
                    }
                }
                return Ok((held, slot));
            }
        }
        if !waited {
            on_wait();
            waited = true;
        }
        // The system has no way to wait on several files at once.
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The process id of a program that has already ended.
    fn ended_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn a_pool_hands_out_each_slot_once() {
        let home = std::env::temp_dir().join(format!("factory-lock-pool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let who = Who { repo: "o/r".into(), issue: 7, what: "verify".into() };
        let (first, a) = acquire_slot(&home, "simulator", 2, Some(&who), &|| panic!("a free pool should not wait")).unwrap();
        let (second, b) = acquire_slot(&home, "simulator", 2, None, &|| panic!("one slot is still free")).unwrap();
        assert_eq!((a, b), (1, 2));
        // The first says who has it. The second was taken without a word.
        let known = holders(&home);
        assert_eq!(known.len(), 1);
        assert_eq!((known[0].pool.as_str(), known[0].slot, known[0].repo.as_str(), known[0].issue, known[0].what.as_str()), ("simulator", 1, "o/r", 7, "verify"));
        assert_eq!(known[0].pid, std::process::id());
        // Both are taken. A third taker waits, and gets whichever is released.
        let waited = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let third = scope.spawn(|| acquire_slot(&home, "simulator", 2, None, &|| waited.store(true, std::sync::atomic::Ordering::SeqCst)).map(|(_, slot)| slot).unwrap());
            std::thread::sleep(std::time::Duration::from_millis(500));
            drop(second);
            assert_eq!(third.join().unwrap(), 2);
        });
        assert!(waited.load(std::sync::atomic::Ordering::SeqCst));
        // Another pool is not affected, and a size of zero still gives one slot.
        assert_eq!(acquire_slot(&home, "codex", 0, None, &|| {}).unwrap().1, 1);
        drop(first);
        // Letting go takes the note away, and a note of a process that is
        // gone counts for nothing.
        assert!(holders(&home).is_empty());
        let gone = Holder { pool: "simulator".into(), slot: 2, pid: ended_pid(), repo: "o/r".into(), issue: 8, what: "verify".into(), since: 1 };
        std::fs::write(home.join("locks/simulator-2.holder"), serde_json::to_string(&gone).unwrap()).unwrap();
        assert!(holders(&home).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_lock_is_exclusive_until_dropped() {
        let home = std::env::temp_dir().join(format!("factory-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let first = try_acquire(&home, "simulator").unwrap();
        assert!(first.is_some());
        // The same name can't be taken twice, even by this process.
        assert!(try_acquire(&home, "simulator").unwrap().is_none());
        // Another name is another lock, and odd characters make a safe file name.
        assert!(try_acquire(&home, "issue o/r#7").unwrap().is_some());
        assert!(home.join("locks/issue_o_r_7.lock").exists());
        drop(first);
        // Another test may be starting a child process at this instant, and
        // for a moment that child holds a copy of the file. Allow for it.
        let again = (0..50).find_map(|_| {
            let held = try_acquire(&home, "simulator").unwrap();
            if held.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            held
        });
        assert!(again.is_some());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn waiting_ends_when_the_holder_lets_go() {
        let home = std::env::temp_dir().join(format!("factory-lock-wait-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let held = try_acquire(&home, "git").unwrap().unwrap();
        let waited = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| acquire(&home, "git", &|| waited.store(true, std::sync::atomic::Ordering::SeqCst)).is_ok());
            std::thread::sleep(std::time::Duration::from_millis(150));
            drop(held);
            assert!(waiter.join().unwrap());
        });
        assert!(waited.load(std::sync::atomic::Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(&home);
    }
}
