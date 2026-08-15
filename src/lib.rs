//! `dev.mcpg.catalog.builtin` — config-driven catalog provider
//! plugin for MCPG.
//!
//! Operators declare per-tool catalog metadata in YAML; the plugin
//! matches against `tools/list` results at runtime, dropping
//! hidden tools and enriching the rest with MCP `_meta`
//! annotations.
//!
//! Pure offline lookup; no outbound network. Composes with
//! `policy_engine` for call-time enforcement (catalog filters at
//! presentation; policy enforces at dispatch).

mod config;

use mcpg_plugin_protocol::PluginManifest;
use mcpg_plugin_protocol::async_trait;
use mcpg_plugin_protocol::catalog::{
    CatalogEntry, CatalogMetadata, CatalogProvider, EnrichedToolDescriptor, trust_level_meets,
};
use mcpg_plugin_protocol::manifest::PluginClass;
use mcpg_plugin_protocol::types::PluginContext;

pub use config::{CatalogConfig, ConfigError, Defaults, ToolEntry};

/// Plugin instance state.
pub struct BuiltinCatalog {
    manifest: PluginManifest,
    config: CatalogConfig,
}

impl BuiltinCatalog {
    /// Construct a new instance from a parsed config + manifest.
    /// The manifest is the runtime-typed mirror of `plugin.yaml`.
    #[must_use]
    pub fn new(config: CatalogConfig) -> Self {
        Self {
            manifest: default_manifest(),
            config,
        }
    }

    /// Construct from JSON config (FFI entry point).
    pub fn from_json(s: &str) -> Result<Self, ConfigError> {
        let config = CatalogConfig::from_json(s)?;
        Ok(Self::new(config))
    }

    /// Compute the effective `trust_required` for a tool, applying
    /// per-entry / global_defaults / strict-defaults rules in
    /// priority order.
    fn effective_trust_required(
        &self,
        incoming: Option<&str>,
        entry: Option<&ToolEntry>,
    ) -> Option<String> {
        if let Some(level) = incoming {
            return Some(level.to_owned());
        }
        if let Some(e) = entry
            && let Some(level) = &e.metadata.trust_required
        {
            return Some(level.clone());
        }
        if let Some(level) = &self.config.global_defaults.trust_required {
            return Some(level.clone());
        }
        if entry.is_none() && self.config.defaults.require_verified_for_unknown {
            return Some("verified".to_owned());
        }
        None
    }

    /// Build the catalog metadata this plugin contributes for a
    /// given tool, combining the per-tool entry and global_defaults
    /// per first-write-wins (per-tool wins over global_defaults).
    fn contribute(&self, entry: Option<&ToolEntry>) -> CatalogMetadata {
        let mut result = entry.map(|e| e.metadata.clone()).unwrap_or_default();
        // Fill from global_defaults using the same first-write-wins
        // logic as the chain merge (so global_defaults acts as
        // "what would I emit if I didn't know more about this
        // tool").
        result.merge_from(&self.config.global_defaults);
        result
    }
}

fn default_manifest() -> PluginManifest {
    PluginManifest {
        id: "dev.mcpg.catalog.builtin".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        name: "Builtin Catalog Provider".to_owned(),
        plugin_class: PluginClass::CatalogProvider,
        protocol_version: "1.0".to_owned(),
        license: None,
        required_capabilities: Vec::new(),
        tags: Vec::new(),
        provides: Vec::new(),
        provides_schemes: Vec::new(),
        module_path_prefix: ::std::module_path!()
            .split("::")
            .next()
            .unwrap_or("")
            .to_owned(),
        backend_profile: None,
    }
}

#[async_trait]
impl CatalogProvider for BuiltinCatalog {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn filter_and_enrich(
        &self,
        ctx: &PluginContext,
        in_progress: &[EnrichedToolDescriptor],
    ) -> Vec<EnrichedToolDescriptor> {
        let mut out = Vec::with_capacity(in_progress.len());
        let caller_trust = ctx.identity.trust_level.as_str();

        for tool in in_progress {
            let entry = self.config.tools.get(&tool.base.name);

            // Plugin-local hide flag: drop the tool entirely.
            if entry.is_some_and(|e| e.hide) {
                continue;
            }

            // hide_unknown strict mode: drop tools not in the
            // catalog.
            if entry.is_none() && self.config.defaults.hide_unknown {
                continue;
            }

            // Trust-level filtering.
            let incoming_trust = tool
                .catalog
                .as_ref()
                .and_then(|c| c.trust_required.as_deref());
            if let Some(required) = self.effective_trust_required(incoming_trust, entry)
                && !trust_level_meets(caller_trust, &required)
            {
                continue;
            }

            // Build the enrichment this plugin contributes, then
            // merge into the in-progress catalog (chain merge
            // rules in mcpg_plugin_protocol::catalog).
            let contribution = self.contribute(entry);
            let mut refined = tool.clone();
            if !contribution.is_empty() {
                let existing = refined.catalog.get_or_insert_with(CatalogMetadata::default);
                existing.merge_from(&contribution);
            }
            out.push(refined);
        }

        out
    }

