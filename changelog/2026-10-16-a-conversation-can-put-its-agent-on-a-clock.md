- **A conversation can put its agent on a clock, and the answers come back
  into the conversation.** An agent's cron already existed, on the fleet page:
  you gave the agent a schedule and a prompt, it fired on time, and it answered
  itself. The turn went into the container's own session and reached no Talaria
  surface at all — `api_server` is a non-push origin, so the gateway had
  nowhere to deliver to, and the one hook that fires at the end of a session
  carries no text. A daily digest you could not read is spend nobody sees.

  A schedule created **from a thread** is that thread's standing request, so
  `POST /api/conversations/{id}/schedule` records the conversation that is
  waiting on the job, and every run of that job posts its final text back as an
  ordinary assistant turn in the same thread — the unread pill moves, the
  agent-reply bell rings for whoever was away, and the next question you ask
  reads the digest as context because it is a message like any other.

  **Two gates, asking different questions.** Seeing the thread answers 404 for
  a thread you cannot reach (a thread you may not read must not be confirmed to
  exist by the way scheduling it fails); then the agent's own manager rule,
  keyed by the model the thread names, because a schedule writes to the agent's
  scheduler and spends its budget on a timer. Being able to read a thread is
  not enough to make its agent work while nobody is watching.

  **The row is written only after Hermes reports a job id**, and if that write
  fails the job is taken back out of the container: a job with no row pointing
  at a conversation fires forever, finds no listener, and is dropped.

  **`conversation_schedules` is keyed `(agent_model, job_id)`**, not by job id
  alone — a job id is unique only inside one agent's own `jobs.json`, and the
  model is both how a conversation names its agent and what the agent's own
  authenticated call back carries. The listing keeps the two halves apart: the
  stored half (name, schedule, when, which job) is always there, and the live
  half (`enabled`, `state`, `nextRunAt`, `lastRunAt`, `lastStatus`) comes back
  null when the container cannot be read, so a rolling agent does not look like
  a vanished schedule.

  **The delivery door is agent-authenticated and idempotent on the execution.**
  `POST /api/agents/cron-output` takes the plugin's `cron:<job>:<execution>`
  task id, refuses anything outside that grammar, finds the listening
  conversation by `(the caller's own model, job)` — never by job alone, so one
  agent's key cannot post into another agent's thread — and clamps and
  secret-scrubs the text at the boundary, the same distrust `/api/agents/
  tool-events` shows the same plugin. The stored turn's marker names the
  execution rather than the clock, so a re-delivered run answers with the first
  message id and writes nothing.

  The `talaria-events` plugin grew a third hook, `post_llm_call`, and forwards
  **only** `platform == "cron"` turns: every chat and work-session turn already
  reaches the platform through the gateway's own stream, and reporting them
  here would double every reply. One bounded queue still serves both doors —
  an event now names its own path, defaulting to the tool-events one, so a tool
  event's URL, body and headers are byte-for-byte what they were.

  A schedule can be taken away again — `DELETE /api/conversations/{id}/
  schedules/{jobId}` removes the job from the agent and the listener row
  together, behind the same two gates; a slug rename carries the rows with
  the agent; a blank delivery is refused rather than landed as an empty
  turn; the listing reads by conversation through its own index.

  Verified: `cargo test -p talaria-agent-crons -p talaria-routes-comms -p
  talaria-routes-fleet -p talaria-fleet-render -p talaria-personal-agent` — 5
  / 13 / 21 / 20 / 5 passing (new: the task-id grammar's eight cases, the
  turn marker's spelling, the UTF-16 clamp at the boundary); clippy
  `-D warnings` clean over those plus `talaria-api-routes`; `bun run gate`
  green; `docs:api` regenerated (316 routes); migration replay + snapshot on
  a throwaway Postgres (427 statements); svelte-check 5476 files, 0 errors;
  `ui/src/lib/schedules.test.ts` 5 passing. **Live** (`#[ignore]` module
  `conversation_schedules_live`, 9/9 against a fresh database): a thread the
  caller cannot see is 404, an 81-character name is 400 and writes nothing,
  a member who does not manage the agent is 403, an agent with no running
  container answers the crate's sentence and never a 500, the stored
  schedule lists for its thread with the live half null, the agent door
  takes no session, a key-shaped span is scrubbed before it lands, an
  unheard or unparseable task id is refused, and a cron turn lands in the
  thread exactly once — the replayed delivery answers the first message id.

  **Not yet exercised end to end**: no run of this has gone plugin → route →
  thread against a real container firing a cron. The header button that
  opens this to a person lands separately; the client
  (`ui/src/lib/schedules.ts`) is in place and speaks the wire.
