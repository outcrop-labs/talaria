- **A personal assistant can work the boards it is told it owns**: a board is born
  carrying its creator's personal assistant on the agent allowlist, and a new assistant is
  seeded onto the boards its owner already has — in both cases inside the same breath as
  the creation, so there is no window where `list_boards` shows the board under the
  owner's role while every board-scoped route 403s it. Boards the owner does not own stay
  the board owner's call, and a removal via `set_board_agents` is never re-added: the
  propagation lives in the seeding, not in the gate.
