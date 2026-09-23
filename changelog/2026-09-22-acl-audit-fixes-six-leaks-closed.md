- **ACL audit fixes — six leaks closed.** (1) `/api/uploads/:id` streamed
  any file by id; now gated by `canAccessUpload` — owner, admin, or
  reachable through a conversation/channel/board/artifact the viewer can
  actually read (agents resolve through their board/channel grants or
  their owner's chats). (2) `/api/history` served full snapshot bodies for
  any key; now enforces per-kind ACLs (artifact + KB perms incl. space
  inheritance and editor grants, memory/skill by agent ownership,
  templates admin-only). (3) `/api/memory/:id` GET let any user read any
  agent's memory; now admin-or-owner like PUT. (4) KB search matched raw
  row visibility, ignoring space inheritance and grants — a doc in a
  private space leaked into results; now filters through the same
  effective-permission check the read routes use. (5) Artifact link
  DELETE had no read gate (POST did). (6) Research list/get had no owner
  predicate at all — everyone saw everyone's runs.
