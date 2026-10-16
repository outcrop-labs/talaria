- **An agent can show you the page it saw, and an operator decides which
  toolsets it gets at all.** Hermes turns every built-in toolset on by default
  and `agent.disabled_toolsets` in the rendered `config.yaml` is its last word
  on availability, so until now the only names Talaria closed were the two it
  hardcoded (`todo`, `clarify` — its own versions are the ones a person can see
  and answer). `agent_defs.disabled_toolsets` is the operator's list on top of
  that pair: a jsonb array, empty by default, set whole through
  `PATCH /api/fleet/defs/{id}` as `disabledToolsets`, read back on the def wire
  (list and get). Only Hermes' 28 governed built-ins pass — a typo would reach
  the rendered config and be ignored *there* in silence, leaving a toolset the
  operator believes closed — and the refusal names the first stranger, because
  one mistyped name among a screenful of checkboxes is otherwise a 400 nobody
  can act on. The renderer merges three lists in one order that is itself the
  meaning: the floor Talaria always suppresses, then what the operator closed,
  then whatever the def's authored config already carried, deduped. A reader of
  the rendered file can tell the three apart by where they sit, and a re-render
  of the same three emits the same bytes. The change rolls the department, and
  a PATCH that also flips Developer Agent is still **one** roll. The change is
  audited as `agent.toolsets` with both sides on it.

  **The first toolset this governs is `browser`**, because a browsing agent
  costs real money on every roll. Chromium is not in the hermes-agent image;
  `hermes tools post-setup agent_browser` downloads it, which takes minutes,
  and the image points `PLAYWRIGHT_BROWSERS_PATH` at `/opt/hermes/.playwright`
  — *inside the container layer*, so the install is gone with the next roll and
  the following browser turn fails on a missing executable. Every agent's
  compose env now carries `PLAYWRIGHT_BROWSERS_PATH=/opt/data/.playwright`, on
  the department's state volume, which outlives the roll (a def whose chassis
  extras name their own path keeps it; the Developer Agent's harness dir still
  wins, on the same volume). The install itself runs at the two places a fresh
  container becomes real — after `wait_healthy_slot` in the roll, and after the
  `up`'s health wait — detached, only when the department's agent still holds
  `browser`, and guarded by a marker file on the volume so the second call
  costs nothing. It is best-effort by contract, like the bundled-skill prune
  beside it: an agent whose browser never landed cannot browse, which is not a
  fleet that cannot serve. The marker is written only on success, so a failed
  download never looks installed.

  **And the screenshot now reaches the person.** `browser_vision` writes a PNG
  inside the container and reports its *path* — a path that means nothing to
  the platform and is gone with the container. The `talaria-events` plugin
  (1.1.0) reads the file the tool named, bounded at 5 MiB, and posts the bytes
  to a new agent-authenticated door, `POST /api/agents/uploads`: base64 in a
  JSON body, three image mimes, both size bounds re-decided server-side because
  the platform does not trust the plugin's clamps. The image rides *after* its
  own tool event, so several MB of base64 never stands in front of the record
  the run watch reads, and the tool-event wire is byte-identical to before. The
  door's two writes are ordered, and the order is the point: the attachment is
  stamped on the turn's message row *before* the chip is surfaced, because
  `can_access_upload` answers "may this reader see these bytes" by probing the
  attachments JSON of rows the reader can reach — a chip ahead of its stamp is
  an image that 404s for everyone but the owner, and the chip write is also the
  realtime nudge that sends a browser to fetch it. No live turn to land on is a
  409, asked before a single byte is stored: a screenshot with nowhere to go is
  a blob nobody will ever open, and the next tool call is one screenshot away.

  Verified: 61 unit tests across the six crates this touches, all green —
  `talaria-agent-defs` 6 (+1: trim, dedupe keeping the first, the cap, and the
  refusal naming the *first* stranger, including a real Hermes platform toolset
  Talaria still refuses), `talaria-fleet-render` 23 (+2: the operator's list
  merging between the floor and the authored names with dedupe against both,
  idempotence over a previous render, `browser_enabled` over three defs, and
  the browsers path landing unless a def named its own), `talaria-fleet-docker`
  5 (+1: the install is marker-guarded on the state volume and `&&`, never
  `;`), `talaria-routes-fleet` 24 (+5: the mime list is the whole list, the
  filename trimmed and bounded, both size bounds — the wire's and the decoded
  one, each proved at its own ceiling — a non-object body, and the image chip's
  shape pinned whole), `talaria-conversations` 3, `talaria-fleet-reconcile`
  compiles. One existing expectation moved: a def whose authored config said
  `[browser, clarify]` now renders `[todo, clarify, browser]` where it rendered
  `[browser, clarify, todo]` — the merge order is the new contract, and nothing
  was loosened. `cargo clippy --all-targets -- -D warnings` clean on those six
  plus `talaria-api-routes`; `cargo fmt` clean. Three `#[ignore]`d live tests
  in `api/tests/it/browser_toolsets_live.rs` link clean and assert from
  Postgres rather than from the handler: the stored column, the wire field and
  the audit row; a real 1×1 PNG landing as one attachment *and* one image chip
  on the live streaming row, with `can_access_upload` then true for the
  conversation's owner through the attachments probe alone (the def owns
  nobody, so the owner shortcut cannot be what answers); and the four refusals
  — unsigned, dead turn, bad mime, oversized body — each leaving the live row
  and the `uploads` table untouched. Plugin behavior exercised out-of-band:
  both result shapes resolve a path, prose merely mentioning a `.png` does not,
  an oversized or unreadable file yields nothing, and a `browser_vision`
  success enqueues exactly two events while every other tool enqueues one.
  `bun run typecheck` 5475 files, 0 errors; `bun run docs:api` regenerated
  (315 routes, agents 25→26); `bun run check` exit 0 — 903 files/15 rules, 837
  doc links, gen-docs all current, mcp-routes, changelog and sweep clean.

  The install has an operator's brake — `TALARIA_BROWSER_INSTALL=off` skips
  it, with a line in the log — because a Chromium download per department is
  real network and disk, and a dev stack rolling agents to test something else
  should not pay it.

  Not here yet: the Svelte renderer for the `image` chip (`chips.ts` carries
  the kind and the `uploadId`, and `fleet-defs.ts` the `disabledToolsets`
  field, as types only), and the manage-modal checkboxes that set the list.
