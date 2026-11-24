//! Settings of this machine, in `<home>/config.yaml`. They are about the
//! machine, not about a project, so they are not in any repository.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Folder for handoff documents. `~` is the home directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoffs: Option<String>,
    /// How many of each resource may be in use at once, across every project
    /// on this machine. A resource a module names in `locks`, such as
    /// `simulator`, or a vendor, `codex` or `claude`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pools: BTreeMap<String, u32>,
}

impl Config {
    fn file(home: &Path) -> PathBuf {
        home.join("config.yaml")
    }

    /// A missing file is an empty config. A file that can't be read as one
    /// is an error: guessing a machine's limits would be worse.
    pub fn load(home: &Path) -> Result<Config> {
        let file = Config::file(home);
        match std::fs::read_to_string(&file) {
            Ok(text) if text.trim().is_empty() => Ok(Config::default()),
            Ok(text) => crate::yaml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
        }
    }

    pub fn save(&self, home: &Path) -> Result<()> {
        std::fs::create_dir_all(home)?;
        let text = crate::yaml::to_string(self).map_err(|e| anyhow::anyhow!(e))?;
        let header = "# Settings of this machine, written by `factory capacity --write`.\n# pools: how many of each resource may be in use at once, across every project.\n";
        std::fs::write(Config::file(home), format!("{header}{text}")).with_context(|| format!("writing {}", Config::file(home).display()))
    }

    /// The size of the pool for a resource a module locks. One, unless this
    /// machine's config says more: one at a time is always safe.
    pub fn pool(&self, name: &str) -> u32 {
        self.pools.get(name).copied().unwrap_or(1).max(1)
    }

    /// The cap on calls to a vendor at once. None, unless the config sets one.
    pub fn vendor_pool(&self, vendor: &str) -> Option<u32> {
        self.pools.get(vendor).copied().map(|size| size.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_default_to_one_and_survive_a_round_trip() {
        let home = std::env::temp_dir().join(format!("factory-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        // No file: one simulator at a time, and no cap on a vendor.
        let empty = Config::load(&home).unwrap();
        assert_eq!((empty.pool("simulator"), empty.vendor_pool("codex")), (1, None));

        let mut config = Config { handoffs: Some("~/Notes/Handoffs".into()), pools: BTreeMap::new() };
        config.pools.insert("simulator".into(), 3);
        config.pools.insert("codex".into(), 2);
        config.pools.insert("broken".into(), 0);
        config.save(&home).unwrap();
        let read = Config::load(&home).unwrap();
        assert_eq!(read, config);
        assert_eq!((read.pool("simulator"), read.vendor_pool("codex"), read.vendor_pool("claude")), (3, Some(2), None));
        // A size of zero would stop everything, so it counts as one.
        assert_eq!(read.pool("broken"), 1);
        assert!(std::fs::read_to_string(home.join("config.yaml")).unwrap().starts_with("# Settings of this machine"));

        // A file that is not a config is an error, not a silent default.
        std::fs::write(home.join("config.yaml"), "pools: [1, 2]\n").unwrap();
        assert!(Config::load(&home).is_err());
        std::fs::write(home.join("config.yaml"), "pool:\n  simulator: 3\n").unwrap();
        assert!(Config::load(&home).unwrap_err().to_string().contains("unknown field"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
