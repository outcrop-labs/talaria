- **Version history speaks Rust — and the port gained a byte-identical YAML
  emitter.** `/api/history` serves both stores (`internal_versions` snapshots
  and `agent_versions`) through the live items' own read models applied
  backwards, every miss and every error reading 403 — fail-closed, or
  history is a permission bypass. `kind=config` serves `stringifyYaml`
  bytes, so the slice dragged `api/src/yaml_string.rs` across: a source port
  of the npm `yaml` package's stringify pinned by 145 fixtures the REAL
  package generates.
