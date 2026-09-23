- **Generated API reference: every path documents itself.** The generator grouped the
  router table by handler module, not by path — so when several `.route()` registrations
  shared one module file, the later ones collapsed onto the first-seen path's section.
  TALA-35's new `/api/workchains/{id}/edges` routes exposed it (documented as duplicate
  `POST /steps` rows, the wire body under the step section); the same collapse had already
  folded `/api/boards/{id}/agents/self`, `/api/push/subscribe`, `/api/push/unsubscribe`,
  and three integrations paths into their module siblings' sections. The table is now
  grouped per path, as the code's own comment said it was: 245 → 254 routes, 422 method
  rows, each registration in its own section. Verified: `bun run check` green
  (invariants, docs links, no drift, mcp cross-check, changelog roll), regenerated
  `docs/api/{README,boards,activity,integrations}.md` reviewed line by line.