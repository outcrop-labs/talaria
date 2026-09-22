- **The app updates itself, from inside the app.** Admin → Security carries an
  Updates panel: it shows the running commit, checks the remote, and an
  Update now button that pulls the latest release, installs, builds into a
  staging directory and swaps it in, then restarts the server with no shell
  involved. Manual by default; an Update automatically toggle (off until
  someone turns it on) has the scheduled check install updates on its own
  every few hours. The restart is the careful part: the build lands in
  `ui/dist-next` and is swapped for `dist` only when complete (a live server
  never serves a half-built bundle), the old `dist` stays behind one
  generation as a manual rollback, and a detached helper waits for the port,
  boots the replacement, and waits for it to answer. The new server is the
  one that marks the update done, and an update that never lands shows as
  exactly that rather than a fake success. Safety rails: a dirty checkout on
  the server is refused rather than pulled over, dev installs never update
  (vite reloads on its own), and `TALARIA_UPDATER=off` is the kill switch for
  deployments supervised some other way. Verified live: the panel's API
  answers in dev with mode dev and refuses to apply there, a check against
  the real remote reports behind/ahead honestly, the auto-update toggle
  round-trips and persists, and 9 unit tests cover mode gating, dirty-tree
  refusal, and the running → done/failed reconciliation across a restart.
