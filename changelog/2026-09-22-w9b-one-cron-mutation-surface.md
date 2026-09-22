- **W9b — one cron mutation surface.** `useCronMutations` replaces the two
  panels' create/edit/act code. The plan's premise is wrong for this pair and the
  agent proved it: both panels hit the SAME endpoint
  (`/api/fleet/agents/{id}/crons`), so the planned `base` parameter was dropped
  rather than kept as a dead argument — the agent id is a method argument. Both
  error channels survive (the agent panel's inline `err`, the fleet tab's
  `toastError` fan-out, with its titles verbatim) and both invalidation rules
  (the panel's "not after a failed create/edit", the tab's unconditional) are
  passed in. Verified with a throwaway vitest against a stubbed fetch: 3/3,
  including confirm-before-request on delete, a 500 firing BOTH channels, and
  `busy` released after the invalidate.
