- **An agent can now say what it is going to do, and where it has got to.**
  Talaria's agents carry 87 tools and had no way to narrate a plan for using
  them. So a request that takes several steps — "read the retry spec, draft the
  tickets, then update the doc" — looked from the outside like silence followed
  by an answer, and the one question a person actually has while waiting ("is it
  doing what I asked, and how far through is it?") had no answer anywhere in the
  product. The agent was deciding; nothing could see the decision.

  The `todo` tool takes a list of short steps, each `pending`, `active` or
  `done`. It lands as a **timeline in the stream** — done steps struck back,
  the active one carrying the same pulsing mark the rest of the app uses for
  work in flight — and it is readable back through
  `GET /api/conversations/{id}/todo`, gated on the conversation's own membership.

  **No conversation parameter**, and that is the same call the research tool
  already makes for the same reason: the agent is speaking in a conversation
  right now, Talaria knows which one, and a model that had to pass the id could
  pass a stale one, pass somebody else's, or forget. The best fix for a mistake
  is to make it unavailable. The write door is agent-key only and resolves the
  thread from the turn that is actually streaming; the read door takes an id and
  checks membership. Two doors, because they answer to different callers.

  **Replace, not patch.** Every call carries the whole list. A patch protocol
  (complete item 3, add one after 2) is more state for a model to track across
  turns and one more thing to get wrong, and it buys nothing — a plan is a
  handful of short lines. An empty list is how an agent clears it.

  **At most one step may be active**, and the refusal says so. The strip answers
  "what is it doing right now", a question with one answer or none; two active
  steps do not make it twice as informative, they make it unable to say
  anything. A blank step is refused by position rather than stored unreadable,
  and the caps (30 steps, 200 characters, counted in **characters** so an
  em-dash plan is not half the length of an ASCII one) keep it a plan rather
  than a backlog. It is deliberately **not** a ticket list: tickets are durable
  work a person signs off, and `plan_drafts` already turns a conversation into
  them.

  **One fix this needed in the chip merge.** `merge_chips` keeps the FIRST chip
  for an id, which is right for an approval — the same action landing twice must
  not duplicate it — and wrong for a plan, which is rewritten several times
  inside one turn. Keeping the first would have pinned the stream to a plan the
  agent had already moved past, which is worse than showing none, because it
  reads as current. Snapshot kinds are now replaced in place (keeping their
  position in the row rather than jumping to the end); event kinds are
  untouched.

  **And one real fix to the toolkit's own guard, found by writing this.** The
  catalogue-sync scanner decided whether an argument was required by looking for
  `.optional()` anywhere in its expression — so a nested optional
  (`items: z.array(z.object({ status: ….optional() }))`) read as "`items` is
  optional". That let the catalogue declare a **required** argument optional,
  which is exactly the flattery those tests exist to stop: a fixture could omit
  it, pass the benchmark, and 400 in production. The scan is now depth-aware,
  and `expose_tools` — which had the same shape and the same wrong answer —
  declares its real required set for the first time.

  Verified: 12 tests in the new `talaria-agent-todo` crate (statuses and their
  defaults, the one-active rule, blank steps refused by position, both caps with
  the character-vs-byte case, trimming, the summary sentence, and the wire
  round-trip that would otherwise serialize a status as `"Pending"` and both
  render wrong and fail to load back); 75 in `talaria-fitness-toolbox` (one new,
  driving the real validator through the sandbox: a plan written, replaced
  wholesale with a step dropped, two refusals that each leave the stored plan
  untouched, and an empty list clearing it); 10 in `talaria-chips` (two new, on
  the snapshot-vs-event merge); 10 in `talaria-fitness-talaria-tools`, one new
  and pinning the depth rule from both sides — a nested optional is not the
  key's, and a top-level one still is. 1488 ui tests pass with 2 added;
  svelte-check clean, which is how the plan's pulsing mark got a row in the
  waiting-site registry instead of a silent fallback. `bun run docs:api`
  regenerated (both routes classified `agent` and `session`, no `unknown`), and
  the schema snapshot ratcheted — its diff is the one new table and nothing
  else.
