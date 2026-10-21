- **Fixed:** A fresh `talaria worktree` could render no fleet, and its `ui/.env`
  strip-list could drift one key short a third time. Two things seen on a fresh
  `bun talaria worktree <name> origin/rc`: the app on `:53xx` answered every
  `/api/*` with `upstream unreachable` while its own api was healthy on `:54xx`
  (main's `TALARIA_RUST_API_URL=http://127.0.0.1:5274` survived the copy and
  `talaria dev` derives the url only when nothing names it), and
  `POST /api/fleet/render` 500'd with `fleet chassis missing` because only
  `talaria setup` seeded `fleet/chassis.yml`. Both lines of code had landed on
  `rc` before this (`da23c0bb`, `ec3b0865`); what had not was the guard against
  the next one, a test for the chassis seed, and the docs.

  The strip-list is now derived from what the generated block writes
  (`WORKTREE_ENV_KEYS`, with `stripOwnKeys`) rather than kept by hand, and a
  test asserts the two sets are equal — a key appended to the block but missing
  from the filter is the exact shape of both earlier bugs. That closed a real
  third instance: a devbox's `ui/.env` carries `TALARIA_GATEWAY_SELF_URL` and
  `TALARIA_FLEET_PROJECT`, so a worktree cut from inside a box inherited them
  and its agents would have called back to the box's app under the box's fleet
  project. The fleet seed is `seedFleetPlane`, tested on its own: seeds the
  chassis from `scripts/chassis.template.yml` and says so, keeps an existing one
  (user-owned config) and says so, and warns rather than silently skips when
  the template is missing — a silent skip reproduces the 500 this step exists
  to prevent. `docs/WORKTREES.md`'s step list names every key the env rewrites
  and the fleet plane step.

  Verified: `cd cli && bun test` — 13 in `worktree.test.ts`, whole suite green;
  removing one key from `WORKTREE_ENV_KEYS` fails two tests (the set-equality
  guard and the devbox-fixture strip); `bun run check` clean.
