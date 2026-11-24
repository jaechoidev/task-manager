//! The single entry point for YAML parsing. serde_yaml is archived, and
//! serde_yaml_ng is its maintained continuation. Swapping crates means
//! changing this one function.

use serde::de::DeserializeOwned;

pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_yaml_ng::from_str::<T>(text).map_err(|e| e.to_string())
}
