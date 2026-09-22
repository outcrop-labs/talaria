- **Enable compiles and loads the app first.** Manage → Apps runs a rebuild +
  module probe before enable (UI surfaces, `server.ts` `fetch`, `mcp.ts` `tools`);
  a failed compile or a module that will not load is refused with that error.
  An already-enabled app whose `current.json` is `failed` is disabled by the
  boot reconciler (api + UI), not by GET `/api/admin/apps`. Verified:
  `enable_block_reason` names a failed compile and lets `ready` through;
  `moduleHasFetch` / `moduleHasMcpTools` / `moduleHasSurfaces` accept the SDK
  shapes and reject empty defaults; `clientArtifactOk` requires an ESM export.
