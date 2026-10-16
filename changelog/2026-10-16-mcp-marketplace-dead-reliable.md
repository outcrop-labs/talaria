- **The MCP marketplace searches a local mirror of the official registry; credentials,
  OAuth app setup and per-agent MCP are first-class.** Search queried the registry live on
  every keystroke, and the registry stalls: measured 2026-10-16, one search took 9.7s and the
  next three timed out at 15s, which the picker rendered as "No servers match"; its search was
  also a name-substring match, so "google drive" found nothing. The scheduler now mirrors the
  registry into `mcp_library_entries` (`/v0.1/servers`, latest versions, full cursor walk once,
  `updated_since` incrementals every 30 min, resumable, "Sync now" from the picker) and search is
  Postgres full text over title/publisher/name/description with tier and hosted/package filters;
  the live registry answers only while the first walk runs. Marketplace package installs were
  filed under the Built-in tab (a `talaria-pkg://` URL failed the http scheme test) and could not
  be removed — they are External now. Servers that need an API key the registry never declared
  had no field for it anywhere: discovery now records a probe (`last_probe`: ok, auth-required,
  unreachable, error, with the server's own words), the card says "needs credentials" and opens a
  credentials editor (declared fields, or free header rows; `mergeHeaders` sets and removes keys
  without ever echoing a value; saving re-tests), installing from the marketplace asks for the
  key in the same step when discovery answers 401, and the custom form takes any number of
  headers. OAuth: the callback URL shown for manual apps came from `window.location` while the
  authorize request used the verified hosting domain — providers rejected the mismatch; the api
  now computes the one canonical callback and the card shows it, with the provider's developer
  portal and the steps that matter there (two dozen providers, `guides.rs`), an explicit scope list for
  manual apps, **Re-detect auth**, and automatic OAuth detection when a header server starts
  answering 401. The agent settings MCP tab attached RAW URLs to the Hermes config, bypassing the
  gateway, with no marketplace and no registry toggles; it is now the registry scoped to the
  agent (`GET`/`POST /api/fleet/defs/{id}/mcp`): what it carries and why, a tool subset per
  assignment, one-toggle assignment of what exists, add-from-marketplace that installs and
  assigns, leftover raw entries listed for removal, raw `add` refused. A personal assistant's
  rendered config also stops listing a server its owner's team denies (the gateway already
  refused it), and hosted servers registered through the api no longer carry a jsonb `null`
  in `package` that every `is_some()` reader took for "is a package" (the insert binds SQL
  NULL; a migration clears the rows already written). Verified: cargo tests for the library (sync bookkeeping, domains, tiers), oauth
  (guides, meta), and the registry crates; vitest for the status matrix and the external/internal
  split; `bun run gate`; and in the worktree app: catalog sync from empty to the full registry,
  search for brands the old path missed, install → needs-credentials → set key → tools discovered,
  the agent tab assigning and narrowing, and the canonical callback on an OAuth-app card.
