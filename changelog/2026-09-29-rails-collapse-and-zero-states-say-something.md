- **The rails collapse, and a surface with nothing on it says what goes there.**
  Work, Plan and Research all put a 288px list rail beside a stage, and all
  three kept it whether or not you wanted it. Each rail now has a collapse
  toggle in its header and remembers the choice per surface, so a plan's
  document or a research report can have the width; collapsed, it stays a
  narrow strip that names sideways what it is holding, and one click on any
  part of it brings the list back.

  The zero states stopped being dead ends. A rail with no rows said "No plans
  yet with this agent." and offered nothing — it named the absence and stopped.
  Work and Plan now say what a session or a plan actually IS and offer the
  button that starts one, and the surface with no agents at all says so with a
  way to hire one rather than the bare sentence "No agents available." (the
  button appears only for someone who can reach Agents, because offering one
  that bounces is worse than offering none). The document column beside each
  conversation gets the same treatment, which matters most there: it owns 44%
  of the stage and was three centred words in an empty pane until a file
  existed.

  Research already used the shared zero state and keeps its own copy; it gains
  the collapse. Its launcher is untouched — that pane is a real question box,
  not an empty state pretending to be one.

  Verified: `bun run check` green; `bun run typecheck` 0 errors (2 pre-existing
  a11y warnings in WorkchainCanvas, untouched); 1321 ui tests pass.
