- **A delegating agent now shows what it handed out, and what came back.** An
  agent that hands work to a child goes quiet. The child runs under its own
  session id, so the tool events the `talaria-events` plugin already reports for
  it carry a correlation the parent's reader has no way to join — and the
  parent's stream shows a minutes-long gap that reads, to the person waiting on
  it, exactly like a hang. Nothing in the chat said work had been delegated at
  all, let alone to whom or how it went.

  Hermes fires `subagent_start` and `subagent_stop` around every child
  `delegate_task` spawns, and the plugin now registers both. They go to their
  own door — `POST /api/agents/subagent-events`, agent-authenticated with the
  agent's own key, the same door shape as tool events — which lands **one chip
  per child** on the turn the parent is speaking in: *"reviewer · check the bind
  casts"*, running, then done with the child's summary and the tools it used.

  **The stop overwrites its start in place.** The chip id is the child's session
  id, because that is the only handle both hooks carry, and `subagent` joins
  `todo` and `question` as a snapshot kind — a kind whose content is the current
  state rather than a record of something that happened. Stacking a second chip
  would read as two children; keeping the start would leave a finished child
  reading as still running, which is the one thing the chip exists to say. A
  stop whose start never arrived **still lands**: the start is best-effort
  observability and can drop under queue pressure, and a result with no
  beginning is far better than a child that silently never finished.

  That rule turned out never to have been enforced against the database.
  `snapshot_kind` existed and `merge_chips` honoured it, but nothing in
  production called `merge_chips` — the write did its own merge in SQL as a pure
  first-wins append, so a plan rewritten mid-turn kept its *first* version in
  the row too. The merge is now one statement that states both rules: every
  incoming chip appends when its id is new, and the snapshot subset **replaces
  the chip already sitting at that id, in its place** — with ordinality, so a
  plan that rewrites itself does not jump to the end of the row and drag the
  reader's eye with it. `todo` and `question` get the behaviour they were always
  documented to have.

  **And a cap, because delegation is the one agent capability that multiplies
  itself** — a child may delegate too, so without a number somewhere a single
  request can fan a managed box out into as many concurrent sessions as the
  model feels like asking for. `agent_defs.max_children` (default **3**: useful
  out of the box, small enough that nobody discovers the cap by running out of
  memory; **0** is a real setting and means this agent delegates to nobody)
  renders into the agent's `config.yaml` as
  `delegation.max_concurrent_children`. It is written from the column every
  render, so a def cannot author its way past it — and every *other* delegation
  key a def set is left alone, because the rest of that block is the agent's to
  tune. `PATCH /api/fleet/defs/{id}` takes `maxChildren` 0..16, audits it as
  `agent.max_children`, and **rolls the agent exactly as the Developer switch
  does** (one roll even when both change — the render it picks up carries
  everything).

  The platform trusts the plugin's clamps no further here than it does for tool
  events: kind, child session id, role, goal and summary are re-validated,
  re-clamped and secret-scrubbed **at the boundary**, scrub before clamp so a
  key cut in half cannot slip its prefix into the row. `childStatus` arrives as
  the harness words it and is mapped here, not in the container — anything
  error-shaped (`error`, `fail`, `timeout`, `abort`, `cancel`) is an error,
  everything else finished — because a container that already holds this plugin
  file cannot be re-taught one harness version's vocabulary.

  Verified: `talaria-chips` 13 tests (2 new: a stop replacing its start in place
  beside an untouched link chip, and only snapshot kinds reaching the replacing
  leg of the write), `talaria-routes-fleet` 22 (3 new: the two kinds and the one
  required field, a start that runs against a stop that carries the harness
  verdict, and the label's clip with a key scrubbed out of the goal),
  `talaria-fleet-render` 21 (1 new: the cap lands alone on an empty map, an
  authored block's own keys survive while a def's `max_concurrent_children: 99`
  is overridden, and the emitted YAML carries it unquoted and parses back
  identically), `talaria-agent-defs` 5. Clippy `-D warnings` clean across those
  four plus `talaria-api-routes`. The exact merge statement the macro emits was
  run against a real Postgres in a rolled-back transaction: the snapshot
  replaced at its own index, the approval left as its first version, the new
  chip appended last. The plugin was exercised in process against a stubbed
  `urlopen` — four hooks registered, a 64-entry mixed tool history clamped to 40
  names, and the tool event still posting its same eight keys with no `url`
  among them, so that wire is byte-identical. Four `#[ignore]`d live tests in
  `api/tests/it/subagent_events_live.rs` **ran, 4/4, against a fresh
  database**: start then stop is one chip that turns done, an orphaned stop
  still lands, the bogus kind and the unkeyed caller land nothing, and the cap
  refuses 17 while keeping 4. Migration replay + snapshot on a throwaway
  Postgres (426 statements); `bun run gate` green. `bun run check` green — 902 files / 15 rules, 596 docs / 835 links, 313
  routes / 503 method rows / 25 generated files all current; svelte-check 5474
  files, 0 errors.

  Not yet: the chip has no renderer. `ChipKind` admits `subagent` and the
  chip's fields are typed, but `ChatChips.svelte` has no branch for it, so the
  chip is stored, merged and fanned and draws nothing — and the Summary-tab
  control for the cap waits on #542, which owns that file. Until both land,
  delegation is visible through the api and the row, not the chat. The child
  id is bounded in UTF-16 units, the same measure the clamp uses.
