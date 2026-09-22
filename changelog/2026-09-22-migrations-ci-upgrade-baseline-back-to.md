- **Migrations: CI upgrade baseline back to `origin/main`.** The repair
  (#398) pinned the upgrade pass's baseline to e9f08476 — the last array
  any deployed instance ran — because main's tip then carried the broken
  order. With the repair merged, the default (`origin/main`) is the honest
  baseline again and the pin is gone. Verified: this PR's own migrations
  run compares origin/main's array against itself — `applied: 0`, snapshot
  matches.
