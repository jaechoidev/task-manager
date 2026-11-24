//! The single entry point for YAML parsing. serde_yaml is archived, and
//! serde_yaml_ng is its maintained continuation. Swapping crates means
//! changing this one function.

use serde::de::DeserializeOwned;

pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_yaml_ng::from_str::<T>(text).map_err(|e| e.to_string())
}

/// Every document of a multi-document file, as JSON values.
pub fn documents(text: &str) -> Result<Vec<serde_json::Value>, String> {
    use serde::Deserialize;
    serde_yaml_ng::Deserializer::from_str(text)
        .map(|document| serde_json::Value::deserialize(document).map_err(|e| e.to_string()))
        .collect()
}
