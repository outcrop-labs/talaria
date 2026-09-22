- **The API reference is generated from the Rust sources again** — the #293
  extractor port. `gen-docs` now reads the router table
  (`api/src/routes/mod.rs`, cross-checking each registration's 405
  allow-string against its parsed method set) and the handler modules under
  `api/src/routes/**`: guards → the Auth column, `crate::body` member calls
  → the body tables, `json!` literals → Returns, `StatusCode` literals →
  Statuses, `// doc:` comment runs → notes. The three TS residents still
  serving `healthz`, `admin/update` and the app dispatch keep their TS-extracted
  rows, and a resident wins over any router twin (`/api/healthz` answers
  direct-binary checks in Rust; the documented surface is the proxy's). The
  frozen-at-cutover posture and its hand-maintenance are gone, and the
  `--check` drift tripwire in `bun run check` is live again. Regenerating
  against the freeze, every divergence fell into named classes: zod
  spellings → `crate::body` vocabulary, the honest 400s the TS extractor
  couldn't see (Rust spells body-parse refusals in the handler), error-shape
  Returns fixed to the real payloads, `/api/mcp/gw/{server}` re-sourced to
  its Rust module, and richer port response shapes (`{agent, registered}`,
  muse POST now an SSE stream).

### Fixed
