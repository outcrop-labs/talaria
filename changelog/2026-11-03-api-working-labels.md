- **Fixed: `GET /api/working` names its lead item correctly in four cases an adversarial review found reading wrong.**
  - A direct-message room was created with an empty name, so a streaming DM reply led Comms with a blank label. It now shows the other person, or the other members by name, using the rule the room index already uses, now one macro both reads share. A DM holding only you and agents had no name in either read: both now name it after its agents. That includes the room index, which showed a blank title for these rooms.
  - A thread with an empty title falls back to its agent the way a missing title does.
  - A research run whose discussion thread is streaming counted as one item but kept only the run's own stamp, so an older thread could lead Work. The run now takes the later of the two.

  Verified: `bun run gate`. Four new live-DB tests against a seeded worktree database, each a case the previous code got wrong:
  - a streaming DM leads with the other person's name;
  - a DM of you and two agents leads with the agents' names (it read blank before, and the index's own title expression on rc returns blank for the same room);
  - an untitled chat leads with its agent;
  - a run updated an hour ago, with its thread streaming now, leads over a work thread that streamed a minute ago, with count 2.

  The seven live tests in the module pass.
