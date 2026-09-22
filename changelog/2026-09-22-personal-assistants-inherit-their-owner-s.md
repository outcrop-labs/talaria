- **Personal assistants inherit their owner's reach for read + draft — with the
  grant friction gone and introspection in its place.** Boards were the one
  policy-gated hole: the board listing showed an assistant its owner's boards
  while the board interior 403'd it, and the only path onto a board was an
  editor's hand. Now a personal assistant adds *itself* to any board its owner
  can read (`POST /api/boards/{id}/agents/self` — one step, own row only,
  audited), removes itself the same way, and for boards the owner cannot see
  files a request (`/api/boards/{id}/agent-requests`) that lands in the
  editors' approvals queue as a sixth approval kind (`board_access`) with a
  one-press Approve/Decline card in Board settings → Agents — approve grants
  and closes atomically, decline notifies the requester's owner. The board's
  agent policy stays authoritative throughout; nothing here widens it.
