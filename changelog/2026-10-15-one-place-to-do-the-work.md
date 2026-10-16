- **Work, Plan and Research are one surface now.** They were three views of the
  same shape — a rail of sessions, the stream on the left, the thing the session
  is about on the right — sharing 19 imports and 1,399 lines, and keeping them
  apart made the person do the filing: *is this thought a plan or a session?* is
  a question the tool should never ask. There is one **Work** surface (`/work`),
  one rail over every session you work in, one agent picker in front of it, and
  one entry in the nav whose badge is the sum of all three.

  **The rail is one list, from one query.** Work sessions, plans and research
  threads come back from `/api/conversations?kind=sessions` already ordered
  together, with the unread counts and the shared-with-you rows computed once.
  Each kind keeps **its own audience rule**: plans and work sessions are
  multiplayer, a research thread follows its run's audience (owner, member, or a
  team the run was shared with), chats stay strictly your own. Two things the
  single-kind list had kept theoretical surfaced on the way: the membership arm
  gated on the *request* rather than the row's own kind, and `research` was
  missing from the route's whitelist, so asking for it returned **chats** — a
  quiet wrong answer rather than an empty one. Both fixed, pinned by a live-DB
  module (`conversation_audience_live.rs`) that CI's self-maintaining suite
  discovers; the stray-membership-row case fails against the old clause. The
  unread badge is brought into lockstep with the list, as its own comment asked.

  **The right-hand pane is a strip of tabs** modelled on OpenWork's panel
  store, and the set grows while the agent works: the session's living
  document, a document the agent opened or wrote, a Google file pinned to the
  session, the run's cited report, the tickets it has drafted. A touched
  document opens **at once** under "Document" and takes its title when the read
  lands; the living-document tab cannot be closed, because the pane always
  needs something to fall back on. The active tab is remembered per session in
  the browser, as a convenience only.

  **Research is an ask, not a view.** The question box opens *over* the stream
  (it used to replace it, which unmounted the chat and aborted a reply in
  flight); the run it starts opens as a session, because a report is something
  you then talk about. Research-origin rows keep what the old rail gave them —
  the status dot that pulses while running, the mode chip — and their Rename and
  Remove act on the **run**, which now takes its discussion with it: the FK
  between a run and its thread is `ON DELETE SET NULL` both ways, so deleting
  either used to orphan the other, with the run's driver still spending. A
  research session is one thing with two rows, and both delete paths say so.
  Runs that have no thread at all (an ownerless org agent's, or rows that
  predate runs making their own) are still listed, in a group of their own, and
  open their report standalone. On a phone a research session shows its report,
  not only the stream, as the old run page did.

  **Every old address still resolves.** `/plan/<id>` reads as the selection;
  `/research/<runId>` resolves the run's discussion and replaces the URL with
  it. The selection logic is a pure helper with its own tests
  (`work-selection.ts`), because the one thing that must never happen on a
  mailed link is a run id leaking into the conversation plane as if it were a
  session. `/plan` and `/research` stay in the grantable-views list as legacy
  denial keys.

  **Three things Work never had, wired.** `/work` was missing from the
  assistant's surface list, so asking the Inbox assistant from a work session
  made it think you were on Home; a work session **could not draft tickets**
  (`plans_id_draft.rs` admitted only `chat` and `plan`); and Work had no badge.
  All three in. With the surfaces merged the creation gate accepts **either**
  `work.sessions` or `plans.create` — a person granted the plan permission alone
  could start a plan yesterday and could start nothing today — and a template
  chosen on a new session now actually seeds its living document: the id was
  stored but `sync_plan_doc` refused any template for a work surface, which was
  right for the agent's *implicit* binding and wrong for a pick the person made
  by name. Explicit seeds either surface; the silent binding stays plan-only.

  **Deliberately not restored**, named rather than lost quietly: a new session
  is `kind='work'` (a plan can no longer be *started*; existing plans keep their
  prompt and their living document until the harness prompt lands), the
  research rail's own empty/error copy (one rail, one voice), per-surface
  remembered state (collapse key, sticky agent — one surface now), and
  `claimViewTitle` naming a run rather than its thread (inert today: nothing
  reads it).

  Verified: `bun run check` clean (911 files, 15 rules, **0 duplicate-body
  clusters** — the three views, `RunView`, and both Home tabs are deleted, not
  left beside their replacements); `bun run typecheck` 0 errors over 5,483
  files; **1,531 ui tests in 99 files** green, 29 of them new (selection,
  the badge sum, the tab store including `labelTab`, the surface list); scoped
  cargo across `talaria-conversations`, `talaria-research`, `talaria-plan-doc`,
  `talaria-routes-boards`, `talaria-routes-comms`, `talaria-harness-defs`, clippy
  clean. The merge was built by a fan-out, then **adversarially reviewed against
  the deleted originals** (45 behaviours preserved, 16 lost, 14 changed), and the
  lost ones fixed and re-reviewed: 13 fixed, 2 partial on the server side until
  the template and delete-together changes above landed, which they now have.
