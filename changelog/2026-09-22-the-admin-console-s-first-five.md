- **The admin console's first five groups speak Rust**:
  `/api/agent-role-templates` (built-in + own role templates, the shadowing
  rules), `/api/admin/password-accounts` (create/reset/remove — the password
  hashes itself never ride the audit log), `/api/admin/google-client` and its
  `/login` sibling (status, sealed client config, login toggle), and
  `/api/admin/instance` (the hosting domain and its self-fetch verification).
  `/api/admin/permissions` joins them, and its PUT body is the port's first
  zod union: probed against zod 4.3.6, only a string userId failing the uuid
  format check surfaces its own message (`Invalid UUID`) — every other
  both-branches-fail shape collapses to `Invalid input` — and that table is
  pinned in the Rust tests. All five byte-diffed against TS across anon,
  member, and admin callers, including the 405s with their `allow` headers,
  the full 400 table, and restore-safe write cycles on both runtimes over the
  same dev database. One recorded divergence (docs/RUST-MIGRATION.md):
  clearing the instance domain works from Rust and cannot from TS, whose
  driver writes JSON null as SQL NULL and answers the clear with a leaked
  Postgres constraint sentence.
