- **Long agent turns finish instead of dying as "· interrupted."** Diagnosed
  from a fleet deploy: a knowledgebase-build turn that ran past ten minutes
  was killed by the API's own request timeout — a total 600s ceiling on the
  agent stream — while the agent's container was still working; the partial
  reply was dropped, the row landed as a bare error the chat rendered as
  "· interrupted", and nothing retried. (Compaction was never the culprit —
  it happens inside the container and simply made the turn long.) The turn
  now ends only on SILENCE: frames flowing — deltas, tool progress,
  keep-alives — keep it alive however long the work takes, with a tunable
  idle ceiling (`TALARIA_AGENT_IDLE_SECS`, default 10 min, floored at 1) and
  a bounded wait only for the stream to open. Liveness throughout is the
  last write, not the row's age: a new `streamed_at` stamps every flush, and
  the stale sweep, the sidebar's working flag, and the generating pulse all
  read it — an hour-long turn no longer reads as dead at ten minutes. A
  stream that does die is retried once, ON THE SAME ROW, after a short
  backoff: the row is resurrected to streaming and the turn re-driven with
  its own failed attempt excluded from the history, and the chat visibly
  picks it back up ("↻ stream dropped — picking the turn back up…") rather
  than sitting on an error. The error itself is now an explanation — the
  row's content says what happened ("the agent's stream went silent for
  600s mid-turn", the container's own failure reason when it refuses)
  instead of a bare interrupted mark, and it survives reloads and reaches
  the next turn's history. A turn that dies twice stays down for a person
  to look at; an agent's refusal (the failure frame) is never retried.
