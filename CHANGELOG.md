# Changelog

All notable changes to Talaria. Milestone labels refer to [`PLAN.md`](./PLAN.md).

## [Unreleased]

### Added
- **Toolkit onboarding — agents get the playbook, not just the tools.** The
  talaria MCP was attached to every agent but nothing taught them when to
  reach for it. Now a fleet-wide `talaria-toolkit` skill (seeded from
  `scripts/skills/` on render, admin-editable after, mounted read-only at
  `/opt/skills` — a mount that was documented but never actually wired) walks
  the reflexes: search before planning, keep the ticket alive, durable output
  goes in Talaria, drafts await approval, report_problem on breakage. The
  rendered SOUL header points at it.
- **`fetch_attachment` toolkit tool.** Agents can now READ the files attached
  to tickets and chats: text formats come back inline (clipped at 50k chars),
  images arrive as real MCP image blocks the model can see, and other binary
  formats report honest metadata instead of pretending. `get_ticket` now
  advertises the attachments array. Verified live: fleet render seeds skill +
  mount, MCP serves the tool, text/image/binary/404 behaviors all correct,
  11/11 checks.
- **Object storage — built-in bucket, bring-your-own, or both.** Upload blobs
  can now live in a real S3-compatible bucket instead of local disk, three
  ways: the **built-in bucket** — a bundled MinIO container (dev-compose
  `minio` service, creds via `TALARIA_S3_*` env, bucket auto-created) so you
  get durable object storage with no cloud account; any **external**
  S3-compatible service (AWS S3, Backblaze B2, Cloudflare R2, MinIO) via
  endpoint/bucket/keys in Admin → Storage (secret sealed by secretbox); and an
  optional **replica** that mirrors every new upload to a second provider as
  it lands (fire-and-forget — a replica outage never blocks an upload), with a
  "Sync all" that backfills everything already stored and automatic read
  fallback to the mirror when the primary can't serve a blob. The client is a
  hand-rolled SigV4 signer over fetch — no SDK. Each upload's row records
  where ITS bytes live (`s3+internal://` / `s3://` / filesystem path), so
  switching modes never strands a file. Connection tests do a real
  write/read/delete round-trip; a background migration moves local files into
  the active bucket. Verified live 22/22 across two runs: external-bucket flow
  against a throwaway MinIO, then built-in mode with auto-created bucket,
  replica mirror-on-upload, full sync, and replica fallback after deleting the
  primary object.
- **Ticket attachments.** Tickets now carry the same attachment chips as chat
  messages: uploaded files plus knowledge-doc/artifact refs (ACL-checked
  against the attacher, content clipped into the chip for models). Attach and
  remove from the ticket detail; changes log to the ticket's activity. Agents
  see attachment metadata in `GET /api/tasks/:id` and can now pull the bytes
  from `/api/uploads/:id` with the fleet key; agent callers can attach uploads
  but not refs (no session to ACL-check). Verified live end to end, 11/11
  checks.
- **Hybrid retrieval — keyword and meaning, fused.** Every brain now indexes
  each chunk twice: the dense embedding it always had, plus a sparse
  bag-of-terms vector (Qdrant IDF-modified, so exact identifiers like env
  vars, ticket numbers, model names, and error strings survive whole).
  Searches fuse both branches with reciprocal-rank fusion, so
  `TALARIA_EMBED_MODEL` finds the doc that names it AND "how do embeddings
  get configured" finds it too. Legacy dense-only brains keep working
  untouched until rebuilt.
- **Guided reindex — the repair path for a changed embedding model.** Swapping
  `TALARIA_EMBED_MODEL` changes vector dimensions and silently breaks every
  index/search against the old collections. Talaria now probes what the
  embedding service is actually serving (model + dimension, shown in Admin →
  Retrieval) against the LIVE Qdrant collection shape — never the registry,
  which had already gone stale once — and raises a critical alert plus an
  admin banner when they diverge (or when a brain predates hybrid search).
  One "Rebuild index" button recreates each brain in the current model's
  shape and refills it from the workspace's own records; index-don't-copy
  makes the rebuild lossless. Verified live: legacy 384d dense brains
  rebuilt to hybrid, exact-identifier and paraphrase queries both rank the
  seeded doc first, and stale points from deleted sources and
  pre-officialization grounding rules washed out in the process.
- **Confab guard: annotate and strict modes now act.** They were configurable
  but every path discarded the result — observe was effectively the only mode.
  Annotate pins findings to the flagged reply (`messages.guard` /
  `channel_messages.guard`) and renders a warning caveat under it in chat and
  channels (channels update live via republish); the public LLM route appends
  the caveat to non-streaming responses and injects one final SSE delta before
  `[DONE]` on streams. Strict additionally redacts detected secrets (keys,
  tokens, whole private-key blocks) from whatever Talaria persists or hasn't
  yet relayed, so saved copies and future transcripts stay clean. Agent-loop
  keys (`gateway_unmetered_keys`) never receive caveats — a finding must never
  re-enter a model's context; internal utility completions (judge, muse,
  research) likewise stay observe-only so parsed outputs can't be corrupted.
  Admin copy now describes what each mode actually does.

## Phase 7 — self-contained under Talaria (2026-07-09)

Everything routes through Talaria, on one network, with no Dockerfiles and
secrets encrypted at rest.

### Changed
- **Fleet routes through Talaria's gateway.** Every model spec in each rendered
  agent config is rewritten to point at Talaria's gateway (`/api/llm/v1`);
  Talaria routes each model to the provider you register on `/models`. Agents
  have exactly one upstream. Legacy litellm model names are bridged to real
  provider ids (`glm → z-ai/glm-5.2`).
- **One `talaria` docker network** for every Talaria container (dropped the
  legacy `ai_default`). The self-hosted inference server is just a registered
  provider, reached like any other.
- **Bridge eliminated.** The app reaches each agent's persona gateway directly
  on a stable published loopback port (`fleet/fleet.json` = model → url + key);
  `proxyChat`/`listAgents` read the manifest. Removed `bridge/`, `ui/Dockerfile`,
  and the legacy build-based `stack/` — **no Dockerfiles** remain; the run path
  is compose-only (official/published images) + host-run app.

### Fixed
- **Knowledge search finds space overviews.** Top-level spaces are documents
  too — search now sweeps their name + overview body alongside docs, and a
  space hit opens the space itself.
- **Research uses the same agent selector as every other surface** — the
  standard picker at the top of the rail (like Plan), not a bespoke composer
  pill. The composer keeps only the depth pill.
