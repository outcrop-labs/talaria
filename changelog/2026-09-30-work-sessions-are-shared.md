- **A work session shares like a plan does.** The session header carries the
  same members row Plan has: the owner invites people or whole teams, a
  collaborator can leave, and a green ring means someone is looking right now.
  @mentions in a session offer its members rather than the whole org, so a
  mention reaches somebody instead of quietly notifying nobody.

  None of that is new machinery. Membership already lived on
  `conversation_members` and `conversation_teams`, which were never
  plan-specific, and a work session has carried the same membership clause
  since the view landed. What changed is that the reader the share routes use
  stopped asking "is this a plan" and started asking "may you share this" — one
  question, two surfaces. The plan's living document and its ticket drafts gate
  themselves separately on being a plan, so they are untouched.

  The share endpoints now also answer at `/api/conversations/{id}/members` and
  `/teams` — the same handlers at a kind-agnostic address, not a second copy.
  The `/api/plans/{id}/…` pair stays because a URL is a promise, but the client
  speaks one spelling. The members component moved out of the Plan route folder
  into the shared chat components, where a thing two surfaces use belongs.

  Verified: `bun run check` green (283 routes, generated reference regenerated);
  `bun run typecheck` 0 errors (2 pre-existing a11y warnings in WorkchainCanvas,
  untouched); 1321 ui tests pass; every touched api crate parses and is
  formatted under `cargo fmt --check`. Compilation is CI's.
