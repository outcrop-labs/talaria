# Talaria — backlog / TODO

Running list of remaining work, so nothing gets lost while we push on artifacts.
Status as of 2026-07-07. Newest product direction lives in the app; this is the
engineering-facing tracker.

## In progress
- **Artifact system (#54)** — *active.* Versioned work products with built-in
  hosting, viewing, and sharing.
  - ✅ Foundation: `artifacts` table, doc kind, sharing (reuses KB visibility +
    viewer/editor grants + `PermissionsModal`), versioning, public viewer.
  - ✅ Make official → markdown → knowledgebase → org brain pipeline.
  - ✅ Microsite kind — HTML editing + sandboxed live preview + public hosting.
  - ✅ Attach an artifact to anything (`artifact_links`; KB docs are the first
    consumer with an Attachments section).
  - ⏳ **File kind + uploads** — store binaries (reuse the uploads infra), host
    + download; the `storage_ref` column is ready.
  - ✅ **Sheet kind** — editable grid, CSV/markdown round-trip, public view.
  - ✅ **Google Workspace export** — per-user OAuth (offline) → push a
    doc/sheet/file into the owner's Drive as a native Google Doc/Sheet. See
    `docs/GOOGLE-WORKSPACE.md`. Agents export via `export_to_google_doc` acting
    as the artifact's human owner (identity proxy, #42).
  - ✅ **Drive import** — browse/search Drive → import as artifact (Docs→
    markdown, Sheets→grid, native→PDF file, else→file). `drive.readonly` scope.
  - ✅ **Calendar agenda** — Home agenda panel (upcoming events + quick create),
    `calendar.events` scope; hidden until Google is connected.
  - ✅ **Gmail** — Home Mail panel (recent mail + link out) + Compose/send as the
    user. `gmail.readonly` + `gmail.send` (restricted scopes).
  - ✅ **Agent identity proxy (Drive)** — personal assistants act as their owner,
    general agents share an admin-connected **org Google account** (Admin →
    Organization Google account). `resolveAgentGoogle()` binds an agent (by unique
    `model`) to a connection. Export routes to owner/org accordingly.
  - ✅ **Agent Calendar/Gmail (confirm-sends)** — an agent reads calendar/mail and
    DRAFTS events/emails; drafts queue as pending actions a human approves on Home
    ("Needs your approval") → executes. Personal assistant → its **owner's**
    Google (owner approves); general agent → the shared **org** Google (admin
    approves). MCP: `read_calendar` / `draft_calendar_event` / `read_recent_email`
    / `draft_email`.
  - ✅ **Personal-assistant isolation** — a personal assistant is usable ONLY by
    its owner (chat, listing, channels), so another user can't drive it to read
    the owner's account/context.
  - ⏳ **Cloud-storage connectors** — S3 behind the `storage_ref` abstraction.
  - ⏳ Wire attachments into more surfaces (tickets, chat) beyond KB docs.

## High-value, ready to pick up
- **Per-agent Talaria toolkit (#58)** — give each agent MCP tools + skills to
  work *inside* Talaria (comment on tickets, edit KB docs they're granted as an
  Editor, search knowledge). Leverages the agent-editor grants already shipped.
- **Plan view (#55)** — a first-class planning surface that turns conversations
  into reviewed tickets and feeds boards/chat. Self-contained.
- **Research view (#56)** — informs chats / plans / boards; when built, wire its
  output into the **activity brain** (closes part of #63).
- **RAG registry tail (#63)** — reranking over merged multi-collection results;
  index the remaining activity sources (plans / research; channels + tickets +
  comments already done). A Retrieval admin view exists.

## Later
- **Talaria identity proxy (#42)** — *first slice shipped:* per-user Google
  connection; agents export artifacts into the owner's Drive as that user. Next:
  extend the same per-user connection to more Google surfaces (Calendar, Gmail)
  and other connected tools.
- **Inference: full-stack monitoring + container controls (#48)** — live inference
  dashboard, restart/reboot containers, warm-up state.
- **WYSIWYG everywhere + modal editors (#46)** — finish converting remaining
  plain-text areas to the shared rich editor.
- **Input consistency sweep (#49)** — key-driven inputs everywhere; audit every
  form control for the same affordances.
- **Proactive agent outreach (#59)** — agents start conversations, comment on
  tickets unprompted, surface things they notice.
- **Universal @mentions (#60)** — core is in; finish coverage across plan /
  research / design surfaces as they land.
- **Design view (#57)** — future, broad scope.

## Done recently (context)
- Knowledgebase: Outline-style editor (slash menu, headings, block-escape),
  read/edit + fullscreen, autosave, nested tree, search, TOC, backlinks.
- KB sharing: visibility + viewer/editor roles for humans **and** agents, a
  Google-Drive-style share dialog, per-item settings menu, folder→doc
  permission inheritance with override, public viewer pages.
- RAG: two-brain (org + activity) + personal per-user brains; visibility-driven
  doc sync; channels/tickets/comments indexed; retrieval admin.
- Agents onboarding via PR #47 (guided import wizard); MM bot token dropped from
  the fleet render as agents move onto Talaria.