### Fixed
- **Plan mode plans; it no longer files tickets.** Plan-surface turns went
  to the agent with its full toolkit and no hint it was in a planning
  session, so an eager agent would create real tickets mid-conversation.
  Every plan turn (live and server-chained) now carries a plan-mode
  harness: think and decide with the teammate, read anything, create
  NOTHING; tickets come from the Draft tickets control once the plan is
  settled. Verified live with an explicit "create the tickets" bait: zero
  mutating tool calls, and the agent pointed to Draft tickets instead.
- **The plan document now actually builds as you talk.** The side-by-side
  doc only ever updated when someone clicked "Sync from chat", despite the
  empty pane promising otherwise, so new plans looked broken: you talked,
  the doc stayed blank. Every landed agent turn now triggers the sync
  automatically (unsaved manual edits are flushed first so a rewrite starts
  from them), with the rewriting overlay as feedback; the manual button
  stays for on-demand refreshes. Verified live: a fresh plan filled its
  document from the first exchange with no clicks.

### Changed
- **Agents converse like colleagues.** Every rendered soul now carries a
  voice contract: acknowledge in a sentence (or ask ONE clarifying question
  when the ask is ambiguous) before diving into tools, do the full work but
  keep the process out of the chat, and report outcomes like a busy human:
  what happened, where it lives, judgment calls worth flagging. Em dashes:
  most replies need zero. Verified live: the same agent whose replies were
  walls of process narration answered in five sentences with a sensible
  scoping question. Souls hot-reload, so this took effect fleet-wide with no
  restart.
- **Em dashes swept from the platform's own copy.** Around 120 gratuitous em
  dashes across 35 files of visible UI copy (placeholders, tooltips, empty
  states, hints) rewritten with ordinary punctuation. The dash survives only
  where it means something: empty-value glyphs in tables and the brand
  tagline. Codified in docs/UI-CONVENTIONS.md.

### Fixed
- **Queued chat messages get their reply on screen.** Sending while an agent
  was still replying queued correctly server-side, and the follow-up turn was
  generated — but the chat never showed it (it only appeared after a reload).
  The chat now keeps watching whenever the last visible message is the
  user's, so the server-chained reply streams in on its own. Verified live
  with a queued mid-stream message.
- **Agents can edit the docs they authored.** Editing a KB doc the agent
  itself created returned 403 (edits required an explicit editor grant, and
  create_kb_doc granted nothing) — so agents worked around it by creating
  duplicates. Authorship now grants edit; everyone else's docs still need a
  grant.

### Added
- **Agents can open a knowledge space — create_kb_space.** "Put this in a
  new Company space" no longer needs a human errand first: agents can create
  a space (find-or-create by name, so retries never duplicate), while
  sharing, deletion, and marking docs official stay human calls.
- **Agents reach for Talaria first — flailing fixed at three layers.** A real
  transcript showed an agent burning 20 tool calls hunting for Notion/
  Obsidian/vaults when the answer was one toolkit call away. Now: (1) every
  rendered soul carries a **toolkit contract** — the talaria MCP is the first
  reach for anything workspace-shaped, with the tool names spelled out and a
  hard "there is no Notion/Obsidian/Airtable" line; (2) the Hermes image's
  **conflicting bundled skill packs** (note-taking/obsidian, productivity's
  notion + airtable + google-workspace, the ungoverned email pack) are pruned
  on every roll and fresh boot — surgically, everything else stays; (3) the
  gaps that CAUSED improvisation are closed: **create_kb_doc** (the "add to
  knowledge base" job was literally impossible), **list_teammates** (resolve
  a name to an email for drafts/board shares), **list_board_members**,
  **list_research**, and a folder param on create_document. Verified live:
  the same task that flailed now completes in two calls.
- **Agents fail gracefully — report_problem.** When something breaks on the
  agent's side, it no longer dumps endpoints and error internals on
  non-technical teammates: the new tool alerts every admin, files a
  **Helpdesk** ticket with the technical details (board find-or-created),
  and hands the agent plain-language reassurance to relay. The soul contract
  teaches the etiquette; a new critical alert fires when the fleet's MCP
  endpoint itself is unreachable (the root cause behind "connection refused"
  flailing).
- **Attach anything — knowledge, artifacts, or files.** The composer paperclip
  is now a menu: attach a knowledge doc or an artifact (search pickers) or
  upload a file. Knowledge/artifact picks become reference chips on the
  message — the referenced content travels to the model on that turn and on
  every later history rebuild (queued turns, resumes, channel transcripts),
  ACL-checked against the attacher, with truly-private items silently
  undiscoverable. Chips render in history and link back to their source.
### Changed
- **Chats are keyboard-first — the send button is gone.** Enter sends
  everywhere (Esc stops a streaming reply); the enlarged "⏎ send" / "esc
  stop" key chips beside the input are the affordance, fading in when live.
  Composer controls all sit on one optical line, with depth/agent/tier pills
  hugging the input's right edge.
- **UI consistency pass — one app, not fifteen.** A full audit (screenshots +
  code inventory, docs/UI-CONVENTIONS.md is the contract) and the first big
  unification: shared surface primitives (`RailSurface`/`Rail`/`Stage`/
  `StageHeader`/`RailRow`/`CountPill`, `IconButton`, `Chip`/`StatusDot`/
  `DangerLink`). Plan's sidebar moved to the LEFT like every other surface and
  gained a real header; Research, Comms, Knowledge, and Artifacts all sit on
  the same w-72 rail with the same h-12 header line running straight across.
  Fewer, smaller controls: contextual actions are icon buttons with tooltips
  (no more giant "+ New space"/"+ New" primaries), Send/Go became one icon
  affordance (Enter submits everywhere), the reranker panel autosaves on
  change (only the API key keeps an explicit save), destructive actions are
  quiet red links instead of buttons, and ellipsis is gone from all UI copy.
### Added
- **Brain routing everywhere content lives.** The same "Brain" control now
  sits in the top menu of BOTH knowledge docs and artifacts (docs, sheets,
  microsites): Auto / a specific brain / None. For artifacts, explicit
  assignment indexes the rendered content (sheets become tables) into that
  brain only — plan documents and research reports routed away from Auto
  leave the activity brain, and flipping back restores them. Content edits
  re-index in whatever home the routing says. Owner-only on both surfaces;
  privacy still trumps routing.
- **Per-doc brain routing — brains contain only what's assigned to them.**
  Every KB doc gains a "Brain" control (owner-only, next to Official): **Auto**
  (space binding / official→org rules), a **specific custom brain**, or
  **None** (never indexed). Explicit assignment always wins, re-placement is
  immediate, and privacy still trumps routing — a private doc only ever
  reaches its owner's personal brain. Members can see brain names for the
  picker; the binding matrix stays admin-only. **OpenRouter** joined the
  reranker registry (US) — and it reuses the LLM endpoint key you already
  registered, so reranking needs zero extra setup.
