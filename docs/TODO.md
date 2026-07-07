# Talaria — backlog / TODO

Running list of remaining work, so nothing gets lost while we push on artifacts.
Status as of 2026-07-07. Newest product direction lives in the app; this is the
engineering-facing tracker.

## In progress
- **Artifact system (#54)** — *active.* Versioned work products (doc / sheet /
  microsite / file) with built-in hosting, viewing, and sharing; cloud-storage
  connectors (S3 / Drive); attach an artifact to anything; "make official" →
  translate to markdown → store in the knowledgebase → org brain. Reuses the KB
  sharing model (visibility + viewer/editor grants + folder-style inheritance),
  the `PermissionsModal`, and the public-viewer pattern.

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
- **Talaria identity proxy (#42)** — per-user MCP identity (Google Workspace), so
  agents act *as* the user against connected tools.
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
