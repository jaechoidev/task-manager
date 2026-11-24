//! Locks that hold across processes, so several `factory run` processes can
//! work at once without stepping on each other.
//!
//! Each lock is a file under `<home>/locks/`, locked with the operating
//! system's advisory file lock. The system releases it when the process
//! exits, however it exits, so a crash never leaves a lock behind.

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

/// A held lock. Dropping it releases the lock.
pub struct Held {
    _file: File,
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
        Ok(()) => Ok(Some(Held { _file: file })),
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
    Ok(Held { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;

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
