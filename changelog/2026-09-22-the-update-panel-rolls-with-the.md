- **The update panel rolls with the engine — and the git-checkout updater
  retires.** Admin → Security → Updates now answers against
  /api/admin/updates: running version and slot, what's available, Check /
  Update now / Roll back (the kept old slot is the target), the auto-update
  toggle (image installs only, off by default), and the deploy key — minted
  once, shown once, stored as a hash. Non-image installs show the engine's
  own refusal sentence; un-adopted installs keep deploying the way they
  always have, and the panel says that too. The TS git updater it replaces
  (ui/src/server/updater.ts, the admin.update resident,
  scripts/update-restart.mjs) is deleted — the proxy resident list drops to
  three, and the routes/api reference carries the Rust surface instead.
