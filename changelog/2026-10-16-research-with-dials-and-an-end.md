- **Research has dials, knows when it has enough, and can start inside the
  conversation you are already in.** A research run was one of three presets
  — recon, brief, expedition — and a preset was all or nothing: somebody who
  wanted an expedition's rounds over a brief's handful of angles took the
  whole shape or none of it. Every run also spent its full budget, round
  after round, whether or not the second round had already answered the
  question; and a run always opened its own thread, so research commissioned
  from a plan or a work session reported somewhere else.

  **Three dials, 1–3 each: effort, breadth, sources.** Effort is rounds
  (1/2/3), breadth is angles per round (1/3/4), sources is how many
  citations the report may stand on (6/12/24). The presets are the dials
  with names — recon 1/1/1, brief 1/2/2, expedition 3/3/3 — and **a mode
  nobody has dialled costs exactly what it always did** (1×1, 1×3, 3×4; the
  maps are pinned to the old budget by test). `POST /api/research` and the
  `research` tool take any of the three beside `mode`; the run records
  them as `dials` and the wire carries them back.

  **`research-enough`** — a new census site (noul, lean 0.75, on). When a
  round completes with rounds still to spend and something citable found,
  the judgment reads the ask, the scoper's read and the findings so far and
  answers "do these answer it at this depth?" — yes writes the report now,
  with the phase saying *enough after round N*; the budget's own "keep
  going" is the shadow baseline. It is never asked on the last round or on a
  round that found nothing. A follow-up's inherited sources sit outside the
  cap (the cap counts what *this* run read), and a marker the cap took off
  the report is not counted as a fabrication — only a number no source ever
  carried is.

  **The scoper acts, one way.** `research-scope` has been measured against
  the scoper's own verdict since it was declared; it may now override a
  *vague* verdict with a confident *crisp* — proceed on the read, ask
  nothing — and never the reverse (a confident "vague" with nothing to ask
  is not an answer). Off by default, and the judgment is not even made
  while it is off.

  **`conversationId`.** A run started with one discusses and reports into
  that conversation — a plan, a work session — rather than minting a
  research thread. The caller must be able to open the conversation (a
  conversation they cannot see answers "not found", never "forbidden"), and
  one live run per thread: a second start while one is working answers 409
  with the run it would duplicate. The conversation-keyed readers follow the
  newest run.

  Verified: 45 tests in `talaria-research-def` (+11: the dial maps and the
  presets' unchanged cost, an old checkpoint without the new fields still
  deserializing, the cap keeping the lowest indices, the sources dial end to
  end, and the loop — enough after round one writes the report and never
  plans round two, a no runs both, the last round is never asked, a round
  with nothing citable is never asked); 100 in `talaria-decide` (+8:
  `enough` state clamping, the one-direction override, the census's
  consistency tests over 12 sites); 19 in `talaria-routes-workbench` (+6,
  the body's validation); 8 in `talaria-research`; the catalogue and sandbox
  parity tests (10 + 77); `bun run docs:api` regenerated; migration snapshot
  updated; svelte-check and the MCP typecheck clean; a new `#[ignore]` live
  module `research_dials_live` (a dial off the scale is a 400 naming it; a
  conversation the caller cannot see is 404; a valid body on a workspace
  with no search path answers the no-search sentence as a 4xx, never a 500).

  Not changed, on purpose: the Plan/Search stages keep their names — a
  3,000-line driver with checkpoints older deploys wrote, for a cosmetic
  rename. A live run needs a search backend and a model; the Z.ai account
  is out of balance, so the loop's live proof waits with the others.
