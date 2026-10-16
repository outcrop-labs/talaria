- **Stop stops, and you can talk to an agent while it works.** Stop used to
  abort the browser's read and nothing else: the Hermes run kept going behind
  a frozen transcript until it ran out of things to do. And there was no way
  to say "not that — this" mid-turn; a correction typed over a streaming reply
  could only wait for the reply to finish being wrong.

  Both now reach the run, through the run id a pinned session lands on the
  row (`#542`): the stop tile posts `/api/conversations/{id}/stop` before it
  aborts, and Hermes hard-interrupts the agent. The turn lands **complete**,
  with whatever it managed to say and a `· stopped` note — not an error row
  with a retry affordance on a turn nobody wants retried — and the chain
  behind it stays quiet unless somebody has spoken since. (A turn on the
  full-history path has no run to interrupt; the route says `hermes: false`
  and the client's abort is, as before, the whole of it.)

  **Send now.** While a reply streams, Enter still queues your message for the
  next turn. ⌘⏎ — or the *Send now* control beside the stop tile — hands it to
  the turn already running: `/api/conversations/{id}/steer` writes the user
  row **first** (what you said mid-run is part of the thread whether or not
  the agent could take it), then steers Hermes. `steered` means the run took
  it, and the row is stamped so the completing turn's chain does not answer
  it a second time; `queued` means the run was past taking it and the row
  waits like any queued message. The row wears a small "sent mid-turn" note.

  **The doom-loop guard.** Over the API, Hermes' own loop breaker is a TUI
  prompt nobody sees. Talaria now counts identical calls — same tool, same
  arguments, one count per call however chatty its progress frames — inside
  one turn. At **3** a `notice` chip lands on the reply ("Repeating the same
  call") and a steer tells the agent to change approach or say what is
  blocking it; at **5** the run is stopped and the reply says why, after its
  last word. The decision is pure (`loop_verdict`) and fires on the exact
  count, so a pair stuck at twelve repeats earns one nudge and one stop, not
  a nudge per frame.

  On the way: a session's tool rows now carry one id **per call** — Hermes
  names no call id on its session frames, and three runs of one tool in one
  reply had been merging into a single row rewritten three times.

  Verified: 78 tests in `talaria-gateway` (+4: `run.cancelled` lands the
  words said before it and ends the turn as *stopped*, `run.steered` lands
  nothing, the steer body is sealed, three calls of one tool are three
  rows); 17 in `talaria-chat-persist` (+5: the guard's thresholds, a repeat
  that warns then stops, different arguments never counting as a loop, pairs
  counted apart, unlabelled calls never counted); 639 UI tests (+18: send-now
  vs queue, the ⌘⏎ key, the notice chip, the two new client calls);
  svelte-check clean; `bun run docs:api` regenerated (two routes); a new
  `#[ignore]` live module `run_control_live` proves the Hermes-free half
  against Postgres — both routes refuse with 409 when nothing streams and
  write nothing, a full-history turn's stop answers `hermes: false`, its steer
  lands as a stamped queued row, and a stranger gets the same 403 as the read.

  Live, against the hired Hermes agent from `#542` (GLM-5.3 Flash via Z.ai):
  a turn told to sleep twice in the terminal, **stopped mid-sleep** — Hermes
  answered `stopping`, the row landed complete and stopped with the partial
  text, `GET /v1/runs/{id}` read `cancelled`, no chain re-fired, and a second
  stop answered 409. A second turn **steered mid-run**: Hermes accepted it
  (`200 steered`), the row landed stamped — and the turn then died on Z.ai
  answering **429 `1113`, "Insufficient balance"**, which also kept the
  doom-loop turn from ever reaching a tool call. Those two live halves — a
  steer carried through to the reply, the guard tripping on a real run — wait
  on the account; the guard's decision and the routes' Hermes-free half are
  the tests above. On the way: Z.ai's `1113` now reads as the account limit
  it is at the gateway's boundary ("out of credit or over its usage limit")
  instead of a generic 429.
