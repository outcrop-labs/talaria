- **An agent can now ask, instead of guessing.** An agent that did not know
  which of two things you meant had exactly two moves: guess, or write a
  paragraph back and hope you noticed. Guessing is how an agent confidently
  works the wrong ticket; a paragraph is how a person ends up re-reading their
  own request to work out what the agent is unsure about. Neither is asking.

  The `ask` tool takes one question and **2 to 6 concrete answers**. It renders
  as a card in the stream — "Waiting on you", the question, the options as
  buttons — and clicking one sends its own words through the composer's send
  path, so the answer lands as **an ordinary message** and triggers the next
  turn exactly as typing it would.

  **There is no question table and no answer endpoint**, and that is the
  design. The transcript is the one place the agent actually reads, and the only
  place a person would look to find out what was decided; a row recording the
  answer separately would be a second source of truth about what was said, and
  the two would eventually disagree. "Still open" is derived the same way — a
  question with a user message after it has been answered, whatever words were
  used. When an answer genuinely needs to be auditable and must refuse a stale
  or unauthorised reply, the precedent is research's parked run
  (`runs::decide`), which files the question with the approvals machinery; a
  clarifying question in a chat is not that job.

  **Two options minimum.** One option is not a question — it is a statement
  with a button under it, and the person would click the only thing available.
  An *open* question needs no tool at all: the agent can ask it in its own words
  and the composer is always right there. So the refusal for one option names
  that move rather than just stating the rule. Six maximum, because past that
  the buttons stop being scannable and the agent is enumerating rather than
  working out what it needs to know. Two answers that **read alike** are refused
  too: the answer travels as the option's own words, so duplicates cannot be
  told apart by the person clicking or the agent reading.

  **The result sentence tells the agent to stop.** The one thing it must not do
  after asking is carry on and act on the guess it just admitted it could not
  make, so the tool answers with "end your turn now and wait for it rather than
  acting on a guess" rather than a bare `ok`.

  Verified: 12 tests in the new `talaria-agent-ask` crate (both option shapes —
  a bare string and an object with its own detail — the two-option floor and
  what its message points at, the six cap, duplicate labels, blank and
  oversized options refused *by position*, the chip's id being stable per
  question so a repeat refreshes and a different question joins it, and the
  stop sentence); 76 in `talaria-fitness-toolbox` (one new, driving the real
  validator and asserting a refused ask asked nobody anything); 11 in
  `talaria-chips`; 14 in `ui/src/lib/chips.test.ts`. `bun run gate` green;
  svelte-check clean; `bun run docs:api` regenerated.

  **Live, and the negative case is the convincing half.** GLM 5.3 Flash through
  Talaria's own gateway, handed `ask` alongside `create_ticket` and
  `list_boards` so the choice was real. Given two boards and "file a ticket for
  the flaky webhook retries" it **asked** — *"Which board should the flaky
  webhook retries ticket be filed on?"*, with the detail *"Retries sound like
  backend infrastructure, but if the webhooks are part of a Growth-owned
  integration it would belong on b-2"* — offered both real boards, and **did not
  file the ticket in the same turn**. Given one board and an explicit title, it
  went straight to `create_ticket` and asked nothing. Ask when it matters, don't
  when it doesn't.
