- **UI detail pass (live walkthrough, 2026-07-28).** A guided sweep of the
  whole surface, in nine moves. **Typography, two voices**: IBM Plex Sans
  for everything a person reads or types (markdown surfaces, editors, all
  form controls, board cards/lists, home rows, both content browsers); mono
  stays the chrome/identifier voice — the rule lives in UI-CONVENTIONS.
  **Dialogs take over the screen** (`Modal takeover`): content-heavy
  dialogs fill the viewport minus a gutter, constant height, content
  scrolls inside; deep editors (skill/memory/soul/config-history) open as
  ticket-style nested slide-ins INSIDE their dialog instead of stacking
  modals, with Esc peeling one layer at a time. **Agent editing reworked**:
  the skill editor no longer opens empty (it seeded from a still-loading
  fetch), memory gains quick-add + rich display, and crons got a real
  scheduling UI — pick daily/weekdays/weekly/monthly/interval with time
  pickers, see it in plain English, cron syntax generated underneath — plus
  in-place job EDITING via the never-wired `hermes cron edit`. **Retired
  agents can be deleted** (admin, confirm-gated): def + versions + secrets
  + rendered files + created-agent volumes; produced history and imported
  legacy volumes are kept. **Admin is tabbed by concern** (Organization /
  People / Agents / Retrieval / Storage / Security) and **Settings too**
  (Profile / Assistant / Connections / API keys) — with every panel's
  explanatory paragraph moved into ⓘ `InfoTip` tooltips. **The assistant
  is fully editable in Settings**: identity, handle, personality, model
  tier chips, start/stop, and inline Schedules/Skills/Memory — the
  user-friendly cut of the admin config, all owner-scoped. **Buttons never
  wrap** (`whitespace-nowrap` in the base). **Skeleton loading
  everywhere**: `Skeleton`/`SkeletonRows`/`SkeletonCard` replace blank
  panes and "Loading" strings across home, boards, tickets, agents, comms,
  browsers, cost, inference, activity, alerts, research — and the app
  shell itself now server-renders a skeleton frame instead of a blank
  page, which was the real "no content, then BAM" (the whole app was
  gated behind the client-side session fetch). Conventions for all of it
  are recorded in docs/UI-CONVENTIONS.md.

### Added
