- **ui's typecheck no longer sweeps gitignored subrepo apps.** Any machine
  with `apps/leadworks`/`apps/waypoint` checked out sprayed ~100 phantom
  "Cannot find module" errors into every svelte-check (subrepo imports
  resolve against their own absent node_modules), which is why CI disagreed
  with every local run. The subrepos are now excluded in `ui/tsconfig.json`,
  and a new `bun run check` invariant (`subrepo-app-inside-the-ui-tsconfig`)
  fails in seconds when a future subrepo appears unexcluded.
