- **One Hermes session per conversation.** Every Talaria agent is a Hermes
  agent, and Hermes has its own memory — turn-start compaction, `session_search`
  across past sessions, `/compress` — all of it keyed on the *session* a turn
  lands in. Talaria never got to use any of it, because of how a turn reached
  the agent: the whole transcript, re-sent as one `/v1/chat/completions` call,
  which Hermes files under a session id derived from *this turn's* system
  prompt plus the first user message. Talaria's system prompt moves between
  turns (the handle note is conditional; the sender block changes per speaker
  in a shared plan), so one conversation quietly became several Hermes sessions,
  each holding a fragment, and compaction and recall saw pieces of a thread
  instead of the thread.

  An agent can now be **pinned**: a per-agent switch on its Summary tab, next
  to Developer Agent. A pinned agent is driven through Hermes' session API —
  one session whose id *is* the conversation id, created on the first turn,
  then one new message per turn with the mode prompt, sender block and handle
  note riding as the turn's ephemeral system message. Hermes holds the history.
  The session stream's events map onto the same tool rows, reasoning and usage
  the chat path already persists, and the run id Hermes names on `run.started`
  lands on the assistant row (`messages.hermes_run_id`) — the handle a stop or
  a mid-turn steer will need, since today a stop only aborts the client's read
  and the run keeps going.

  **A conversation that ran before the flip stays on the full-history path.**
  Hermes never saw that transcript; a turn sending only the new message would
  answer from an empty room. The row itself says which it is — an assistant row
  with a run id was pinned-driven, a conversation with no assistant rows is
  fresh, and assistant rows without one are the legacy transcript — so the flip
  takes effect for new conversations and costs nothing in the container: no
  roll, no restart, the next turn simply takes the other door.

  **Two Hermes toolsets are now off in every agent**, pinned or not:
  `agent.disabled_toolsets: [todo, clarify]` in the rendered config. Talaria's
  own `todo` and `ask` are the ones that land chips and reach a person;
  Hermes' `clarify` has no callback on the API platform and comes back
  undelivered, and two todo tools leave a model guessing which list is real.

  **Two things the live run exposed, fixed on the way:**
  - The gateway's upstream-error boundary turns a provider's 400 into
    "upstream error (400)" on purpose — provider prose never crosses. But one
    refusal has to still *read* as what it is: a model whose thinking is
    always on refusing a request that asked for none (Z.ai's GLM-5.3). Hermes
    recovers from that on its own — steps its effort up and retries — if the
    message says so. The boundary now classifies that refusal structurally (a
    reasoning field named next to "cannot be disabled" / "mandatory") and says
    it in its own words; the provider's sentence still stays on this side.
    Before this, every turn on such a model read as a malformed request and
    the agent gave up.
  - `scripts/cleanup-sweep.mjs` read a worktree's fleet project
    (`talaria-wt-<name>-fleet`, the api's `TALARIA_FLEET_PROJECT`) as the
    stack of a worktree named `<name>-fleet`, found no such checkout, and the
    stop gate removed a live agent container at every turn boundary. The
    `-fleet` suffix is the worktree's own now, as it already was for devboxes.

  Verified: 74 tests in `talaria-gateway` (+8: every session event name
  through the parser in realistic chunking — a frame split across two feeds,
  two frames in one chunk, a trailing frame with no blank line — a reply that
  never streamed still landing as content, usage staying silent when the
  frame carries no counts, a started/completed pair folding into one tool
  row, Hermes' real run-verdict shapes including a failed run ending the turn
  as an error, and the sealed session body); 125 in `talaria-chat-persist`
  (+4: the fold of a full-history message list into the ephemeral system
  message and the trailing run of user rows, including an image turn's parts);
  22 in `talaria-fleet-render` (+2: the suppression adds, never replaces, and
  round-trips the YAML emitter); 9 in `talaria-error` (+1); 21 checks in the
  sweep (+1). `bun run gate` green; svelte-check clean; `bun run docs:api`
  regenerated; migration snapshot updated (427 statements).

  **Live, against a real hired Hermes agent** (v0.21.4, GLM-5.3 Flash via
  Z.ai through Talaria's gateway) on a fresh worktree stack: the rendered
  config carries the two disabled toolsets; `POST /api/sessions` answers 201;
  turn 1 "Remember the word 'quokka'" landed `OK` with its run id; turn 2
  "What word did I ask you to remember?" — sent to Hermes as *only that
  message* — answered **`quokka`**, with a second run id, and Hermes'
  `GET /api/sessions/{conversation}/messages` held both user turns in one
  session. Unpinned, a new conversation's turn landed with no run id; after
  re-pinning, its next turn stayed on the full-history path (no run id, still
  recalled "vermilion"), Hermes had no session under that id, and a fresh
  conversation took the pinned door.
