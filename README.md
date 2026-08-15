# Builtin Catalog Provider — `dev.mcpg.catalog.builtin`

> class `catalog_provider` · `native` · package `mcpg-plugin-catalog-builtin` · artifact `libmcpg_plugin_catalog_builtin.so` · Apache-2.0

A `catalog_provider` plugin for the MCPG gateway that turns a block of YAML into
a governed tool catalog. Operators describe each tool once — owner, tags,
documentation URL, sample arguments, maturity, approval and trust requirements —
and the plugin applies that description to every `tools/list` response, hiding
the tools a caller should not see and attaching the rest as MCP `_meta`
annotations so clients can render owners, badges, and docs links. It is a pure
offline lookup with no outbound network. Reach for it when you want a curated,
attributed tool catalogue without standing up a service-catalogue backend.

## What it does
- Walks the `tools/list` result and annotates each surviving tool under
  `_meta.mcpg.catalog` with the metadata configured for it.
- Drops any tool marked `hide: true`, and — with `defaults.hide_unknown` — every
  tool absent from the config, turning the catalogue into an allowlist.
- Filters by caller trust: a tool whose effective `trust_required` exceeds the
  caller's trust level is removed from the listing.
- Fills gaps from `global_defaults`, so catalogue-wide facts (owning team,
  default maturity) are written once.
- Participates in the provider chain with first-write-wins semantics for scalar
  fields and union semantics for `tags`, so an earlier provider stays
  authoritative and this plugin only fills what is still unset.
- Answers catalogue lookups (`describe`, `list_catalog`) from the same config.
- Validates `trust_required`, `maturity`, and tag syntax when the plugin is
  constructed; an invalid catalogue aborts the gateway's boot rather than
  quietly serving a broken listing.
- Declares no required capabilities — it performs no I/O.

Filtering here is a **presentation** control: a hidden tool disappears from the
listing but the binding still exists. Pair it with a `policy_engine` or
`tool_gate` plugin when the tool must also be refused at dispatch.

## Configuration
Loaded from the flat top-level `plugins:` list with `class: catalog_provider`.
Providers form a chain in `plugins[]` order, and earlier entries win on scalar
fields, so place this one first when its metadata should be authoritative.

```yaml
plugins:
  - id: dev.mcpg.catalog.builtin
    class: catalog_provider
    kind: native
    source:
      path: ./plugins/libmcpg_plugin_catalog_builtin.so
      # or, platform-agnostic:
      # oci: ghcr.io/mcpg-dev/source-code/plugins/catalog-builtin:protocol-1
    config:
      tools:
        orders.search:
          owner: platform-team <platform@example.com>
          tags: [read-only, orders]
          doc_url: https://docs.example.com/tools/orders-search
          maturity: stable
          trust_required: verified
          sample_arguments:
            query: "status:open"
        orders.refund:
          owner: payments-team
          maturity: beta
          requires_approval: true
          attributes:
            backstage_ref: component:default/orders
        internal.health:
          hide: true
      defaults:
        hide_unknown: false
        require_verified_for_unknown: false
      global_defaults:
        owner: platform-team
        maturity: stable
```

| Field | Type | Default | Description |
|---|---|---|---|
| `tools` | map<tool name, entry> | `{}` | Per-tool catalogue entries. The key matches the binding's `name`. |
| `tools.<name>.hide` | bool | `false` | Drop this tool from the listing entirely. |
| `defaults.hide_unknown` | bool | `false` | Drop every tool with no `tools` entry (allowlist mode). |
| `defaults.require_verified_for_unknown` | bool | `false` | Require `verified` trust for tools with no `tools` entry. |
| `global_defaults` | metadata | `{}` | Metadata applied wherever a per-tool entry leaves a field unset. |

Each per-tool entry — and `global_defaults` — carries the same metadata fields:

| Field | Type | Description |
|---|---|---|
| `tags` | string[] | Free-form labels. Each must match `[a-z0-9-]+`. Unioned across the chain. |
| `owner` | string | Owning team or contact. |
| `doc_url` | string | External documentation URL. |
| `sample_arguments` | object | A representative invocation, useful to clients building few-shot examples. |
| `trust_required` | string | One of `verified`, `header_asserted`, `anonymous`. |
| `requires_approval` | bool | Surfaced as an annotation; enforcement is a `tool_gate` plugin's job. |
| `maturity` | string | One of `experimental`, `beta`, `stable`, `deprecated`. |
| `attributes` | map<string,string> | Catalogue-source-specific extras (for example a service-catalogue entity ref). |

Values outside the accepted sets for `trust_required` and `maturity`, an empty
tool name, or a tag with characters outside `[a-z0-9-]` are rejected when the
plugin is constructed, naming the offending tool.

## Security
Trust filtering resolves in a fixed order: metadata already attached by an
earlier provider in the chain, then the tool's own `trust_required`, then
`global_defaults.trust_required`, then — only for a tool with no `tools` entry —
`verified` when `defaults.require_verified_for_unknown` is set. The gateway
records a catalogue-filtered audit event on every listing, naming the provider
that dropped each tool, so "did this caller see tool X?" is answerable from the
audit lane.

## Build
`cdylib-export` is enabled by default, so the plain build already produces the
loadable artifact. Disable the default features when linking this crate as an
rlib path dependency alongside other plugins, so the workspace build does not
link two `mcpg_plugin_register` exports.

```bash
cargo build -p mcpg-plugin-catalog-builtin --features cdylib-export --release   # → target/release/libmcpg_plugin_catalog_builtin.so
```

## Sign & load (production)
Sign the artifact, pin/verify via the entry's `signature:` block, and honour
revocations. See <https://mcpg.dev/docs/security/plugin-security>.

## See also
- <https://mcpg.dev/docs/plugins/plugins-and-protocol> — plugin classes, the ABI, and how the gateway loads them.
- <https://mcpg.dev/docs/reference/configuration> — the full gateway config schema, including `plugins[]`.
- <https://mcpg.dev/docs/plugins/plugin-catalogue> — the other plugins that ship in-tree.
