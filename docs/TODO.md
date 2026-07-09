# Talaria — backlog / TODO

Running list of remaining work, so nothing gets lost between pushes.
Status as of 2026-07-09. Newest product direction lives in the app; this is the
engineering-facing tracker.

## In progress
- **Agent reliability (#76)** — *active.* Robustness/QA layer for agent work.
  - ✅ **Talaria QA judge** — advisory LLM-as-judge at the ticket quality gate.
    On ticket→quality_review, a configurable model (org LLM gateway, metered)
    reviews the agent's reported outcome vs. requirements → verdict (pass /
    revise / escalate) + specific issues, surfaced above the human approve gate.
    Admin: enable + model. Per-board `judge_mode` (advisory/off) column + logic
    (board-settings toggle UI = quick follow-up). Verified live end-to-end.
  - ✅ **Enforcing revision loop** (opt-in per board, `judge_mode='enforcing'`) —
    a "revise" verdict auto-bounces the ticket to in_progress with the issues as a
    judge comment ("revision N/3"), bounded to 3 cycles then escalates (stays in
    quality_review for a human). pass/escalate always go to the human. Board
    settings → QA judge selector. Verified live.
  - ✅ **Confab guard (Talaria-native, #80)** — the Hermes confab-guard plugin,
    reborn as a configurable **gateway guardrail** (drop-in across model classes).
    3 structural checks (zero-tool claim / ungrounded ref / fabricated outage),
    tool record derived from the request messages (no trace export needed), modes
    off/observe(default)/annotate/strict, findings recorded out-of-band (zero
    added model tokens). Admin → Confab guard. Verified live (all 3 checks fired).
  - ⏳ **Confab guard follow-ups** — streaming-annotate on the public route;
    confidence scoring; layered structural→judge tiering; pluggable checks
    (PII/secret-leak); feedback-into-agent-memory.
  - ⏳ **Hermes self-review** (#78) — enable subagent-driven-development /
    requesting-code-review skills fleet-wide as the agent's first line.
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
  - ✅ **Org build targets** — admin-configurable per org connection: Shared Drive
    (team-owned agent files), calendar ID, and send-as alias for org mail.
    Multi-account / delegation = future setup path.
  - ⏳ **Cloud-storage connectors** — S3 behind the `storage_ref` abstraction.
  - ⏳ Wire attachments into more surfaces (tickets, chat) beyond KB docs.

## High-value, ready to pick up
- **Multiplayer Plan** — plans are owner-private today; Jon's direction: Plan is
  "iterating on a document with agentic coworkers" and needs multiple humans on
  one plan (shared plan conversations + the living doc, presence, mentions
  already notify doc readers). The biggest open product thread.
- **Comms follow-through** — unread indicators + message notifications for DMs
  (mention notifications exist; plain DM messages don't notify); consider an
  expand-chevron so an agent's threads are visible without selecting it.
- **Brain-routability health** — provider pools churn under no-train routing
  (qwen lost its US pool mid-day and chats silently froze pre-fix). Surface
  "brain unroutable" on agent cards / alerts by probing each agent's rendered
  model against the gateway registry.
- **QA judge template conformance** — ticket templates give the judge an
  objective rubric (are the skeleton's sections present and filled?); wire the
  resolved template into the judge prompt at quality review.
- **Per-agent Talaria toolkit (#58)** — *in progress.* Agents already have tickets
  (comment/triage/create/report/deps/time), artifacts (create/update/list/get),
  RAG `search_knowledge`, and Google. ✅ **Knowledgebase tools** now added:
  `list_kb_spaces` / `list_kb_docs` / `read_kb_doc` / `edit_kb_doc` — browse + read
  by effective audience, edit only where granted Editor (sharing stays human-only).
  ✅ **Channel tools** — `list_channels` / `read_channel` / `post_to_channel`;
  an agent participates in channels a human has added it to (gated on channel-
  agent membership; agent posts don't trigger other agents — no reply storms).
  ⏳ Remaining: a "Talaria toolkit" onboarding skill (lands with #78, Hermes-side)
  (overlaps #59).
- **Research view (#56)** — informs chats / plans / boards; when built, wire its
  output into the **activity brain** (closes the rest of #63).
- **RAG registry tail (#63)** — reranking over merged multi-collection results;
  research remains to index (plans + plan docs + relay summaries + chat
  distills now feed the activity brain; channels/tickets/comments were already
  done). A Retrieval admin view exists.

## Later
- **Roll volume isolation** — during a rolling replacement both slots briefly
  share the agent's state volume (benign for chat-serving agents; a heavy
  mid-write could theoretically conflict). Escalation if it ever bites:
  per-roll volume snapshot/clone.
- **Org profile depth** — tone/values fields on the org profile for fleet-wide
  voice consistency; feed the org line into channel/relay replies and the QA
  judge's context.
- **Explicit plan-template picker** — plan docs seed from the agent's bound
  template today; an explicit per-plan pick (like tickets have) is the missing
  half of the chain.
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
- **Comms (2026-07-09)**: Chat + Channels unified into `/comms` — #channels,
  **Relays** (named ad-hoc gatherings that Conclude → summary posted + indexed →
  archive), teammate DMs, agent DMs (fresh thread by default, threads nested in
  the sidebar). Distill-then-archive decay for idle agent chats
  (`TALARIA_CHAT_TTL_DAYS`). Header user-chip flyover.
- **Rolling agent replacement (2026-07-09)**: two compose slots per agent; a
  change brings the new container up on a fresh port, cuts over after health,
  drains, retires the old — org/config/MCP applies never kill a conversation.
  `proxyChat` holds-and-retries residual gaps.
- **Org identity (2026-07-09)**: Admin → Organization; muse generation and every
  rendered SOUL.md anchor agents to the business; saves propagate by rolling.
- **Plan view #55 (2026-07-08/09)**: plan conversations + side-by-side living
  plan document (a real artifact) with agent Sync, doc-aware + dependency-aware
  ticket drafting, @mentions, activity-brain indexing.
- **Ticket & plan templates**: org library (markdown skeleton + agent guidance),
  board bindings with default, per-agent overrides, resolution chain everywhere
  tickets/plan docs form (incl. bare ticket creation seeding).
- **Provider catalogs hardened**: live `/models` everywhere (pagination, bare
  arrays, Gemini id normalization; Perplexity, which has no catalog API, reads its live docs instead),
  live OpenRouter US no-train pool (no maintained lists), config saves
  auto-register picked models on their endpoint.
- Knowledgebase: Outline-style editor (slash menu, headings, block-escape),
  read/edit + fullscreen, autosave, nested tree, search, TOC, backlinks.
- KB sharing: visibility + viewer/editor roles for humans **and** agents, a
  Google-Drive-style share dialog, per-item settings menu, folder→doc
  permission inheritance with override, public viewer pages.
- RAG: two-brain (org + activity) + personal per-user brains; visibility-driven
  doc sync; channels/tickets/comments indexed; retrieval admin.
- Agents onboarding via PR #47 (guided import wizard); MM bot token dropped from
  the fleet render as agents move onto Talaria.
