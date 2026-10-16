- **Conversations can be pinned, grouped and ordered — by each person, for
  their own rail — and an agent can file the session it is in.** The rail
  was one list, newest activity first, and the only organisation it knew was
  Archive. A plan you keep coming back to sank under every new chat.

  Three things a row can now carry, per viewer: **pinned** (first in the
  list), a **group** (a short name, 1–40 characters), and a **position**
  (an explicit order among the rows that have one). They live in a new
  `conversation_layout` table keyed by *(viewer, conversation)*, not on the
  conversation — a shared plan sits in every member's rail and each of them
  organises their own; the owner's pin is not the collaborator's. The list
  left-joins the viewer's row and orders `pinned, position, then newest
  activity`; `PATCH /api/conversations/{id}` takes the three (`null` clears
  a group or a position), and a row whose three values are all default is
  deleted rather than kept.

  **`organize_session`** — the agent's door, with no conversation parameter
  for the same reason `todo` has none: the agent is speaking in a
  conversation right now and Talaria knows which. `{title, pinned, group,
  archived}` → `POST /api/agent/organize` renames, pins, files under a group
  or puts away *this* conversation, writing the **owner's** layout row, so
  "move this to Done when you're finished" is a sentence an agent can act
  on. Never archive unasked — the description says so, and that sentence is
  the only guard behind it. The full tool contract rides along: the MCP
  registration, the catalogue entry (85 tools), the sandbox arm, the
  exposure list.

  The rail itself — pinned first, collapsible groups, drag to reorder, a
  ⌘K palette over sessions — lands on top of the merged Work harness (#541);
  this is the server half it stands on, and the wire already carries the
  three fields on every row.

  Verified: 5 tests in the new `talaria-conversation-layout` (bounds, trim,
  absent-vs-cleared, an empty patch refused, a non-boolean pin refused); 10
  in `talaria-fitness-talaria-tools` (the catalogue carries every tool
  exactly once, at 85); 77 in `talaria-fitness-toolbox` (+1: the sandbox
  organises the session and refuses a call that asks for nothing); 13 in
  `talaria-routes-comms`; `bun run docs:api` regenerated (313 routes);
  migration snapshot updated (426 statements); svelte-check and the MCP
  typecheck clean. Live, against the worktree database (`#[ignore]` module
  `conversation_layout_live`, 5 tests): pinned-then-position-then-recency
  ordering with the three keys on every row; a patch merges rather than
  replaces and an all-default row is deleted; a collaborator's pin is
  invisible to the owner; 41-character and blank groups, `-1` and `100001`
  positions and an empty body are 400s, a stranger 404; an agent mid-turn
  renames and groups the conversation through the door and the owner's list
  shows both, while an agent with no live turn is told 409.