    async fn describe(&self, tool_id: &str) -> Option<CatalogEntry> {
        self.config.tools.get(tool_id).map(|e| CatalogEntry {
            tool_id: tool_id.to_owned(),
            display_name: None,
            description: None,
            catalog: {
                let mut m = e.metadata.clone();
                m.merge_from(&self.config.global_defaults);
                m
            },
            access_summary: None,
            last_invoked_at: None,
            recent_invocations: None,
        })
    }

    async fn list_catalog(&self) -> Vec<CatalogEntry> {
        self.config
            .tools
            .iter()
            .map(|(tool_id, entry)| {
                let mut m = entry.metadata.clone();
                m.merge_from(&self.config.global_defaults);
                CatalogEntry {
                    tool_id: tool_id.clone(),
                    display_name: None,
                    description: None,
                    catalog: m,
                    access_summary: None,
                    last_invoked_at: None,
                    recent_invocations: None,
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// SDK FFI surface
// ---------------------------------------------------------------------------

impl mcpg_plugin_sdk::ffi::SyncCatalogProvider for BuiltinCatalog {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn filter_and_enrich(
        &self,
        ctx: &PluginContext,
        in_progress: &[EnrichedToolDescriptor],
    ) -> Vec<EnrichedToolDescriptor> {
        // Sync surface delegates by blocking on the async impl.
        // The plugin's logic is pure compute (no I/O), so this is
        // synchronous in practice — the async wrapper just keeps
        // the trait shape consistent.
        let async_self: &dyn CatalogProvider = self;
        futures_executor_block(async move { async_self.filter_and_enrich(ctx, in_progress).await })
    }

    fn describe(&self, tool_id: &str) -> Option<CatalogEntry> {
        let async_self: &dyn CatalogProvider = self;
        futures_executor_block(async move { async_self.describe(tool_id).await })
    }

    fn list_catalog(&self) -> Vec<CatalogEntry> {
        let async_self: &dyn CatalogProvider = self;
        futures_executor_block(async move { async_self.list_catalog().await })
    }
}

/// Block on a future inside the sync FFI shim. Acceptable because
/// every method on this plugin is pure compute — no I/O, no awaits
/// that actually yield. Sized for the catch_panic_to_empty_rstring
/// envelope the macro wraps around the FFI entry point.
fn futures_executor_block<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut fut = Box::pin(fut);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        // Pure-compute futures never yield. If a future ever
        // returns Pending here it's a bug — panic loudly so the
        // SDK macro's catch_panic_to_empty_rstring surfaces it
        // as fail-closed.
        Poll::Pending => panic!(
            "BuiltinCatalog sync shim hit Poll::Pending — \
             plugin is supposed to be pure compute"
        ),
    }
}

mcpg_plugin_sdk::declare_plugin! {
    plugin_id: "dev.mcpg.catalog.builtin",
    plugin_version: env!("CARGO_PKG_VERSION"),
    descriptor_yaml: include_str!("../plugin.yaml"),
    capabilities: &[],
    entities: [
        catalog_provider as entity {
            inner_name: "",
            plugin_type: BuiltinCatalog,
            factory: |cfg: &str, _host: ::mcpg_plugin_sdk::HostHandle| -> BuiltinCatalog {
                // Validation panics surface via the SDK's
                // catch_panic_to_null_handle as a "make returned null"
                // signal. Operators see the underlying panic in stderr.
                BuiltinCatalog::from_json(cfg)
                    .unwrap_or_else(|e| panic!("invalid catalog config: {e}"))
            },
        }
    ],
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mcpg_plugin_protocol::catalog::ProtocolToolDescriptor;
    use mcpg_plugin_protocol::types::PluginIdentity;
    use serde_json::json;

    fn config_with(yaml: &str) -> CatalogConfig {
        CatalogConfig::from_yaml(yaml).unwrap()
    }

    fn ctx(trust: &str) -> PluginContext {
        PluginContext {
            request_id: "r".into(),
            session_id: None,
            tool_name: "tools/list".into(),
            surface: "tool".into(),
            identity: PluginIdentity {
                kind: trust.into(),
                trust_level: trust.into(),
                subject_id: None,
                auth_provider: None,
                issuer: None,
                roles: vec![],
                groups: vec![],
                scopes: vec![],
                attributes: Default::default(),
            },
            transport: "http".into(),
        }
    }

    fn descriptor(name: &str) -> EnrichedToolDescriptor {
        EnrichedToolDescriptor::from_base(ProtocolToolDescriptor {
            name: name.into(),
            title: None,
            description: "x".into(),
            input_schema: json!({}),
            output_schema: None,
        })
    }

    #[tokio::test]
    async fn unknown_tools_pass_through_with_no_metadata() {
        let plugin = BuiltinCatalog::new(CatalogConfig::default());
        let input = vec![descriptor("orders.search")];
        let out = plugin.filter_and_enrich(&ctx("verified"), &input).await;
        assert_eq!(out.len(), 1);
        assert!(out[0].catalog.is_none());
    }

    #[tokio::test]
    async fn hide_flag_drops_tool() {
        let cfg = config_with(
            r#"
tools:
  orders.search:
    hide: true
"#,
        );
        let plugin = BuiltinCatalog::new(cfg);
        let input = vec![descriptor("orders.search"), descriptor("orders.list")];
        let out = plugin.filter_and_enrich(&ctx("verified"), &input).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].base.name, "orders.list");
    }

    #[tokio::test]
    async fn hide_unknown_drops_unmapped_tools() {
        let cfg = config_with(
            r#"
tools:
  orders.search:
    tags: ["read-only"]
defaults:
  hide_unknown: true
"#,
        );
        let plugin = BuiltinCatalog::new(cfg);
        let input = vec![descriptor("orders.search"), descriptor("internal.x")];
        let out = plugin.filter_and_enrich(&ctx("verified"), &input).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].base.name, "orders.search");
    }

    #[tokio::test]
    async fn trust_required_drops_low_trust_callers() {
        let cfg = config_with(
            r#"
tools:
  orders.search:
    trust_required: "verified"
"#,
        );
        let plugin = BuiltinCatalog::new(cfg);
        let input = vec![descriptor("orders.search")];
        let anon = plugin.filter_and_enrich(&ctx("anonymous"), &input).await;
        assert!(anon.is_empty());
        let verified = plugin.filter_and_enrich(&ctx("verified"), &input).await;
        assert_eq!(verified.len(), 1);
    }

    #[tokio::test]
    async fn enriches_with_metadata() {
        let cfg = config_with(
            r#"
tools:
  orders.search:
    tags: ["read-only", "orders"]
    owner: "platform-team"
    doc_url: "https://example.com/docs"
"#,
        );
        let plugin = BuiltinCatalog::new(cfg);
        let input = vec![descriptor("orders.search")];
        let out = plugin.filter_and_enrich(&ctx("verified"), &input).await;
        let catalog = out[0].catalog.as_ref().unwrap();
        assert_eq!(
            catalog.tags,
            vec!["read-only".to_owned(), "orders".to_owned()]
        );
        assert_eq!(catalog.owner.as_deref(), Some("platform-team"));
        assert_eq!(catalog.doc_url.as_deref(), Some("https://example.com/docs"));
    }

    #[tokio::test]
    async fn first_write_wins_for_chain_existing_metadata() {
        let cfg = config_with(
            r#"
tools:
  orders.search:
    owner: "team-builtin"
    tags: ["from-builtin"]
"#,
        );
        let plugin = BuiltinCatalog::new(cfg);
        let mut existing = descriptor("orders.search");
        existing.catalog = Some(CatalogMetadata {
            owner: Some("team-from-earlier".into()),
            tags: vec!["from-earlier".into()],
            ..CatalogMetadata::default()
        });
        let out = plugin
            .filter_and_enrich(&ctx("verified"), &[existing])
            .await;
        let catalog = out[0].catalog.as_ref().unwrap();
        // owner: first-write-wins → earlier provider wins.
        assert_eq!(catalog.owner.as_deref(), Some("team-from-earlier"));
        // tags: union.
        assert!(catalog.tags.contains(&"from-earlier".to_owned()));
        assert!(catalog.tags.contains(&"from-builtin".to_owned()));
    }

    #[tokio::test]
    async fn require_verified_for_unknown_drops_low_trust() {
        let cfg = config_with(
            r#"
defaults:
  require_verified_for_unknown: true
"#,
        );
        let plugin = BuiltinCatalog::new(cfg);
        let input = vec![descriptor("orders.unknown")];
        let anon = plugin.filter_and_enrich(&ctx("anonymous"), &input).await;
        assert!(anon.is_empty());
        let verified = plugin.filter_and_enrich(&ctx("verified"), &input).await;
        assert_eq!(verified.len(), 1);
    }

    #[tokio::test]
    async fn describe_returns_metadata_merged_with_global_defaults() {
        let cfg = config_with(
            r#"
tools:
  orders.search:
    tags: ["read-only"]
global_defaults:
  owner: "platform-team"
  maturity: "stable"
"#,
        );
        let plugin = BuiltinCatalog::new(cfg);
        let entry = plugin.describe("orders.search").await.unwrap();
        assert_eq!(entry.catalog.tags, vec!["read-only".to_owned()]);
        assert_eq!(entry.catalog.owner.as_deref(), Some("platform-team"));
        assert_eq!(entry.catalog.maturity.as_deref(), Some("stable"));
    }
}