- **The RAG stack lives — and got a real retrieval pipeline.** The retrieval
  plane (Qdrant + a native CPU embedding model via TEI, default
  `bge-small-en-v1.5`) is now part of the self-contained compose — it had been
  silently dead since the Phase-7 stack cut, with every index call swallowed.
  Three defenses so that can't recur: a critical **alert** when either service
  is unreachable, a one-click **"Reindex everything" backfill** (Admin →
  Retrieval; content-hash idempotent) that restored the workspace's history,
  and a **15-minute incremental sweep** that self-heals missed rows after an
  outage. Retrieval gained a **reranker** precision stage: a provider registry
  like LLM endpoints — self-hosted TEI, Voyage AI (US), Together AI (US),
  NVIDIA (US), Pinecone (US), Cohere (Canada), Jina (Germany) — with live
  model catalogs where providers expose one, sealed API keys, and graceful
  fallback to vector order (reranking can never break search). And RAG brains
  are finally **curatable in the UI**: create a brain, bind who can search it
  (teams now supported alongside users/agents/everyone), and point KB spaces
  at it — every non-private doc in a bound space feeds that brain instead of
  the org default, re-routing immediately on bind/unbind.
- **Artifacts file themselves — every agent gets a cabinet.** Auto-created
  artifacts stop piling up at the root: each agent gets a folder named after
  it, with category subfolders created on demand — **Plans** (plan documents),
  **Research** (reports), **Documents** (agent-authored docs via MCP),
  **Media** (image saves without an explicit folder), and **Chat summaries**.
  Distilled idle chats now ALSO become browsable artifacts (private to the
  chat's owner) instead of living only in the activity brain. Filing is
  best-effort by construction — a folder hiccup can never kill the flow
  creating the artifact. Humans' hand-created artifacts and explicit folder
  picks are untouched.
- **Research view (#56) — Perplexity-grade cited research, run by YOUR agents.**
  Ask a question on `/research`, pick a depth — **Recon** (one fast pass, a
  cited answer), **Brief** (planned angles, a briefing document), **Expedition**
  (iterative deep dive with gap-chasing rounds, a full report) — and whose
  expertise should drive it. The pipeline runs server-side, outside any chat
  context: the chosen agent's own persona plans the queries, judges the gaps,
  and writes the document, while Perplexity sonar models (through the org
  gateway, metered) run the search stages and supply sources. Every factual
  claim carries an inline [n] citation against a deduped global source
  registry; unresolvable markers are stripped and a mechanical Sources section
  is appended. Reports are org-visible doc artifacts (versioned, shareable,
  exportable), indexed into the activity brain, with a completion notification
  deep-linking back (`/research?r=`). Every agent gets `research` +
  `research_status` MCP tools — an agent researches its own field without its
  conversation window ever swallowing a search dump.
- **Model Roles — tailor the model stack per activity.** `/models` gains a
  "Model roles" panel: assign which model handles each class of work — a
  **search model per research tier** (Recon / Brief / Expedition — Perplexity's
  sonar family maps one-to-one; pointing a tier at a deep-research-class model
  makes the engine run fewer, bigger stages instead of multiplying effort) and
  **Utility** (catalog blurbs, chat distills, Muse fallback) are live;
  **Image understanding**, **Image generation**, **Embeddings**, and
  **Reranker** slots are reserved for their surfaces. Unset = auto (sensible
  pick from what's registered); an assignment only wins while it still routes,
  so a deleted model can never silently break a subsystem. Admin-only, audited.
- **Multiplayer Plan — several humans, one plan.** Plans are no longer
  owner-private: the owner shares a plan by email (avatars + share control in
  the plan header), and collaborators get the whole surface — the conversation
  (they talk to the same agent; turns carry author names so voices stay
  distinct, in the UI and in the agent's transcript), the living document
  (auto editor grant, revoked on leave), ticket drafting, and agent Sync. A
  "Shared with you" sidebar section surfaces plans riding other agents;
  sharing notifies with a `/plan?p=` deep link; presence rings show who's
  viewing right now. Owner shares/removes; collaborators can leave. The doc
  stays owned by the plan's owner, and @mention notifications now reach every
  collaborator (they're doc readers by construction).
- **QA judge scores against the ticket template.** At quality review the judge
  now resolves the same template chain ticket creation uses (assignee binding →
  board default) and receives the skeleton as an objective rubric: every
  section must be meaningfully addressed ("n/a" only where truly inapplicable);
  missing or skeleton sections come back as named "revise" issues.
- **Brain-routability health — unroutable agents surface instead of freezing.**
  Provider pools churn under no-train routing; when an agent's configured
  model drops off its endpoint, chats used to hang silently. Every enabled
  agent's config targets (main / tiers / fallbacks) are now probed against the
  gateway registry (30s cache): an unroutable MAIN raises a critical alert
  ("X's brain is unroutable") and a red chip on the agent's card; dead
  tiers/fallbacks get a warning + amber chip. Fix from /models or the agent.
- **Comms follow-through: unread badges, DM notifications, thread peek.**
  Per-member read cursors (`channel_members.last_read_seq`) drive unread
  count pills on every channel/relay/DM row; having a channel open marks it
  read live. A DM message now drops an inbox notification outright (deduped:
  while one sits unread, further messages fold into it), deep-linking back to
  the conversation via `/comms?c=<id>`. Agent rows in the sidebar gained an
  expand chevron so you can peek at an agent's threads without selecting it.
- **Elevated assistants — promote an admin's assistant to org-wide view/edit.**
  Admin → Users gains an "elevated assistant" toggle on admin rows
  (`agent_defs.elevated`). An elevated personal assistant reaches every live
  board (tickets + governance as editor), every channel and relay, and gets
  implicit editor rights on org-visible knowledge docs and artifacts. Hard
  lines that elevation never crosses: human↔human DMs, other users' private
  items, owner-only actions (board team moves, deletes, sharing changes).
  Elevation is only effective while the owner is an admin — demote the human
  and the assistant's reach collapses with them (demotion also clears the
  flag). Audited (`user.assistant_elevated`).
- **Drag boards between teams.** In the nav rail, a board's owner can drag it
  onto another team group (or Personal) to move it — groups highlight as drop
  targets, empty groups say "drop here". Server-side the move is owner-only
  (it changes who can see the board): `PATCH /api/boards/:id { teamId }`, or
  `{ teamName }` by name ("personal" clears the team).
- **Personal assistants can join group channels — behind a privacy gate.** The
  hard block is gone: your assistant can be added to a shared channel (still
  only by YOU — someone else's assistant never shows up in your picker). Its
  group replies carry a privacy gate above channel instructions: never reveal
  the owner's private context (memory, mail, calendar, private docs) outside a
  DM with the owner, and never use owner-identity tools on a channel's behalf —
  it declines and points people at the owner.
- **Ask your assistant to run your boards.** A personal assistant now acts as
  its owner for board governance (`actingUser` identity proxy): move a board
  between teams, share/unshare it by email, and allow/remove agents. Five new
  MCP tools — `list_teams`, `move_board_to_team`, `add_board_member`,
  `remove_board_member`, `set_board_agents` — assistant-only by construction
  (general agents get 401; the routes resolve identity server-side, and team
  moves still require the owner role). `list_boards` now shows a personal
  assistant its owner's boards (with the owner's role) alongside its
  policy-allowed boards, and `GET /api/teams` answers to the identity proxy.

### Changed
- **Inbox is tailored to you AND the org.** Two zones: the personal column
  (notifications, approvals, your triage/review/blocked queues, agenda, mail,
  quick cards, assistant) beside an org rail titled with the business name —
  fleet health, a live activity **Pulse** across boards/comms/fleet, and for
  admins two glance tiles: live alert count and today's spend (both deep-link).
  Members see the pulse without the admin numbers.
- **Home and Inbox merged.** `/` is now **Inbox** — the top nav item and the
  landing surface: notifications up top (mark-read on open, mark-all-read, the
  panel disappears when quiet), then the day's dashboard (assistant, approvals,
  agenda, mail, triage/review/blocked queues, fleet glance). The unread badge
  moved to the top-level item; `/inbox` redirects; quick cards point at the
  current surfaces (Comms · Plan · Boards · Artifacts).

### Added
- **The Talaria toolkit is ATTACHED — every agent has its tools.** talaria-mcp
  grew a fleet HTTP mode (stateless streamable-HTTP, per-request identity via
  X-Agent-Name, fleet-key auth); the app self-hosts it as a supervised child
  (probe-guarded, respawning) and every rendered config now carries the
  `talaria` MCP entry automatically. Agents get the whole safe surface —
  tickets, artifacts, channels, KB, `save_image_artifact` — on their next
  roll. Closes the long-standing "HTTP transport for containerized agents"
  backlog item (#58).
- **Agents speak product, not plumbing.** The org soul header now instructs
  agents to point teammates at workspace surfaces (Artifacts, boards, docs)
  instead of file paths and containers, unless the person is working at that
  technical level.
- **Save agent images to Artifacts.** Every agent-produced image in chat gets
  a hover "Save to Artifacts" (title + folder picker) that copies it out of
  the agent's container into a durable file artifact. Agents can do it
  themselves too: the talaria MCP grew `save_image_artifact` (path + title +
  folder-by-name, find-or-create) — agent saves default org-visible so the
  team actually sees them. For science. And company meme folders.
- **Agents can show images in chat.** Files an agent creates in its own
  container and references as `MEDIA:<path>` render inline in DMs and
  channels, streamed through `/api/agent-media/:model` — gated on the same
  access as chatting with the agent, restricted to images under the agent's
  own `/opt/data` volume (traversal-proof, size-capped, nosniff), slot-aware.
  The rewrite happens at render time, so past messages light up too; remote
  image URLs in replies already rendered via markdown.
- **Send while the agent is replying.** Agent chats flow like Claude: messages
  sent mid-reply queue into history without interrupting, and when the current
  reply finishes the server automatically runs the next turn covering
  everything queued (chaining until the conversation goes quiet, surviving
  reloads — the follow-up turn is server-driven). The composer stays live
  during streaming (Stop and Send side by side); dead streams can't wedge a
  conversation (10-minute staleness guard). Applies to agent DMs and Plan
  chats; channels were already non-blocking.
- **Org-voice model blurbs.** Model descriptions get ONE rewrite pass into
  task-oriented one-liners ("what it's good at, when to pick it") in the org's
  voice, cached in `model_blurbs`; newly registered models get theirs on the
  next catalog read (throttled, detached). Raw public-catalog text is the
  fallback; nothing is invented for unknown models.
- **Learned parameter support at the gateway.** When an upstream 400 names a
  parameter we sent (newer models retire tunables — sonnet-5 rejects
  `temperature`), the gateway strips it, retries, and remembers per
  endpoint+model so later calls pre-strip. Dynamic specs straight from the
  provider — no tables to maintain.
- **Member model access + human-friendly model picking.** Admins choose which
  models non-admins may pick for AI drafting / preferred model (Models →
  Member access; empty = all, admins never restricted), enforced server-side
  in the catalog, the preference save, and muse resolution (a restricted
  preference falls back). The picker itself grew up: models show a pretty name
  and a one-line "what it's good at" blurb, populated automatically from the
  public catalog (no maintained lists; unknown/self-hosted models simply show
  their id).
- **Rolling agent replacement — edits never kill a conversation.** Each managed
  agent runs in one of two compose slots; applying a change brings the incoming
  slot up on a **fresh port** beside the old container, cuts the manifest over
  only after real health (the app re-reads it per call, so traffic shifts
  instantly), drains in-flight replies (`TALARIA_ROLL_DRAIN_SECONDS`, default
  45), then retires the old container. A newcomer that never gets healthy is
  discarded — the old agent never blinks. Org saves and config/MCP applies roll
  instead of restarting; `proxyChat` additionally holds-and-retries through any
  residual gap instead of failing (or answering with the mock).
- **Organization config — agents join YOUR team.** Admin → Organization sets
  the business name + what it does. Woven in automatically everywhere agent
  identity forms: muse-generated agents/souls/personalities anchor to the
  business, and every rendered SOUL.md opens with an org header (a render-time
  projection — stored souls stay clean, existing agents pick it up on the next
  render/restart). Agents stop introducing themselves as "on the Hermes team."
- **Comms — every conversation in one place.** Chat and Channels merge into a
  single Slack-shaped, agent-native surface (`/comms`): persistent **#channels**
  (ambient talk), **Relays** (named ad-hoc gatherings of people + agents around
  a purpose), **teammate DMs** (human↔human, riding the channel machinery,
  deduped per pair), and **agent DMs** (durable 1:1 threads). One sidebar, four
  sections; old `/chat` and `/channels` routes redirect.
- **Conversations decay instead of accumulating.** Relays **conclude**: a
  summary of what was decided is posted as the final message, indexed for
  retrieval (channel-membership ACL), and the relay archives. Idle agent DMs
  (default 14 days, `TALARIA_CHAT_TTL_DAYS`) are **distilled** — durable
  substance summarized into the activity brain, owner-scoped — then archived
  out of the sidebar. Sweeps run opportunistically (throttled hourly, never
  blocking a request); plans are exempt (they're documents, not scrollback).
- **Ticket & plan templates.** An org-wide template library (markdown skeleton
  + agent guidance per template — the headings are the schema): boards bind the
  ticket templates they use and mark a default (Board settings → General);
  agents can carry overrides (agent modal → Summary → Templates). Resolution
  everywhere: explicit pick → agent binding → board default → freeform. Applied
  when agents draft tickets from plans/channels, when the plan document is
  created/synced, and at ticket creation itself — a bare ticket (quick-add or
  an agent's create tool) is seeded with the resolved skeleton.
- **Dependency-aware ticket drafting.** Planners propose `dependsOn` ordering
  between drafted tickets; the review modal shows/edits them as "blocked by"
  chips, and creation wires real ticket dependencies. The review modal itself
  is board-first (the board's template shapes drafts), roomier (wide layout,
  full description editing), and numbered for dependency reference.
- **Generation-in-progress states.** A shared `Generating` treatment (shimmer
  skeleton lines + stepped dots, plus an in-place overlay variant) replaces
  button-label-only waits: drafting tickets shows skeleton proposal cards,
  the plan document veils while the agent rewrites it, cron drafting shows a
  designing row, and ticket creation counts down.
- **Plan view, phase 2 — the document lives.** "Sync from chat" has the plan's
  own agent rewrite the living plan document from the conversation so far
  (`POST /api/plan/:id/doc`, metered like any chat turn; agent preamble and
  code fences are stripped). Draft tickets now treats the plan document as the
  curated source of truth, with the transcript as supporting context.
- **@mentions on the plan surface (#60).** The plan composer autocompletes
  teammates (shared mention machinery extracted from channels into
  `components/chat/mentions.tsx`); mentioned users are notified once they can
  read the plan's document (owner-private plans mention silently until shared).
- **Plans feed the activity brain (#63).** Plan turns and the living plan
  document are indexed into the ambient activity collection, ACL-scoped to the
  plan's owner (`planOwnerId`) — private planning never surfaces for anyone
  else. Hand edits to the doc re-index via the artifact save path.

### Fixed
- **Fresh-install model selection.** Bare model ids that contain `/`
  (OpenRouter-style, e.g. `qwen/qwen3-14b`) were mistaken for
  `endpoint/model` pins, leaving the preferred-model picker empty and the muse
  with "no models configured". The gateway catalog now tags qualified ids
  explicitly (`GatewayModel.qualified`).
- **No-train routing pool is fetched live.** The OpenRouter US no-train
  provider pool comes from `GET /providers` (US datacenters/HQ) on every call
  (briefly cached) instead of a hardcoded six-provider list that had gone stale
  and 404'd models it no longer served ("No allowed providers are available").
  A stored `only` list is only the offline fallback.
- **Provider catalogs are always live.** Preset seed model lists are gone —
  adding a provider drops straight into the endpoint's manage modal, where
  models come from the provider's live `/models` catalog: full list browseable
  on focus (the old picker capped at 8 alphabetical matches, hiding newer
  models), provider ordering preserved (OpenRouter lists newest first), and
  pagination followed (Anthropic pages at 20 by default).
- **Fleet network self-creates.** `fleetUp` ensures the external `talaria`
  docker network exists before `compose up` — a fresh install no longer fails
  with "network talaria declared as external, but could not be found"
  (`setup.sh` also created the wrong name, `talaria-fleet`).

### Security
- **Provider API keys encrypted in the DB**, not in configs. Sealed with
  AES-256-GCM in `llm_endpoints.api_key_cipher`; entered on `/models`, never
  returned to a client. Existing config keys are migrated into the DB
  automatically.
- **Envelope encryption + one-click rotation.** A random 256-bit DEK encrypts
  every secret and is stored wrapped by the root secret, so the
  unlock-everything key is never in a config. Admin → Encryption rotates the key
  and re-encrypts every secret (provider keys, agent secrets, OAuth tokens) in a
  single pass. All symmetric AES-256 — post-quantum-safe (no asymmetric crypto).

## [Unreleased]: Phase 6 — product depth (2026-07-06)

Turning the elegant shell into a capable product: one place to manage each
agent, attachments, personal assistants, governance, and a real audit trail.

### Added
- **Unified agent management modal**, one modal per agent with tabs — Summary ·
  Config · Skills · Memory · MCP · Versions. Every internal (previously separate
  top-level pages) lives here: config editing, skills (WYSIWYG + history),
  memory, MCP with live connection testing, and version history with one-click
  revert. Read-only for non-admins.
- **Agents roster redesign**, a toggleable **grid / list** where each agent
  shows only name, **role**, a health dot (up/degraded/down/retired/legacy from
  real container state), and icon controls (start/stop · manage · duplicate ·
  retire/migrate/re-hire). Detail moved into the modal.
- **Editable agent roles** (`agent_defs.role`) — a human title (e.g. "Support
  Lead") shown on the roster, set at creation, editable in the modal.
- **Re-hire + duplicate**, retired agents can be un-retired (re-enable → render →
  start from the preserved volume); any agent can be duplicated into a new one.
  Retire is a typed-slug double opt-in.
- **Fleet Reconcile**, one button renders all managed configs and starts every
  enabled agent that isn't running (drift + cold start; reboot survival is
  already handled by `restart: unless-stopped`).
- **Personal assistants**, everyone can spin up their own Hermes agent (own
  container, key, memory) from Home — `agent_defs.owner_user_id`,
  `createPersonalAgent()`, a "Your assistant" card.
- **Attachments in chat + channels**, images and documents (disk-backed
  `uploads` table, served from `/api/uploads/:id`); images render inline and are
  passed to vision models as data-URL content parts.
- **Per-view access control**, admins grant/revoke each primary view per member
  (`users.denied_views`); denied views are hidden from the nav and route-gated.
- **Audit trail + retention**, a real `audit_log` (actor · action · target ·
  before/after) wired into governance mutations, surfaced to admins on the Audit
  page; `audit_retention_days` is the first admin-editable app setting.
- **Chat tier picker**, the composer's raw model-tier `<select>` is now a
  premium portaled pill.

### Changed
- **Models page** is compact: provider cards show identity + a model count +
  Manage; the model list (with a proper catalog-search **add-model** flow, not a
  LabelPicker), pricing, class, and privacy routing moved into a modal.
- **Modals center on the viewport** (portaled to `<body>`) instead of within a
  backdrop-filtered card.

## [Unreleased]: Phase 5 — product IA + elegance (2026-07-06)

Reworking Talaria from a feature grid into a coherent product: two mental
modes, a real landing, versioned internals, and a self-hostable vocabulary.

### Added
- **Home / Today** at `/`, the seamless landing. In Talaria's guardrail model a
  person's job is to triage, review, and unblock the agents' work, so Home
  surfaces exactly those queues (scoped to your boards) plus unread mentions,
  quick entries into the work surfaces, and an accurate fleet-health glance
  (real container status). Chat moves to `/chat`.
- **Talaria LLM gateway** ([`server/llm-gateway.ts`]), one OpenAI-compatible
  endpoint over the whole model registry (`/api/llm/v1/{models,chat/completions}`,
  streaming). Provider keys stay server-side; per-endpoint `request_defaults`
  merge into every call; usage is metered per key. **Per-user API keys**
  (`tlk_…`, minted in Settings → API keys, admin-grantable via `can_mint_keys`).
- **No-train routing as a setting**, a per-cloud-endpoint toggle on /models
  (OpenRouter no-store allowlist + `data_collection: deny`, or portable deny) —
  privacy is opt-in, not baked in.
- **Versioned skills + memory**, every save is an immutable, authored revision
  (`internal_versions`); recover or load any prior one. Edited in a **WYSIWYG
  modal** (RichEditor + a history rail) — the raw textareas are gone.
- **MCP connection testing**, live status chips (Connected / Login required /
  Unreachable / Error) via a real MCP `initialize` probe carrying the agent's
  identity, plus a premium add-server modal that tests before saving.

### Changed
- **Navigation regrouped** into **Work** (Home · Chat · Channels · Boards ·
  Inbox) and **Manage** (Agents · Models · Compute · Cost · Audit · Alerts) +
  System — simple for the non-technical surfaces, control grouped for the
  technical ones. Skills/Memory/MCP/Models moved off the top nav into the Agents
  page. Fleet overview folded into Agents (`/fleet` → `/agents`).
- **"Local" → "Self-hosted"** in all user-facing copy (people run models on
  local machines *and* other on-prem boxes). "Local inference" → **Compute**,
  "Activity" → **Audit**.

### Removed
- The dead **`/tasks`** nav item (it matched the boards API route; no page ever
  existed behind it).

## [Unreleased]: Phase 2 UI (2026-07-02)

Talaria's own front end ([`ui/`](./ui), Vite + TanStack Start) grows a full,
self-hosted **project-management suite**, owned in Talaria's own Postgres/Redis,
not proxied from mission-control.

### Added
- **Boards & teams**, shareable kanban boards (personal or team-owned), a
  consolidated **Board settings** modal (General / People / Agents), board-scoped
  agent policy (restrictive by default), teams + member management, and soft
  **archive** for boards and tickets.
- **Tickets**, rich detail modal: TipTap WYSIWYG description (markdown under the
  hood) with read/edit toggle + slide-in full-screen editor, syntax-highlighted
  code + hover links, comments (Ctrl+Enter to send), an **Activity** tab, watchers,
  and a quality-review approval gate.
- **Ticket routes**, each ticket is a directly-linkable nested route
  (`/boards/:boardId/:taskId`) with copy-link buttons on cards, list rows, and the
  modal.
- **Fields**, agent-appropriate **effort** (XS-XL), **multiple assignees**
  (board-scoped agents only), ticket **dependencies** (blocked-by / blocks), labels,
  due date, and **auto-accumulated time-spent** (`addTimeSpentSeconds`). Manual hour
  estimates removed.
- **Blocked status**, a new kanban column for stalled / needs-input work.
- **List view**, configurable, drag-reorderable, click-to-sort columns, persisted
  per board.
- **Multiplayer**, live boards via Redis pub/sub → SSE (`/api/boards/:id/events`).
- **Reusable UI primitives**, `CloseButton`, `CopyLinkButton`, `InlineCreate`,
  `danger` button variant; `RichEditor` gains `bare` / `fill` / `onSubmit` modes.
- **Agent guardrails**, on `PUT /api/tasks/:id`, agents may triage but cannot
  self-assign (`assigned` → 403) or self-complete (`done` → `quality_review`), and
  cannot change assignees.
- **Agent MCP (`talaria-mcp`)** ([`mcp/`](./mcp)), an MCP server exposing only the
  safe tools (`list_boards`, `list_tickets`, `get_ticket`, `create_ticket`,
  `triage_ticket`, `comment`, `report_outcome`, `add_time`, `add_dependency`) — no
  assign, no complete; guardrails hold by construction. Identity via
  `TALARIA_AGENT_KEY` + `TALARIA_AGENT_NAME`.
- **Agent-authed task API**, the fleet key plus a new `x-agent-name` header opens
  an agent path on boards list, board tasks (list + create → `inbox`, never
  assigned), ticket detail, comments, and add-dependency. Every agent call is
  checked against the board's agent policy and attributed to the named agent in
  activity/comments. Named agents on `PUT /api/tasks/:id` are policy-checked too;
  unnamed key callers (legacy plugin heartbeat/report) keep their old access.
- **Group chat (channels)**, Slack-style channels where teammates and fleet
  agents are members. Channels + members + agents + messages live in Talaria's
  Postgres; live over Redis pub/sub → SSE (`/api/channels/:id/events`). Agents
  reply when **@mentioned** (by name or model id): the reply streams into the
  channel for every member, built from the channel transcript via the gateway
  plane. Composer has @mention autocomplete; channel settings manage people +
  agents (adding an agent requires access to it). New "Channels" nav surface.

- **People pickers**, one searchable `UserPicker` (over `GET /api/users`, everyone
  who has signed in) replaces every type-an-email field: board sharing, board
  creation invites, teams, and channel members.
- **Label picker**, ticket labels are tag chips + the shared combobox: the board's
  existing labels surface for reuse, and typing creates a new one (Enter or comma).
  Replaces the raw comma-separated text input.
- **Display names**, users can set how they appear (Settings → profile; updates the
  live session, no re-login). Member lists, channel messages, and avatars prefer
  the name and show the email as secondary.
- **Consistent control sizing**, one `sm`/`md` scale (`h-9`/`h-11`) shared by
  Button, Input, Select, and Combobox via a `size` prop — mixed-height form rows
  and hand-set `h-8`/`h-9` overrides are gone.
- **Notifications + user @mentions**, the channel composer autocompletes human
  members alongside agents; @mentioning a person drops a notification in their
  **Inbox** (new nav surface with an unread badge). `GET/PUT /api/notifications`;
  mention tokens are the email localpart, dashed name, or first name.
- **Token ledger**, every agent generation (1:1 chat turn + channel reply) lands
  in `usage_events` — real gateway-reported counts (`stream_options.include_usage`),
  or char-based estimates flagged `~` when the gateway doesn't report. The `/cost`
  page is live: today/7d/30d token tiles, a 14-day daily strip, and a per-agent
  breakdown. Dollar cost lands with per-LLM pricing attribution (see ROADMAP).
- **Admin console**, `/admin` is live: everyone who has signed in, with role
  management (member/admin; `AUTH_ADMIN_EMAILS` admins are pinned, no
  self-demotion) and the per-person agent allow-list UI (empty = all agents) that
  the access model always supported. Admin-gated `GET/PUT /api/admin/users`.
- **Agent harness (phase A)**, Talaria starts becoming the fleet's source of
  truth: `llm_endpoints` (model backends classed local/cloud — feeds the coming
  cost split), `agent_defs` + immutable `agent_versions` (soul, main model,
  `model_aliases` tiers, fallbacks, plugins, MCP servers — diffable, revertible),
  and an idempotent importer that ingests the existing `ai/orchestration` stack
  (`agents.yaml`, per-agent `config.yaml` + `SOUL.md`). `/agents` grows an
  admin-only **Definitions** panel showing each agent's tiers (local/cloud chips),
  fallback chain, soul, and version. Rendering + spin up/down land in phase B.
- **Agent harness (phase B)**, Talaria renders and runs the fleet: versions
  materialize into a gitignored `fleet/` dir (per-agent `config.yaml` emitted as
  YAML 1.1 so PyYAML sees exactly the original semantics, `SOUL.md`, a generated
  compose that reuses the legacy `ai_hermes-<dept>` volumes so memories survive,
  and the gateway manifest — which the bridge now **hot-reloads**, no restart).
  `/agents` gains live container status and lifecycle buttons: start/stop for
  managed agents, one-click **Migrate** for legacy ones (stop old → render →
  start managed → health-gate). Pilot migrated: `sam-support` runs Talaria-managed
  with memories intact, answering through the gateway.
- **Agent harness (phase C1: config control)**, edit an agent **in-app** — soul,
  main model, alias tiers, fallback chain, all against the endpoint registry —
  and save as a new immutable version; **Save & apply** re-renders and restarts
  the managed container. Reverts re-publish an old payload as a new version
  (history is append-only). Structured edits are merged into the raw Hermes
  config so rendering stays faithful.
- **Local vs cloud in the ledger**, every generation now records the serving
  model's endpoint class (from the agent's current main endpoint) + model id.
  `/cost` shows the 30-day local/cloud share bar, a stacked per-day strip, and a
  per-agent "% local" column — the view for optimizing the small-model/frontier
  mixture.
- **Models tab**, a System-area registry for model backends: one-click presets
  for **every common US provider** (Anthropic, OpenAI, Google, xAI, Meta,
  OpenRouter, Groq, Together, Fireworks, Cerebras, Perplexity, DeepInfra,
  DeepSeek, local Ollama/vLLM) with base URLs and wiring preconfigured — pick,
  name the key env var, done. Local/cloud is **inferred** (never asked for known
  providers; LAN/loopback URL heuristic for custom). Provider marks throughout;
  the provider chooser and the model tier picker are the same searchable
  combobox. Each provider card offers its **live catalog** (server-side
  `/models` fetch, keys never leave the box) so you search what the provider
  actually serves; per-provider **model catalogs** (tag-style add/remove) and
  cloud pricing fields.
  The agent editor's clunky selects are replaced by **one searchable picker**
  over every catalog. Deleting a model or provider that agents still use warns
  with the blast radius and **double opt-ins**, then cascades: each affected
  agent gets a new version with the tier stripped (revertible), re-rendered,
  running managed agents restarted. A model that is some agent's **main** is
  never cascaded — reassign first.
- **Agent harness (phase C2: create/retire)**, spin agents up and down on a
  whim. **New agent** on `/agents`: pick a template (any existing agent — model
  tiers/tools/plugins carry over with every identity reference re-stamped to the
  new slug), Talaria allocates a fresh gateway key into the stack `.env`, writes
  v1 with a starter soul, renders a fresh-chassis service (own state volume),
  starts it, and the bridge picks it up live. **Retire** removes the container
  and drops the agent from the fleet manifest; state volume + version history
  stay.

- **Pricing**, real dollars in the ledger: per-model $/MTok prices live on each
  provider (Models page grid; endpoint-level rates as fallback; Anthropic preset
  ships with official prices), every generation records its serving endpoint,
  and cost computes at read time — editing a price reprices history instantly.
  `/cost` gains a **Cloud spend** tile (30d + today), per-model $ in the split
  legend, a per-agent $ column, and a loud warning for **unpriced** cloud tokens
  (never silently $0). Local tokens are $0 by definition.
- **Model-tier routing**, chat any agent on any of its configured tiers: the
  fleet manifest now carries one gateway entry per alias (`<base>-<alias>`,
  resolved by the agent's own Hermes gateway), `/api/agents` returns real
  agents (not raw gateway models) with their tiers, the chat composer gains a
  tier select, and the API validates tiers against the agent's definition. The
  ledger attributes tier-routed turns by the **alias's** endpoint — a `glm`
  turn lands as cloud/glm while main-model turns stay local.
- **Auto-fetched pricing**, zero-config rates: a server-side price oracle pulls
  OpenRouter's public model catalog (no key) and prices every matched cloud
  model automatically (`llm_endpoints.auto_prices`, refreshed in the background
  and on provider/model changes). Cost coalesces user override → auto → endpoint
  default; the pricing grid shows an "auto" tag with fetched rates as
  placeholders. No exact match → honestly unpriced, never guessed.
- **Channel tier mentions**, `@Dex:deepseek` routes that reply to the tier; the
  composer autocompletes `Label:tier`; unknown tiers fall back to main; the
  ledger attributes the turn to the alias endpoint.
- **Activity** (`/activity`), one merged, user-scoped feed — ticket events,
  channel messages, agent config versions — with kind filters. A read model
  over existing tables; nothing new stored.
- **Alerts** (`/alerts`), live-derived health: down/unhealthy managed
  containers, unreachable gateway plane, unpriced cloud usage,
  estimate-dominated ledger, failed and week-stale blocked tickets. Severity
  ranked, deep-linked, nothing to configure.
- **Skills** (`/skills`), the fleet's skills as they exist on disk (shared
  stack dir + each agent's dept/fleet mount): parsed descriptions, live
  SKILL.md editing, admin create/delete. Hermes reads skills per invocation,
  so edits apply on the next run — no restart.
- **Memory** (`/memory`), each managed agent's `memories/MEMORY.md` read and
  written through its running container — no second copy to drift.
- **MCP** (`/mcp`), per-agent MCP servers from the versioned config: add and
  remove as NEW immutable config versions (optionally applied live), untouched
  entries preserved byte-for-byte; plus a talaria-mcp explainer.
- **Inference** (`/inference`), your own hardware: local backends probed live
  (status, latency, serving-now models) plus local token throughput.
- **Per-ticket token spend**, agents report tokens burned on a ticket
  (`POST /api/tasks/:id/usage`, MCP tool `log_usage`, board policy enforced,
  tier-aware); reports are first-class priced ledger rows; the ticket rail
  shows tokens · $ · per-model.
- **Plan chat**, a channel's **Plan** button turns the conversation into
  tickets: a chosen agent (any tier) drafts structured proposals from the
  transcript, a human edits/prunes them in a review modal and creates the
  keepers — into inbox, never assigned.

### Fixed
- SSE event streams no longer crash the server when a client disconnects before
  the Redis subscriber finishes connecting (unhandled rejection in the
  board/channel event stream).
- Ticket labels no longer require hand-typed comma lists (see label picker).
- Logging in no longer clobbers a user-set display name (the provider identity
  only fills the unfriendly defaults).

### Changed
- Sessions are Redis-backed (opaque sid → `sess:<sid>`), not HMAC cookies.

## [Unreleased]: 0.1.0

Initial working slice: hermes-workspace ↔ mission-control bridge + per-agent adapter plugin.
All milestones below verified live against a running stack on 2026-07-01
([`scripts/verify-stack.sh`](./scripts/verify-stack.sh), all checks pass).

### Added
- **Gateway plane** (`bridge/src/gatewayPlane.ts`), the fleet multiplexer. Fronts the Hermes gateway
  `:8642` for a whole fleet: `/v1/models` = every agent, `/v1/chat/completions` routed by model to that
  agent's real gateway (per-agent key, SSE streamed). One workspace talks to every agent via the model
  switcher. Fleet declared in a manifest (`TALARIA_FLEET`/`TALARIA_FLEET_FILE`). Stood up the Phase-1
  fleet engine as a two-plane runtime (gateway multiplexer + dashboard management bridge).
- **Bridge** (`bridge/`, Node/TS), transparent reverse-proxy of the Hermes dashboard `:9119`.
  - **M1** pass-through: all 164 dashboard routes (incl. OAuth + 4 websockets) proxied byte-for-byte
    to the real dashboard; conductor capability-probe (`GET /api/conductor/missions`) served as
    `200 application/json` so the workspace uses remote dispatch instead of native-swarm.
  - **M2** mission create: `POST /api/conductor/missions {name,prompt}` → mission-control
    `POST /api/tasks`, response shaped for the Conductor.
  - **M3** status round-trip: `GET/DELETE /api/conductor/missions/{id}` poll + cancel, mapping
    mission-control task status → the workspace mission enum. Never forces the Aegis-gated `done`.
- **Fleet board**, the bridge serves the workspace's `/api/plugins/kanban/*` surface from
  mission-control, so the swarm/kanban board becomes a live view of the MC fleet (columns = MC
  statuses, cards = MC tasks, full CRUD; `done` stays Aegis-gated). Toggle `TALARIA_KANBAN_FROM_MC=0`.
  The conductor poll's `lines` are also enriched with the task's header + comment feed.
- **Plugin** (`plugin/talaria/`, Hermes standalone), per-agent mission-control adapter.
  - **M3** register (`POST /api/agents/register`) + opt-in background heartbeat
    (`TALARIA_HEARTBEAT_SECONDS`) that polls `/api/agents/{id}/heartbeat` + reports via
    `PUT /api/tasks/{id}` (toward `quality_review`, never `done`). Safe no-op until configured.
- **mission-control adapter** (`adapter/`), **M4** `HermesAdapter` making Hermes a first-class
  framework in mission-control; PR-ready patch + verification.
- **Stack** (`stack/`), compose wiring workspace + mission-control + bridge on the shared `edge`
  network; `scripts/verify-stack.sh` reproduces the M1-M3 verification end-to-end.
- **Docs**, [`docs/m0-contract.md`](./docs/m0-contract.md) (the M0 contract diff + `:9119` allowlist),
  README with architecture + compatibility matrix.
- **Fleet: both Hermes deployment shapes** (`bridge/src/config.ts`, `gatewayPlane.ts`, `sessions.ts`) -
  a fleet entry now supports (A) separate installs (one gateway per agent) and (B) multiple Hermes
  profiles on one host (each profile's API server on its own port). Optional `profile` / `upstreamModel`
  / `pathPrefix` fields: Talaria rewrites the forwarded `model` to the profile when set and honours a
  profile path prefix on chat + session calls. The UI only ever sees Talaria's exposed model ids.
  `pathPrefix` seeds support for Hermes' emerging single-endpoint profile multiplex (`multiplex_profiles`).
- **Phase 2 UI** (`ui/`), the first slice of Talaria's own front end (Vite + TanStack Start, React 19,
  TypeScript), matching the hermes-workspace stack so its chat components lift cleanly.
  - **Mercury design system** (`ui/src/styles.css`, `ui/src/lib/theme.ts`), hand-rolled Tailwind v4
    tokens, dark (`mercury`) + light (`mercury-light`), violet→magenta neon on Mercury-planet neutrals.
    Reuses hermes-workspace's `--theme-*` token contract to keep component lifts frictionless.
  - **Pluggable auth**, each provider independently enable-able (flag **+** secrets required):
    registry (`ui/src/server/auth/config.ts`), stateless HMAC-signed sessions, **Google OAuth**
    (start + callback) and username/password, with routes `/api/auth/{providers,session,google,
    google/callback,password,logout}`. Login screen renders only the enabled providers. Verified live:
    session round-trip, logout, tampered-cookie rejection, provider gating.
  - Both upstreams vendored under `vendor/` (gitignored) as lift sources.

### Key findings (verified against source)
- The Hermes dashboard has **no** `/api/conductor/*` routes, Talaria *serves* them (adds capability),
  it does not override native behavior. Unset `HERMES_DASHBOARD_URL` → 100% native.
- mission-control gates `done` behind **Aegis** approval; Talaria respects it (human-only Done).
- mission-control has no published image (builds from source, pinned `d09e608`).

### Not yet
- Decomposed/broadcast mission parity (those go workspace-local `:3000`), needs the
  `HERMES_MISSION_API_URL` upstream PR to hermes-workspace.
- Executing pulled work inside the Hermes run loop (heartbeat pulls; in-agent dispatch is next).
- Enabling the plugin on the live PackLedger fleet (staged, not yet `--force-recreate`d).
