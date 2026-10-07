- **An agent in a channel can now answer a message that did not @mention it,
  when a judgment says the message is for that agent in particular.** The
  asymmetry this fixes: in a ticket's task room the assigned agent answers
  whatever the relevance gate says is for it, but in a group channel an agent
  spoke only when named. That rule was right — the alternative had been chatter
  — but it meant an agent that could have answered the question in #eng sat
  there silently unless somebody remembered it existed. The gate was never
  affordable to run on every message in every channel. At a typed yes/no in
  under a tenth of a second it is.

  **This is the highest-risk site on the port**, and it is the only one that
  makes Talaria do something it could not do before rather than doing an
  existing thing better. The design is shaped by the failure mode:

  - **AT MOST ONE AGENT SPEAKS.** If three agents clear the floor on one
    unaddressed message, three agents reply and the room stops being usable by
    people. The highest-probability candidate speaks and the rest stay quiet.
    This is the difference between "more interactive" and "a room full of
    bots", and it is not a tuning decision — ties go to the caller's own order,
    which is deterministic, so two agents never take turns at random.
  - **The floor is 0.90 and it LEANS** — the probability of yes, never distance
    from the middle. Read as certainty, a confident *no* would clear the floor
    and the agent would speak, which is the exact inversion that matters here.
    It is the highest floor in the census, and a test asserts that rather than
    leaving it as a coincidence: a missed answer is an agent staying quiet,
    which is what happens today and what everyone is used to; a wrong yes is a
    bot interrupting people, which is the failure that makes a workspace turn
    the whole feature off.
  - **Every room can opt out.** `channels.agent_initiative`, with a switch in
    channel settings beside the agent picker. It defaults **on** so that
    enabling the capability is one deliberate act rather than one switch
    followed by an invisible second step — and the workspace-wide site is off
    on every install, so nothing changes until somebody turns it on with the
    warning in front of them.
  - **An agent with no `role` is not asked about.** The remit is the only thing
    that distinguishes one candidate's question from another's, so a yes/no
    over a bare model id is a coin flip dressed as a decision — the same rule
    the workflow pass holds for a hook with neither name nor description.
  - **Eight candidates at most.** One question per candidate in a single
    request, so this is a cost; past a handful the answer to "which of these
    should speak" is "a person should pick".

  **It is not a loop, and that was checked rather than assumed.** An agent's
  reply is written through `insert_channel_message`, not through the POST
  route, so it never re-enters `trigger_agent_replies` and cannot answer
  itself. `channel_initiative` also refuses a message with no human sender —
  belt and braces for a future caller. A person replying to an agent that spoke
  unprompted is a conversation, which is the point.

  Every judgment is recorded under `channel-speech`, including the ones below
  the floor, with a baseline of `false` because silence is what the existing
  rule chose for every one of these messages. Worth reading carefully: a high
  agreement rate at this site means "it would rarely have spoken", not "it is
  right" — the number to look at is the mean certainty when it disagreed.

  With the site off — the default — `channel_initiative` costs one settings
  read and the `return` that was there before.

  Verified: 89 tests in `talaria-decide` (4 new — the site leans and holds the
  highest floor in the census, an agent with a blank remit or model is dropped,
  the subject names the room and keeps an empty history empty rather than
  omitting it, and twenty candidates are capped deterministically with the
  first two by the caller's order; that last one is asserted as BEHAVIOUR over
  real candidates because comparing the constant with a literal is a
  compile-time tautology clippy rejects); 13 in `talaria-routes-comms`. clippy
  clean across `talaria-decide`, `talaria-channel-replies` and
  `talaria-routes-comms`; `bun run typecheck` 0 errors; `bun run gate` green.
  The migration is one `alter table channels add column` and the schema
  snapshot was regenerated against a scratch `postgres:16-alpine` — a one-line
  diff, with the replay confirmed idempotent (`applied: 0, schema matches
  snapshot`).

  **Not exercised live.** No real judgment has decided whether an agent speaks
  in a real channel. The floor is reasoned from the asymmetry, not measured,
  and this is the site where that matters most — read `channel-speech`'s rows
  before lowering it.
