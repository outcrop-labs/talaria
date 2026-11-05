- **Changed: a live ticket shows what its agent is doing now, on the board and in the ticket.** A work session used to say `turn 2 of 12 — waiting on doug-engineering` from the moment the turn was sent until it ended. On the owner's instance that was the same for a session forty minutes into a build and one its agent had not started, and the ticket strip printed the turn twice. Each watch-tail frame is now stamped with its arrival time, and each turn records when it began. The ticket and board reads return the session's live status:
  - whether it is running a step, thinking, quiet, queued, waiting for a person, or still waiting for the agent to take the turn. The run row decides first, so a queued run never reads as sent, and a run picked up again after a restart never shows the dead turn's step. A status that could not be read says "unknown".
  - the step and how long it has run, the last output or sentence, and recent failures with the last one's text.
  - the tokens and cost booked to the ticket since the session started. A partly priced total says what it leaves out (`$1.20 + 3 unpriced`).

  When nothing is booked to the ticket, the read says why, and only when the reason belongs to this agent in this session's window: its own calls carried no session tag (Hermes before v0.22), or none landed under its name while shared-key calls arrived. Hermes crons are not counted.

  Board cards and list rows show the same in two lines. A queued ticket names the tickets it waits behind only on boards the reader can see, and counts the rest. A failed session's row in the Work log shows why it ended. A workbench job shows its provisioning step, or the step it failed at and why, from the `provision` the job read returns once the workbench provisions jobs itself (#706). Board polls reuse a session's pulse while its tail is unchanged, or read within the last 2 s. Everything comes from the server, so a refresh or a second tab shows the same.

  Verified: unit tests for the tail reader cover the shapes on the owner's instance:
  - a terminal call inside an execute_code cell, and a build killed at the 420-second ceiling;
  - the events plugin reporting a call before the stream does;
  - parallel calls, where one finishing leaves the other as the step;
  - a plugin-only tail, a cut window, and a refused workbench tool;
  - a command that mentions `"at":`.

  Unit tests also cover the cache rule (same turn and length, or under 2 s), the state word against the run row (queued, re-queued with a dead step, awaiting), each attribution reason and when it stays silent, the behind split, and the labels including partial pricing. A live-database test (`work_wait_behind_live`) puts one agent's sessions on two boards with different members and checks each reader sees only their own board's ticket by name. `svelte-check` reports 0 errors.

  The first version was exercised on a worktree stack with a real Hermes v0.22.4 Developer Agent and a mock model. With three, then six, live tickets, the board read and the screenshots showed each one `running` with its own shell step and clock, 5.3k tokens booked to each ticket, and `quiet` with its age while the agent was slow to take turns. After a failed turn, the Work log row showed the session's error.
