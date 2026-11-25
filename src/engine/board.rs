//! What a running scheduler is doing, written where other processes can
//! read it: `<home>/schedulers/<pid>.json`.
//!
//! The scheduler decides in its own memory who gets a place. `factory serve`
//! is another process, so the scheduler writes down what it decided each
//! time that changes. The file describes. It decides nothing: a file whose
//! process is gone is ignored, and removed by the next scheduler.

use super::util;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub pid: u32,
    /// Seconds since 1970.
    pub started_at: u64,
    pub updated_at: u64,
    /// How many issues may run at once, over all its projects.
    pub places: usize,
    pub in_order: bool,
    pub watch: bool,
    /// Why no further issue is started although a place is free.
    pub held_back: Option<String>,
    pub projects: Vec<ProjectBoard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectBoard {
    /// `owner/name`.
    pub repo: String,
    pub label: String,
    pub max_parallel: Option<usize>,
    /// Issues that hold a place.
    pub running: Vec<Entry>,
    /// Labelled issues that have not started, each with what it waits on.
    pub waiting: Vec<Entry>,
    /// Issues set aside until somebody has quota again. They hold no place.
    pub parked: Vec<Entry>,
    pub merged: Vec<u64>,
    pub opened: Vec<u64>,
    pub failed: Vec<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub number: u64,
    pub key: String,
    pub title: String,
    /// Running: when it got its place.
    pub since: Option<u64>,
    /// Parked: when it is picked up again.
    pub until: Option<u64>,
    /// Waiting: what for.
    pub waits_on: Option<String>,
}

fn dir(home: &Path) -> PathBuf {
    home.join("schedulers")
}

fn file(home: &Path, pid: u32) -> PathBuf {
    dir(home).join(format!("{pid}.json"))
}

/// Writes the board whole, so a reader never sees half of one.
pub fn write(home: &Path, board: &Board) -> Result<()> {
    let dir = dir(home);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let partial = dir.join(format!("{}.json.partial", board.pid));
    std::fs::write(&partial, serde_json::to_string_pretty(board)?).with_context(|| format!("writing {}", partial.display()))?;
    std::fs::rename(&partial, file(home, board.pid)).with_context(|| format!("writing {}", file(home, board.pid).display()))
}

pub fn remove(home: &Path, pid: u32) {
    let _ = std::fs::remove_file(file(home, pid));
}

fn all(home: &Path) -> Vec<(PathBuf, Option<Board>)> {
    std::fs::read_dir(dir(home))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .map(|path| {
            let board = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok());
            (path, board)
        })
        .collect()
}

/// The boards of the schedulers that are running now, the oldest first.
pub fn live(home: &Path) -> Vec<Board> {
    let mut boards: Vec<Board> = all(home).into_iter().filter_map(|(_, board)| board).filter(|board| util::alive(board.pid)).collect();
    boards.sort_by_key(|board| (board.started_at, board.pid));
    boards
}

/// Removes what schedulers that died left behind.
pub fn sweep(home: &Path) {
    for (path, board) in all(home) {
        if !board.is_some_and(|board| util::alive(board.pid)) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(pid: u32, started_at: u64) -> Board {
        let entry = Entry { number: 7, key: "a".into(), title: "Add it".into(), since: Some(started_at), ..Entry::default() };
        let project = ProjectBoard { repo: "o/r".into(), label: "today".into(), max_parallel: None, running: vec![entry], waiting: Vec::new(), parked: Vec::new(), merged: vec![3], opened: Vec::new(), failed: Vec::new() };
        Board { pid, started_at, updated_at: started_at, places: 3, in_order: false, watch: true, held_back: None, projects: vec![project] }
    }

    #[test]
    fn only_a_scheduler_that_still_runs_has_a_board() {
        let home = std::env::temp_dir().join(format!("factory-board-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        assert!(live(&home).is_empty());
        let mut ended = std::process::Command::new("true").spawn().unwrap();
        let gone = ended.id();
        ended.wait().unwrap();
        write(&home, &board(std::process::id(), 200)).unwrap();
        write(&home, &board(gone, 100)).unwrap();
        std::fs::write(dir(&home).join("77.json"), "not a board").unwrap();
        // The one whose process is gone is not shown, and neither is a file
        // that can't be read.
        let boards = live(&home);
        assert_eq!(boards, vec![board(std::process::id(), 200)]);
        assert_eq!(boards[0].projects[0].running[0].number, 7);
        // Written again, it is replaced, not added to.
        let mut later = board(std::process::id(), 200);
        later.updated_at = 260;
        later.projects[0].running.clear();
        write(&home, &later).unwrap();
        assert_eq!(live(&home), vec![later]);
        // The next scheduler clears away what the dead ones left.
        sweep(&home);
        assert_eq!(std::fs::read_dir(dir(&home)).unwrap().count(), 1);
        remove(&home, std::process::id());
        assert!(live(&home).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }
}
