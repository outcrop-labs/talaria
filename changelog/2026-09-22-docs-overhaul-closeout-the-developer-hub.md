- **Docs overhaul, closeout — the developer hub.** New `docs/ARCHITECTURE.md`: the two planes
  (one app process + the stateless MCP proxy) with the port table, the request lifecycle
  (including the honest note that CSRF protection is SameSite=Lax + OAuth state, not
  middleware), the auth stack (sessions, `tak_` agent keys, the legacy-key refusal, `tlk_`
  bearer keys), the data layer (append-only checksummed migrations, envelope-encrypted
  secrets, numerics-as-strings), the realtime bus and its "says what changed, never what it
  says" rule, rendered fleet + blue/green rolls, the 51-tool MCP plane and its server-side
  guardrails, the gateway as the single enforcement point, compile-in apps, and the
  prod-only scheduler. `DEVELOPERS.md` rewritten as the complete hub — every doc in the repo
  listed and grouped, nothing orphaned; `CONTRIBUTING.md` slimmed to the rules that aren't
  style; the old `HANDOFF.md` moved to `docs/history/` behind a stub, and the completed
  2026-08-26 audit with it. Truth fixes the pass surfaced on the way out: routing is the
  `defineApi('…')` literal, not the filename (API-CONVENTIONS said the dot rule was
  load-bearing; it's convention); dev infra is six services, not two (Postgres, Redis,
  Qdrant, TEI, MinIO, SearXNG); `mcp/`'s guardrail described as what it actually is — no
  assignee writes, no terminal status moves.
