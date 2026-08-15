//! Operator-supplied configuration schema for `dev.mcpg.catalog.builtin`.
//!
//! Validation runs at boot via `CatalogConfig::from_json` /
//! `CatalogConfig::from_yaml` — invalid configs panic ("malformed
//! config aborts boot rather than silently degrading").

use std::collections::BTreeMap;

use mcpg_plugin_protocol::catalog::CatalogMetadata;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Top-level config for the builtin catalog plugin.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogConfig {
    /// Per-tool catalog entries. Key is the tool id (matches the
    /// binding's `name` field).
    #[serde(default)]
    pub tools: BTreeMap<String, ToolEntry>,

    /// Defaults that apply to tools not in the per-tool map.
    #[serde(default)]
    pub defaults: Defaults,

    /// Catalog-wide defaults applied when a per-tool entry omits
    /// the field (per-tool entry wins; this is the fallback before
    /// the chain's first-write-wins kicks in).
    #[serde(default)]
    pub global_defaults: CatalogMetadata,
}

/// Per-tool catalog entry. `metadata` carries the user-facing
/// fields surfaced as MCP `_meta` annotations; `hide` is plugin-
/// local — it drops the tool from the chain output (sticky OR per
/// the chain merge rules).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolEntry {
    #[serde(flatten)]
    pub metadata: CatalogMetadata,
    /// When `true`, the plugin drops this tool from the chain
    /// output. Useful for marking deprecated / internal-only tools
    /// without removing the binding plugin.
    #[serde(default)]
    pub hide: bool,
}

/// Strictness knobs applied when a tool is NOT in the per-tool map.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Defaults {
    /// When `true`, tools not in the catalog config are dropped.
    /// Operators graduating from "open catalog" to "explicit
    /// allowlist" flip this. Default `false` — unknown tools pass
    /// through with no enrichment.
    #[serde(default)]
    pub hide_unknown: bool,

    /// When `true`, tools without an explicit `trust_required`
    /// require at least `"verified"`. Default `false`.
    #[serde(default)]
    pub require_verified_for_unknown: bool,
}

/// Validation error surface.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid catalog config JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("invalid catalog config YAML: {0}")]
    InvalidYaml(#[from] serde_yaml::Error),
    #[error("tool id is empty in catalog config")]
    EmptyToolId,
    #[error(
        "tool '{tool_id}': trust_required '{value}' is not one of \
         {{verified, header_asserted, anonymous}}"
    )]
    InvalidTrustLevel { tool_id: String, value: String },
    #[error(
        "tool '{tool_id}': maturity '{value}' is not one of \
         {{experimental, beta, stable, deprecated}}"
    )]
    InvalidMaturity { tool_id: String, value: String },
    #[error("tool '{tool_id}': tag '{tag}' must match [a-z0-9-]+")]
    InvalidTag { tool_id: String, tag: String },
}

impl CatalogConfig {
    /// Parse + validate from a JSON string. Used by the FFI
    /// `make` entry point which receives JSON-encoded config.
    pub fn from_json(s: &str) -> Result<Self, ConfigError> {
        let cfg: Self = serde_json::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Parse + validate from a YAML string. Used by tests + by
    /// operators who want to round-trip YAML through this type.
    pub fn from_yaml(s: &str) -> Result<Self, ConfigError> {
        let cfg: Self = serde_yaml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Run config validation rules.
    fn validate(&self) -> Result<(), ConfigError> {
        for (tool_id, entry) in &self.tools {
            if tool_id.is_empty() {
                return Err(ConfigError::EmptyToolId);
            }
            if let Some(level) = &entry.metadata.trust_required {
                validate_trust_level(tool_id, level)?;
            }
            if let Some(maturity) = &entry.metadata.maturity {
                validate_maturity(tool_id, maturity)?;
            }
            for tag in &entry.metadata.tags {
                validate_tag(tool_id, tag)?;
            }
        }
        if let Some(level) = &self.global_defaults.trust_required {
            validate_trust_level("<global_defaults>", level)?;
        }
        if let Some(maturity) = &self.global_defaults.maturity {
            validate_maturity("<global_defaults>", maturity)?;
        }
        for tag in &self.global_defaults.tags {
            validate_tag("<global_defaults>", tag)?;
        }
        Ok(())
    }
}

fn validate_trust_level(tool_id: &str, value: &str) -> Result<(), ConfigError> {
    match value {
        "verified" | "header_asserted" | "anonymous" => Ok(()),
        _ => Err(ConfigError::InvalidTrustLevel {
            tool_id: tool_id.to_owned(),
            value: value.to_owned(),
        }),
    }
}

fn validate_maturity(tool_id: &str, value: &str) -> Result<(), ConfigError> {
    match value {
        "experimental" | "beta" | "stable" | "deprecated" => Ok(()),
        _ => Err(ConfigError::InvalidMaturity {
            tool_id: tool_id.to_owned(),
            value: value.to_owned(),
        }),
    }
}

fn validate_tag(tool_id: &str, tag: &str) -> Result<(), ConfigError> {
    if tag.is_empty()
        || !tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ConfigError::InvalidTag {
            tool_id: tool_id.to_owned(),
            tag: tag.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_yaml() {
        let yaml = r#"
tools:
  orders.search:
    tags: ["read-only"]
    owner: "platform-team"
"#;
        let cfg = CatalogConfig::from_yaml(yaml).unwrap();
        let entry = cfg.tools.get("orders.search").unwrap();
        assert_eq!(entry.metadata.tags, vec!["read-only".to_owned()]);
        assert_eq!(entry.metadata.owner.as_deref(), Some("platform-team"));
    }

    #[test]
    fn rejects_invalid_trust_level() {
        let yaml = r#"
tools:
  orders.search:
    trust_required: "alien"
"#;
        let err = CatalogConfig::from_yaml(yaml).unwrap_err();
        match err {
            ConfigError::InvalidTrustLevel { value, .. } => assert_eq!(value, "alien"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_maturity() {
        let yaml = r#"
tools:
  orders.search:
    maturity: "ancient"
"#;
        let err = CatalogConfig::from_yaml(yaml).unwrap_err();
        match err {
            ConfigError::InvalidMaturity { value, .. } => assert_eq!(value, "ancient"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_tag() {
        let yaml = r#"
tools:
  orders.search:
    tags: ["Bad Tag"]
"#;
        let err = CatalogConfig::from_yaml(yaml).unwrap_err();
        match err {
            ConfigError::InvalidTag { tag, .. } => assert_eq!(tag, "Bad Tag"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_hide_flag() {
        let yaml = r#"
tools:
  internal.health:
    hide: true
"#;
        let cfg = CatalogConfig::from_yaml(yaml).unwrap();
        assert!(cfg.tools.get("internal.health").unwrap().hide);
    }

    #[test]
    fn parses_defaults() {
        let yaml = r#"
defaults:
  hide_unknown: true
  require_verified_for_unknown: true
"#;
        let cfg = CatalogConfig::from_yaml(yaml).unwrap();
        assert!(cfg.defaults.hide_unknown);
        assert!(cfg.defaults.require_verified_for_unknown);
    }

    #[test]
    fn parses_global_defaults() {
        let yaml = r#"
global_defaults:
  owner: "platform-team"
  maturity: "stable"
"#;
        let cfg = CatalogConfig::from_yaml(yaml).unwrap();
        assert_eq!(cfg.global_defaults.owner.as_deref(), Some("platform-team"));
        assert_eq!(cfg.global_defaults.maturity.as_deref(), Some("stable"));
    }
}
