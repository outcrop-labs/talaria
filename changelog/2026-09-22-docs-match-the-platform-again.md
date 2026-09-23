- **Docs match the platform again.** Three read-only audits diffed the doc set
  against the code; what they caught is fixed: `ui/README` no longer claims a
  React/TanStack Start stack or a violet-to-magenta Mercury accent (both
  pre-Svelte, pre-gold) and says bun where it printed npm, `CONTRIBUTING` and
  `HANDOFF` match it (typecheck is svelte-check, not `npx tsc`), `mcp/README`'s
  install and example config use bun, `MCP.md` names the rendered gateway URL
  for what it is (the app's own port, not the toolkit's standalone listener),
  `AGENT-NETWORKING` gains a dated update on what the preflight covers now
  (the app port, the `/api/mcp/gw` path agents actually dial, external DNS)
  and the chassis's pinned resolvers, `WORKBENCH` explains why the built-in
  browser depends on that DNS, and per-person timezones (brief open time,
  digest arrival) are documented for the first time. Historical entries —
  dated plans, the changelog itself, the phase log — were left as history.

### Removed
