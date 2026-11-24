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

/// Takes one slot of a pool of `size`, waiting while every slot is in use.
/// Returns the slot's number, from 1, with its lock. Whoever holds slot 2 of
/// `simulator` is the only one using the second simulator.
pub fn acquire_slot(home: &Path, name: &str, size: u32, on_wait: &dyn Fn()) -> Result<(Held, u32)> {
    let size = size.max(1);
    let mut waited = false;
    loop {
        for slot in 1..=size {
            if let Some(held) = try_acquire(home, &format!("{name}-{slot}"))? {
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

    #[test]
    fn a_pool_hands_out_each_slot_once() {
        let home = std::env::temp_dir().join(format!("factory-lock-pool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let (first, a) = acquire_slot(&home, "simulator", 2, &|| panic!("a free pool should not wait")).unwrap();
        let (second, b) = acquire_slot(&home, "simulator", 2, &|| panic!("one slot is still free")).unwrap();
        assert_eq!((a, b), (1, 2));
        // Both are taken. A third taker waits, and gets whichever is released.
        let waited = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let third = scope.spawn(|| acquire_slot(&home, "simulator", 2, &|| waited.store(true, std::sync::atomic::Ordering::SeqCst)).map(|(_, slot)| slot).unwrap());
            std::thread::sleep(std::time::Duration::from_millis(500));
            drop(second);
            assert_eq!(third.join().unwrap(), 2);
        });
        assert!(waited.load(std::sync::atomic::Ordering::SeqCst));
        // Another pool is not affected, and a size of zero still gives one slot.
        assert_eq!(acquire_slot(&home, "codex", 0, &|| {}).unwrap().1, 1);
        drop(first);
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
