- **Skills and tool text now name the address the routes accept.** A ticket
  is its `id` or its ref (`PLAT-118`); a bare number is not an id. That
  sentence is on every ticket door, on `start_job`, and in the toolkit and
  workbench skills. `list_tickets` says each row has both fields, and its
  parent filter matches either. `create_ticket` no longer offers `parentId`
  — the API drops it for an agent, and the description used to promise a
  sub-task. `report_gap` and `report_problem` resolve a ref before the
  board check, so the assignment title is not a 400. A comment 409 that
  the ticket has no owner for its room is named in the tool and the skill:
  report it once, don't retry. The toolkit skill no longer points at
  `requesting-code-review`, which does not ship. Verified: `cargo check` on
  the workbench and fleet route crates; `bun run check`.

### Fixed
