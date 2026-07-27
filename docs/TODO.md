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
  - ✅ **Confab guard follow-ups (2026-07-15)** — annotate/strict are real:
    findings pin to the flagged message (`messages.guard` /
    `channel_messages.guard`, caveat rendered in chat + channels), the public
    route appends the caveat non-streaming and injects a final SSE delta
    before `[DONE]` streaming (never for agent-loop keys — no context
    contamination), and strict redacts detected secrets from persisted /
    not-yet-relayed content. Confidence scoring, structural→judge tiering
    (`guardText` feeds the QA judge), and the secret-leak check had already
    shipped.
  - ✅ **Guard tail (2026-07-18)** — `pii_leak` check (SSN / Luhn card / IBAN;
    strict also redacts PII) and opt-in coaching: repeated findings (≥2/check
    per 7 days) become templated notes in the agent's rendered soul — counts +
    fixed advice only, flagged content never re-enters model context. Admin →
    Confab guard → "Coach agents from findings".
  - ✅ **Hermes self-review (#78) (2026-07-27)** — the skills plumbing bug
    fixed (rendered configs now carry `skills.external_dirs` — before this,
    Hermes never scanned the `/opt/skills` / `/opt/dept-skills` mounts at
    all); `subagent-driven-development` vendored into `scripts/skills/`
    (bundled `requesting-code-review` was already enabled); toolkit skill +
    soul header teach self-review-before-`report_outcome`. Seeding upgraded
    to pristine tracking (`.seeds.json`): un-edited copies follow canonical
    updates, admin edits never clobbered.
- **Artifact system (#54)** — *active.* Versioned work products with built-in
  hosting, viewing, and sharing.
  - ✅ Foundation: `artifacts` table, doc kind, sharing (reuses KB visibility +
    viewer/editor grants + `PermissionsModal`), versioning, public viewer.
  - ✅ Make official → markdown → knowledgebase → org brain pipeline.
  - ✅ Microsite kind — HTML editing + sandboxed live preview + public hosting.
  - ✅ Attach an artifact to anything (`artifact_links`; KB docs are the first
    consumer with an Attachments section).
  - ✅ **File kind + uploads** — verified working end-to-end (2026-07-09):
    create file artifact → multipart upload → attach `storage_ref` →
    authenticated download + public-slug download. (Landed with the
    image-artifact/Drive-import work; this entry was stale.)
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
  - ✅ **Per-agent filing cabinets (2026-07-10)** — auto-created artifacts file
    under "<Agent>/<Category>" (Plans / Research / Documents / Media / Chat
    summaries, find-or-create); chat distills become private artifacts too.
  - ✅ **Cloud-storage connectors (2026-07-17)** — any S3-compatible bucket
    (AWS/Backblaze B2/R2/MinIO) behind uploads via hand-rolled SigV4
    (`server/storage.ts`); Admin → Storage config (secretbox-sealed secret),
    connection test, background local→bucket migration. Per-row path dispatch:
    switching modes never strands a file. Plus a **built-in bucket** (bundled
    MinIO container, `TALARIA_S3_*` env, auto-created) and an optional
    **replica** to a second provider: mirror-on-upload, "Sync all" backfill,
    and read fallback when the primary can't serve a blob.
  - ✅ **Ticket attachments (2026-07-17)** — files + KB/artifact ref chips on
    tickets (same `attachments` jsonb shape as messages), attach/remove in the
    ticket detail, activity-logged; agents read metadata via the task API and
    fetch bytes from `/api/uploads/:id` with the fleet key. Chat already had
    attachments. Follow-ons shipped same day: `fetch_attachment` toolkit tool;
    channel replies hand images to agents (data-URL blocks, 1:1-chat parity);
    textual uploads contribute contents to prompts in chat + channel paths.

## High-value, ready to pick up
- ✅ **Elevated admin assistants (2026-07-09)** — admins can promote an admin's
  personal assistant to org-wide view/edit: all boards (editor-level, incl.
  governance), all non-DM channels, implicit editor on org-visible KB docs +
  artifacts. Never: DMs, others' private items, owner-only actions. Only
  effective while the owner is an admin; audited. Admin → Users toggle.
- ✅ **Multiplayer Plan (2026-07-09)** — `conversation_members` + member-aware
  access across chat/doc/draft/sync; share by email (header avatars + picker),
  auto doc editor grants (revoked on leave), author names on turns (UI + agent
  transcript), presence rings (Redis TTL), "Shared with you" sidebar section,
  share notifications with `/plan?p=` deep link.
- ✅ **Comms follow-through (2026-07-09)** — per-member read cursors → unread
  badges on channels/relays/DMs (open = read, live); plain DM messages notify
  (deduped while unread, `/comms?c=` deep link); agent thread expand-chevron.
- ✅ **Brain-routability health (2026-07-09)** — `fleetBrainHealth()` probes
  every enabled agent's main/tier/fallback targets against the gateway
  registry (30s cache); critical alert + red card chip for unroutable mains,
  warning + amber chip for dead tiers/fallbacks.
- ✅ **QA judge template conformance (2026-07-09)** — the judge resolves the
  ticket's template (assignee binding → board default) and scores against its
  sections as an objective rubric; missing/skeleton sections are named
  "revise" issues.
- **Per-agent Talaria toolkit (#58)** — *in progress.* Agents already have tickets
  (comment/triage/create/report/deps/time), artifacts (create/update/list/get),
  RAG `search_knowledge`, and Google. ✅ **Knowledgebase tools** now added:
  `list_kb_spaces` / `list_kb_docs` / `read_kb_doc` / `edit_kb_doc` — browse + read
  by effective audience, edit only where granted Editor (sharing stays human-only).
  ✅ **Channel tools** — `list_channels` / `read_channel` / `post_to_channel`;
  an agent participates in channels a human has added it to (gated on channel-
  agent membership; agent posts don't trigger other agents — no reply storms).
  ✅ **Toolkit ATTACHED (2026-07-09)** — talaria-mcp fleet HTTP mode
  (per-request identity, fleet-key auth), self-hosted by the app, injected into
  every rendered config; visible + probeable as the "built-in" server on the
  MCP tab. ✅ **Onboarding skill (2026-07-17)** — fleet-wide `talaria-toolkit`
  skill (canonical in `scripts/skills/`, seeded on render, `/opt/skills`
  mount now wired) teaching the reflexes; `fetch_attachment` tool (text
  inline, images as MCP image blocks, binary = metadata); soul header points
  at both. Running agents need one compose recreate for the new mount.
- ✅ **Research view (#56) (2026-07-10)** — `/research` with Recon / Brief /
  Expedition modes: server-side pipeline (agent persona plans/gap-checks/
  writes; sonar search stages via the org gateway), inline [n] citations
  against a deduped source registry, org-visible doc artifacts, activity-brain
  indexing (the #63 research-indexing piece), completion notifications, and
  `research`/`research_status` MCP tools for every agent. Reranking remains
  (#63 tail — a Model Roles slot is reserved).
- ✅ **Model Roles (2026-07-10)** — /models panel assigning a model per
  activity class: per-tier research roles (Recon / Brief / Expedition — the
  sonar family maps one-to-one; deep-research-class assignments shrink the
  engine's own loop) + utility wired (research engine, systemModel,
  museModelFor); vision / image-generation / embeddings / reranker slots
  reserved. Assignments only win while routable.
- ✅ **RAG registry tail (#63) (2026-07-10)** — retrieval plane resurrected
  (Qdrant + native TEI embeddings in compose; they'd been dead since the
  Phase-7 stack cut): unreachable-service alerts, one-click backfill, 15-min
  incremental self-healing sweep. Reranking shipped as a provider registry
  (TEI self-hosted, Voyage/Together/NVIDIA/Pinecone US, Cohere CA, Jina DE;
  live model catalogs, sealed keys, fallback-to-vector-order). Brains curatable
  in Admin → Retrieval: team/user/agent bindings + KB-space→brain feeding.
- ✅ **Hybrid retrieval + guided reindex (2026-07-15)** — every brain indexes
  sparse (IDF bag-of-terms, identifiers kept whole) alongside dense; searches
  fuse both via RRF, so exact names/env vars/error strings rank with meaning.
  Embedding-model swaps are detected live (TEI /info + actual Qdrant shape,
  never the registry — it had gone stale once), alerted, and repaired by a
  one-button rebuild-from-sources in Admin → Retrieval. Verified live on the
  dev brains (v1 384d → hybrid; identifier + paraphrase queries both rank #1).

- **MCP toolkit gaps (from the 2026-07-10 coverage audit)** — deferred,
  design questions attached: agents starting Relays (whose channel is it?);
  read_plan (plans are member-scoped to humans); ticket search by title
  (cross-board); ticket watching/subscription for agents; agent
  self-scheduling follow-ups (crons are human-managed today); KB doc re-filing
  (move within tree). search_knowledge's description over-promises "plans".

## Later
- **Roll volume isolation** — during a rolling replacement both slots briefly
  share the agent's state volume (benign for chat-serving agents; a heavy
  mid-write could theoretically conflict). Escalation if it ever bites:
  per-roll volume snapshot/clone.
- **Org profile depth** — tone/values fields on the org profile for fleet-wide
  voice consistency; feed the org line into channel/relay replies and the QA
  judge's context.
- ✅ **Explicit plan-template picker (2026-07-20)** — Plan surface header picker
  for new plans; the pick persists on `conversations.plan_template_id` and is
  the top link in `resolveTemplate('plan', …)` (explicit → agent binding →
  none). Seeds the living doc verbatim + shapes agent rewrites. Verified live.
- **Talaria identity proxy (#42)** — *first slice shipped:* per-user Google
  connection; agents export artifacts into the owner's Drive as that user. Next:
  extend the same per-user connection to more Google surfaces (Calendar, Gmail)
  and other connected tools.
- **Inference: full-stack monitoring + container controls (#48)** — live inference
  dashboard, restart/reboot containers, warm-up state.
- **WYSIWYG everywhere + modal editors (#46)** — finish converting remaining
  plain-text areas to the shared rich editor.
- ✅ **Input consistency sweep (#49) (2026-07-20)** — shared `submitOnEnter` +
  `inlineEditKeys` helpers (`ui/control.ts`); 16 gaps fixed across inline
  renames (Enter commit / Escape revert, shielded from modal close),
  field+button Enter-submits, the home compose dialog reparented onto the
  shared Modal, and missing modal autofocuses. ~80 controls / 33 files audited.
- ✅ **Proactive agent outreach (#59) (2026-07-27)** — `message_user` MCP tool
  (agent→human chat conversation + inbox notification; owner-only for
  personal assistants, per-pair daily caps) and the opt-in check-in sweep
  (`server/outreach.ts`, throttled-kick pattern): each proactive agent
  periodically reviews its stale/blocked work through its own persona
  gateway and acts via its normal governed tools. Admin → Proactive
  outreach (master switch off by default, per-agent flags, caps, event
  log). Verified live end-to-end.
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
