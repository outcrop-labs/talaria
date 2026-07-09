// Postgres — durable state (users, roles, per-agent access, conversations,
// messages). postgres.js (no native build). Migrations run once, lazily, on
// first query. Cached on globalThis so HMR doesn't open a new pool each reload.

import postgres from 'postgres'
import { initSecretbox } from '../secretbox'

type Sql = ReturnType<typeof postgres>
const g = globalThis as unknown as { __talariaSql?: Sql; __talariaMigrated?: Promise<void> }

export function getSql(): Sql {
  if (!g.__talariaSql) {
    const url = process.env.DATABASE_URL
    if (!url) throw new Error('DATABASE_URL is not set')
    g.__talariaSql = postgres(url, { max: 10, idle_timeout: 20, onnotice: () => {} })
  }
  return g.__talariaSql
}

// One statement per entry (postgres.js extended protocol is one-statement).
const MIGRATIONS: string[] = [
  `create table if not exists users (
     id uuid primary key default gen_random_uuid(),
     sub text unique not null,
     email text,
     name text,
     picture text,
     role text not null default 'member',
     created_at timestamptz not null default now(),
     last_seen_at timestamptz not null default now()
   )`,
  `create table if not exists user_agent_access (
     user_id uuid not null references users(id) on delete cascade,
     agent_model text not null,
     primary key (user_id, agent_model)
   )`,
  `create table if not exists conversations (
     id uuid primary key default gen_random_uuid(),
     user_id uuid not null references users(id) on delete cascade,
     agent_model text not null,
     title text,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now(),
     archived boolean not null default false
   )`,
  `create index if not exists conversations_user_agent_idx
     on conversations(user_id, agent_model, updated_at desc)`,
  `create table if not exists messages (
     id uuid primary key default gen_random_uuid(),
     conversation_id uuid not null references conversations(id) on delete cascade,
     seq integer not null,
     role text not null,
     content text not null default '',
     reasoning text not null default '',
     tools jsonb not null default '[]',
     status text not null default 'complete',
     created_at timestamptz not null default now(),
     unique (conversation_id, seq)
   )`,
  `create index if not exists messages_conv_idx on messages(conversation_id, seq)`,
  // Fleet agent registry — Talaria's own "brain" (ripped from mission-control's
  // agents table). Agents register + heartbeat to Talaria, not MC.
  `create table if not exists fleet_agents (
     id uuid primary key default gen_random_uuid(),
     name text unique not null,
     role text not null default 'agent',
     status text not null default 'offline',
     last_seen timestamptz,
     last_activity text,
     framework text,
     capabilities jsonb not null default '[]',
     config jsonb not null default '{}',
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  // Teams — a group of users that can collectively own/access boards.
  `create table if not exists teams (
     id uuid primary key default gen_random_uuid(),
     name text not null,
     created_by uuid references users(id) on delete set null,
     created_at timestamptz not null default now()
   )`,
  `create table if not exists team_members (
     team_id uuid not null references teams(id) on delete cascade,
     user_id uuid not null references users(id) on delete cascade,
     role text not null default 'member',
     created_at timestamptz not null default now(),
     primary key (team_id, user_id)
   )`,
  // Boards — user-owned kanban boards, shareable across the team.
  `create table if not exists boards (
     id uuid primary key default gen_random_uuid(),
     name text not null,
     owner_id uuid not null references users(id) on delete cascade,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  // A board may belong to a team — all team members can access it.
  `alter table boards add column if not exists team_id uuid references teams(id) on delete set null`,
  // Membership = sharing. role: owner | editor | viewer.
  `create table if not exists board_members (
     board_id uuid not null references boards(id) on delete cascade,
     user_id uuid not null references users(id) on delete cascade,
     role text not null default 'editor',
     created_at timestamptz not null default now(),
     primary key (board_id, user_id)
   )`,
  // Task queue — Talaria's own (ripped from mission-control's tasks), scoped to a board.
  `create table if not exists tasks (
     id uuid primary key default gen_random_uuid(),
     board_id uuid not null references boards(id) on delete cascade,
     title text not null,
     description text,
     status text not null default 'inbox',
     priority text not null default 'medium',
     assigned_to text,
     created_by text not null default 'user',
     result text,
     tags jsonb not null default '[]',
     metadata jsonb not null default '{}',
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  `create index if not exists tasks_board_idx on tasks(board_id, status, updated_at desc)`,
  `create index if not exists tasks_assignee_idx on tasks(assigned_to)`,
  `alter table tasks add column if not exists due_date timestamptz`,
  // Threaded comments on a task (author is a user email or an agent name).
  `create table if not exists task_comments (
     id uuid primary key default gen_random_uuid(),
     task_id uuid not null references tasks(id) on delete cascade,
     author text not null,
     content text not null,
     parent_id uuid references task_comments(id) on delete set null,
     created_at timestamptz not null default now()
   )`,
  `create index if not exists task_comments_task_idx on task_comments(task_id, created_at)`,
  // Activity/audit log for a task (created, status change, assigned, comment, …).
  `create table if not exists task_activity (
     id uuid primary key default gen_random_uuid(),
     task_id uuid not null references tasks(id) on delete cascade,
     actor text not null,
     type text not null,
     description text not null,
     created_at timestamptz not null default now()
   )`,
  `create index if not exists task_activity_task_idx on task_activity(task_id, created_at desc)`,
  // Ticket refs (BOARD-12): a per-board prefix + monotonic counter.
  `alter table boards add column if not exists ticket_prefix text`,
  `alter table boards add column if not exists ticket_seq integer not null default 0`,
  // Richer task fields (ripped from mission-control): ticket no, effort, the
  // agent's structured result (outcome/resolution/error), completion time.
  `alter table tasks add column if not exists ticket_no integer`,
  `alter table tasks add column if not exists estimated_hours numeric`,
  `alter table tasks add column if not exists actual_hours numeric`,
  `alter table tasks add column if not exists outcome text`,
  `alter table tasks add column if not exists resolution text`,
  `alter table tasks add column if not exists error_message text`,
  `alter table tasks add column if not exists completed_at timestamptz`,
  // Watchers — users/agents following a task for updates.
  `create table if not exists task_watchers (
     task_id uuid not null references tasks(id) on delete cascade,
     watcher text not null,
     created_at timestamptz not null default now(),
     primary key (task_id, watcher)
   )`,
  // Quality review / approval gate (agent → quality_review → human approves → done).
  `create table if not exists quality_reviews (
     id uuid primary key default gen_random_uuid(),
     task_id uuid not null references tasks(id) on delete cascade,
     reviewer text not null,
     status text not null,
     notes text,
     created_at timestamptz not null default now()
   )`,
  // Board-scoped agents — which fleet agents may be assigned on a board. Access
  // is either "allow all" (explicit flag) OR the specific board_agents list.
  `create table if not exists board_agents (
     board_id uuid not null references boards(id) on delete cascade,
     agent_model text not null,
     primary key (board_id, agent_model)
   )`,
  `alter table boards add column if not exists allow_all_agents boolean not null default false`,
  // Soft archive for boards and tickets — hidden from default views, restorable.
  `alter table boards add column if not exists archived_at timestamptz`,
  `alter table tasks add column if not exists archived_at timestamptz`,
  // Agent-appropriate effort (t-shirt size), multiple assignees, and dependencies.
  // Estimates in hours are silly for agents — dropped from the UI/API.
  `alter table tasks add column if not exists effort text`,
  `alter table tasks add column if not exists assignees jsonb not null default '[]'`,
  // Actual time is accumulated from agent iterations (not a manual estimate).
  // Future: attribute token spend + which LLM APIs were used per ticket.
  `alter table tasks add column if not exists time_spent_seconds bigint not null default 0`,
  // Backfill assignees from the old single assigned_to column (once).
  `update tasks set assignees = to_jsonb(array[assigned_to]) where assigned_to is not null and assignees = '[]'::jsonb`,
  // Ticket → ticket dependencies (task is blocked by depends_on_id).
  `create table if not exists task_dependencies (
     task_id uuid not null references tasks(id) on delete cascade,
     depends_on_id uuid not null references tasks(id) on delete cascade,
     created_at timestamptz not null default now(),
     primary key (task_id, depends_on_id)
   )`,
  // Group chat — Slack-style channels where humans and fleet agents are members.
  // msg_seq is a per-channel counter (like boards.ticket_seq): channels have
  // many concurrent writers, so max(seq)+1 would race.
  `create table if not exists channels (
     id uuid primary key default gen_random_uuid(),
     name text not null,
     topic text,
     created_by uuid references users(id) on delete set null,
     msg_seq integer not null default 0,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now(),
     archived_at timestamptz
   )`,
  `create table if not exists channel_members (
     channel_id uuid not null references channels(id) on delete cascade,
     user_id uuid not null references users(id) on delete cascade,
     role text not null default 'member',
     created_at timestamptz not null default now(),
     primary key (channel_id, user_id)
   )`,
  `create table if not exists channel_agents (
     channel_id uuid not null references channels(id) on delete cascade,
     agent_model text not null,
     primary key (channel_id, agent_model)
   )`,
  // author_type 'user' | 'agent'; author is the user's email/name or the agent model.
  `create table if not exists channel_messages (
     id uuid primary key default gen_random_uuid(),
     channel_id uuid not null references channels(id) on delete cascade,
     seq integer not null,
     author_type text not null,
     author text not null,
     content text not null default '',
     status text not null default 'complete',
     created_at timestamptz not null default now(),
     unique (channel_id, seq)
   )`,
  `create index if not exists channel_messages_idx on channel_messages(channel_id, seq)`,
  // Notifications — user-facing inbox (mentions today; more kinds later).
  `create table if not exists notifications (
     id uuid primary key default gen_random_uuid(),
     user_id uuid not null references users(id) on delete cascade,
     kind text not null,
     title text not null,
     body text not null default '',
     href text not null default '',
     read_at timestamptz,
     created_at timestamptz not null default now()
   )`,
  `create index if not exists notifications_user_idx on notifications(user_id, created_at desc)`,
  // Token ledger — one row per completed agent generation (1:1 chat or channel
  // reply). Real counts when the gateway reports usage; char-based estimates
  // (flagged) otherwise. Future: cost attribution per ticket / per LLM API.
  `create table if not exists usage_events (
     id uuid primary key default gen_random_uuid(),
     agent_model text not null,
     source text not null,
     ref_id uuid,
     prompt_tokens integer not null default 0,
     completion_tokens integer not null default 0,
     estimated boolean not null default false,
     created_at timestamptz not null default now()
   )`,
  `create index if not exists usage_events_agent_idx on usage_events(agent_model, created_at desc)`,
  `create index if not exists usage_events_created_idx on usage_events(created_at desc)`,
  // ── Agent harness: Talaria as the single source of truth for the fleet ──────
  // LLM endpoint registry — the model backends agents draw from. class drives
  // the local-vs-cloud ledger split; api_key_env names the env var (never the key).
  `create table if not exists llm_endpoints (
     id uuid primary key default gen_random_uuid(),
     name text unique not null,
     provider text not null,
     base_url text,
     class text not null default 'cloud',
     api_key_env text,
     context_length integer,
     price_in_per_mtok numeric,
     price_out_per_mtok numeric,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  // Agent definitions — one row per agent identity. `managed` flips true once
  // Talaria renders + orchestrates the agent (Phase B); imported-only until then.
  `create table if not exists agent_defs (
     id uuid primary key default gen_random_uuid(),
     slug text unique not null,
     department text not null,
     model text unique not null,
     display_name text not null,
     enabled boolean not null default true,
     managed boolean not null default false,
     current_version integer not null default 0,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  // Local-vs-cloud attribution on the token ledger: which upstream model class
  // served the generation (from the agent's main endpoint at generation time).
  `alter table usage_events add column if not exists endpoint_class text`,
  `alter table usage_events add column if not exists llm_model text`,
  // 'imported' agents reuse the legacy stack's volumes/service chassis;
  // 'created' agents get fresh talaria-fleet volumes + a templated chassis.
  `alter table agent_defs add column if not exists source text not null default 'imported'`,
  // Curated model catalog per endpoint — what the agent editor's picker offers.
  `alter table llm_endpoints add column if not exists models jsonb not null default '[]'`,
  // Immutable version payloads — soul + structured config (main model, aliases,
  // fallbacks, toolsets, mcp servers, plugins, and the full raw config for
  // faithful rendering). Every edit is a new version: diffable, revertible.
  `create table if not exists agent_versions (
     id uuid primary key default gen_random_uuid(),
     agent_id uuid not null references agent_defs(id) on delete cascade,
     version integer not null,
     soul text not null default '',
     config jsonb not null default '{}',
     note text,
     created_by text,
     created_at timestamptz not null default now(),
     unique (agent_id, version)
   )`,
  // Backfill ledger attribution for rows recorded before endpoint_class
  // existed: they were all main-model turns, so the agent's current main
  // endpoint is historically accurate. No-op once attributed.
  `update usage_events u
   set endpoint_class = e.class, llm_model = (v.config->'main'->>'model')
   from agent_defs d
   join agent_versions v on v.agent_id = d.id and v.version = d.current_version
   join llm_endpoints e on e.name = (v.config->'main'->>'endpoint')
   where u.endpoint_class is null and u.agent_model = d.model`,
  // Auto-fetched prices (OpenRouter public catalog) - separate from user
  // overrides so a refresh never clobbers a hand-set rate.
  `alter table llm_endpoints add column if not exists auto_prices jsonb not null default '{}'`,
  // Pricing: per-model $/MTok overrides on the endpoint ({model: {in, out}});
  // endpoint-level price_in/out stay as the fallback. Cost is computed at read
  // time, so price edits reprice history automatically.
  `alter table llm_endpoints add column if not exists model_prices jsonb not null default '{}'`,
  // The serving ENDPOINT name per generation - exact price lookup.
  `alter table usage_events add column if not exists endpoint text`,
  // Backfill endpoint by matching the recorded llm_model across the agent's
  // current main + alias targets. No-op once set.
  `update usage_events u set endpoint = t.ep
   from (
     select d.model as agent_model, x.model as llm_model, x.endpoint as ep
     from agent_defs d
     join agent_versions v on v.agent_id = d.id and v.version = d.current_version
     cross join lateral (
       select v.config->'main'->>'model' as model, v.config->'main'->>'endpoint' as endpoint
       union all
       select a->>'model', a->>'endpoint' from jsonb_array_elements(coalesce(v.config->'aliases','[]'::jsonb)) a
     ) x
   ) t
   where u.endpoint is null and u.agent_model = t.agent_model and u.llm_model = t.llm_model`,
  // Per-ticket token spend: agents report usage against the ticket they're
  // working (MCP log_usage → POST /api/tasks/:id/usage).
  `alter table usage_events add column if not exists task_id uuid`,
  `create index if not exists usage_events_task_idx on usage_events(task_id) where task_id is not null`,
  // ── Talaria LLM gateway: one org endpoint over the whole model stack ────────
  // Per-user API keys (sha256 of the secret; plaintext shown exactly once).
  `create table if not exists llm_api_keys (
     id uuid primary key default gen_random_uuid(),
     user_id uuid not null references users(id) on delete cascade,
     name text not null,
     key_hash text unique not null,
     prefix text not null,
     created_at timestamptz not null default now(),
     last_used_at timestamptz,
     revoked_at timestamptz
   )`,
  // Who may mint keys (admins always may; this grants others).
  `alter table users add column if not exists can_mint_keys boolean not null default false`,
  // Extra request-body defaults deep-merged into every outbound call to this
  // endpoint (e.g. OpenRouter's provider allowlist / data_collection deny).
  `alter table llm_endpoints add column if not exists request_defaults jsonb not null default '{}'`,
  // ── Version history for agent internals (skills + memory) ───────────────────
  // Uniform snapshot store: kind ∈ {skill, memory}; owner_key is "<owner>/<name>"
  // for a skill or the agent def id for memory. One row per saved revision.
  `create table if not exists internal_versions (
     id uuid primary key default gen_random_uuid(),
     kind text not null,
     owner_key text not null,
     content text not null,
     created_by text,
     created_at timestamptz not null default now()
   )`,
  `create index if not exists internal_versions_idx on internal_versions(kind, owner_key, created_at desc)`,
  // A human-readable role/title per agent (e.g. "Support Lead"), shown on the
  // roster and editable in the manage modal. Distinct from `department` (the
  // routing/mount key) and `display_name` (the person-name).
  `alter table agent_defs add column if not exists role text`,
  // ── Attachments: uploaded files referenced by chat/channel messages ─────────
  `create table if not exists uploads (
     id uuid primary key default gen_random_uuid(),
     filename text not null,
     mime text not null,
     size integer not null,
     path text not null,
     uploaded_by uuid references users(id) on delete set null,
     created_at timestamptz not null default now()
   )`,
  // Attachment metadata carried on a message ([{id, filename, mime, size}]).
  `alter table messages add column if not exists attachments jsonb not null default '[]'`,
  `alter table channel_messages add column if not exists attachments jsonb not null default '[]'`,
  // Personal assistants: an agent owned by one user (their own Hermes agent),
  // spun up from the dashboard. Null = a shared fleet agent.
  `alter table agent_defs add column if not exists owner_user_id uuid references users(id) on delete set null`,
  // Per-view access: nav routes a member may NOT reach (deny list; empty = all
  // views, the open default). Admins are never restricted.
  `alter table users add column if not exists denied_views text[] not null default '{}'`,
  // ── Audit trail + app settings ──────────────────────────────────────────────
  // A real audit log: who did what to which target, with before/after state.
  `create table if not exists audit_log (
     id uuid primary key default gen_random_uuid(),
     actor text not null,
     action text not null,
     target_type text not null,
     target_id text,
     target_label text,
     before jsonb,
     after jsonb,
     created_at timestamptz not null default now()
   )`,
  `create index if not exists audit_log_created_idx on audit_log(created_at desc)`,
  `create index if not exists audit_log_target_idx on audit_log(target_type, target_id)`,
  // Key/value app settings (admin-editable). Audit retention lives here.
  `create table if not exists app_settings (
     key text primary key,
     value jsonb not null,
     updated_at timestamptz not null default now()
   )`,
  // ── Retrieval (RAG) registry ────────────────────────────────────────────────
  // Talaria owns retrieval as a registry of collections. kind: 'activity' (the
  // auto ambient workspace index) | 'org-kb' (the auto curated knowledgebase) |
  // 'custom' (admin/user-created, e.g. departmental). Each maps to one Qdrant
  // collection. auto collections can't be deleted.
  `create table if not exists rag_collections (
     id uuid primary key default gen_random_uuid(),
     name text not null,
     kind text not null,
     qdrant_name text unique not null,
     description text,
     auto boolean not null default false,
     embed_dim integer,
     created_by text,
     created_at timestamptz not null default now()
   )`,
  // Who a collection is bound to. principal_type: 'all' (everyone) | 'user' |
  // 'agent'. principal_id null when 'all'. A collection can have many bindings
  // (sets of users/agents = departmental knowledge).
  `create table if not exists rag_collection_access (
     collection_id uuid not null references rag_collections(id) on delete cascade,
     principal_type text not null,
     principal_id text,
     unique (collection_id, principal_type, principal_id)
   )`,
  // Index bookkeeping: what's been embedded into which collection, so re-index
  // is idempotent and deletes propagate. A source doc becomes many chunk points;
  // point_ids holds their Qdrant ids, content_hash gates re-embedding.
  `create table if not exists rag_points (
     id uuid primary key default gen_random_uuid(),
     collection_id uuid not null references rag_collections(id) on delete cascade,
     source_type text not null,
     source_id text not null,
     point_ids jsonb not null default '[]',
     content_hash text not null,
     updated_at timestamptz not null default now(),
     unique (collection_id, source_type, source_id)
   )`,
  // ── Knowledgebase (Outline-style markdown drive) ────────────────────────────
  // Spaces group docs; docs nest via parent_id. kind: 'human' (freeform) |
  // 'agent' (OKF-structured for machine consumption). visibility: private
  // (creator) | org (all members) | public (anyone via the public route).
  // official docs are indexed into the org-kb RAG collection.
  `create table if not exists kb_spaces (
     id uuid primary key default gen_random_uuid(),
     name text not null,
     description text,
     icon text,
     created_by text,
     created_at timestamptz not null default now()
   )`,
  `create table if not exists kb_docs (
     id uuid primary key default gen_random_uuid(),
     space_id uuid not null references kb_spaces(id) on delete cascade,
     parent_id uuid references kb_docs(id) on delete set null,
     title text not null default 'Untitled',
     body text not null default '',
     kind text not null default 'human',
     official boolean not null default false,
     visibility text not null default 'org',
     public_slug text unique,
     sort integer not null default 0,
     created_by text,
     updated_by text,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  `create index if not exists kb_docs_space_idx on kb_docs(space_id, parent_id, sort)`,
  // Outline-parity: per-doc emoji icon, and a full-text search index over
  // title + body so the knowledgebase is searchable at scale.
  `alter table kb_docs add column if not exists icon text`,
  `create index if not exists kb_docs_fts_idx on kb_docs using gin (
     to_tsvector('english', coalesce(title,'') || ' ' || coalesce(body,'')))`,
  // Spaces double as top-level documents: an editable overview body. And docs
  // carry their owner's user id so private docs can route to a personal RAG.
  `alter table kb_spaces add column if not exists body text not null default ''`,
  `alter table kb_docs add column if not exists owner_user_id uuid references users(id) on delete set null`,
  // Personal RAG collections belong to a user (created with their assistant).
  `alter table rag_collections add column if not exists owner_user_id uuid references users(id) on delete cascade`,
  // Permissions: folders (spaces) get the same read-visibility model as docs,
  // and both docs and folders get an edit policy. edit_policy ∈
  //   'owner'      — only the owner may edit
  //   'org'        — any member who can read may edit (agents still need a grant)
  //   'restricted' — the owner + an explicit editor list (users and/or agents)
  `alter table kb_spaces add column if not exists owner_user_id uuid references users(id) on delete set null`,
  `alter table kb_spaces add column if not exists visibility text not null default 'org'`,
  `alter table kb_spaces add column if not exists public_slug text unique`,
  `alter table kb_spaces add column if not exists edit_policy text not null default 'org'`,
  `alter table kb_docs add column if not exists edit_policy text not null default 'org'`,
  // The explicit editor grants for a doc or space (used when edit_policy =
  // 'restricted'). principal_type ∈ 'user' | 'agent'.
  `create table if not exists kb_editors (
     item_type text not null,
     item_id uuid not null,
     principal_type text not null,
     principal_id text not null,
     primary key (item_type, item_id, principal_type, principal_id)
   )`,
  `create index if not exists kb_editors_item_idx on kb_editors(item_type, item_id)`,
  // Each grant now carries a role: 'viewer' (can see) or 'editor' (can see +
  // edit). Existing rows were all editors.
  `alter table kb_editors add column if not exists role text not null default 'editor'`,
  // Docs inherit their audience (visibility / edit policy / grants) from their
  // folder unless individually customized. The creator always keeps ownership.
  `alter table kb_docs add column if not exists perms_inherited boolean not null default true`,
  // Artifacts — versioned work products (doc/sheet/microsite/file) with their
  // own hosting, sharing (reusing kb_editors with item_type='artifact' + the
  // same visibility/edit-policy model), and versioning (internal_versions kind
  // 'artifact'). Promoting one to "official" mirrors it into the knowledgebase.
  `create table if not exists artifacts (
     id uuid primary key default gen_random_uuid(),
     kind text not null default 'doc',
     title text not null default 'Untitled',
     icon text,
     body text not null default '',
     content_type text,
     storage_ref text,
     visibility text not null default 'private',
     edit_policy text not null default 'owner',
     public_slug text unique,
     official boolean not null default false,
     kb_doc_id uuid references kb_docs(id) on delete set null,
     owner_user_id uuid references users(id) on delete set null,
     created_by text,
     updated_by text,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  `create index if not exists artifacts_owner_idx on artifacts(owner_user_id)`,
  // An artifact can be attached to anything (a KB doc/folder, a ticket, a
  // channel, …). target_type namespaces the id.
  `create table if not exists artifact_links (
     artifact_id uuid not null references artifacts(id) on delete cascade,
     target_type text not null,
     target_id text not null,
     created_by text,
     created_at timestamptz not null default now(),
     primary key (artifact_id, target_type, target_id)
   )`,
  `create index if not exists artifact_links_target_idx on artifact_links(target_type, target_id)`,
  // Organize artifacts into a nestable folder tree (org-wide organizational
  // containers; artifacts inside stay gated by their own sharing).
  `create table if not exists artifact_folders (
     id uuid primary key default gen_random_uuid(),
     name text not null default 'Untitled',
     icon text,
     parent_id uuid references artifact_folders(id) on delete set null,
     created_by text,
     created_at timestamptz not null default now()
   )`,
  `alter table artifacts add column if not exists folder_id uuid references artifact_folders(id) on delete set null`,
  `create index if not exists artifacts_folder_idx on artifacts(folder_id)`,
  // Preferred model for AI drafting (muse) — a gateway model id, e.g.
  // "pl-main" or "anthropic/claude-sonnet-5". Null = the server default.
  `alter table users add column if not exists preferred_model text`,
  // Per-agent secrets, configured in the UI and stored ENCRYPTED (secretbox).
  // Materialized only at render time into the agent's env file — no hand
  // edits to fleet/.env required for per-agent credentials.
  `create table if not exists agent_secrets (
     agent_id uuid not null references agent_defs(id) on delete cascade,
     name text not null,
     value_enc text not null,
     updated_by text,
     updated_at timestamptz not null default now(),
     primary key (agent_id, name)
   )`,

  // Per-user Google (Workspace) connection for Drive/Docs. Tokens are stored
  // ENCRYPTED (see server/secretbox.ts) — refresh_token is a live credential, not
  // an env-var name. One row per user; connecting again replaces it.
  `create table if not exists google_connections (
     user_id uuid primary key references users(id) on delete cascade,
     google_sub text not null,
     email text,
     scope text not null default '',
     refresh_token_enc text,
     access_token_enc text,
     access_expires_at timestamptz,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  // Where an artifact has been mirrored into the owner's Google Drive.
  `alter table artifacts add column if not exists google_file_id text`,
  `alter table artifacts add column if not exists google_file_url text`,

  // A single SHARED org-wide Google connection (admin-configured). General
  // fleet agents (no human owner) act as this identity for Drive/Docs; personal
  // assistants act as their own owner. Singleton: id is pinned to 1.
  `create table if not exists google_org_connection (
     id integer primary key default 1 check (id = 1),
     google_sub text not null,
     email text,
     scope text not null default '',
     refresh_token_enc text,
     access_token_enc text,
     access_expires_at timestamptz,
     connected_by uuid references users(id) on delete set null,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,

  // Outbound Google actions an agent DRAFTED that need human sign-off before they
  // go out (send email, create a calendar event). The owner approves; on approval
  // it executes as the owner's Google. Reads + drafts are free; sends are gated.
  `create table if not exists google_pending_actions (
     id uuid primary key default gen_random_uuid(),
     kind text not null,
     payload jsonb not null,
     summary text,
     agent_model text,
     owner_user_id uuid references users(id) on delete cascade,
     status text not null default 'pending',
     result jsonb,
     created_at timestamptz not null default now(),
     decided_at timestamptz,
     decided_by uuid references users(id) on delete set null
   )`,
  `create index if not exists google_pending_owner_idx on google_pending_actions(owner_user_id, status)`,
  // Org-scoped pending actions (drafted by a general agent for the shared org
  // Google account) have no owner — an admin approves them instead.
  `alter table google_pending_actions add column if not exists is_org boolean not null default false`,
  `create index if not exists google_pending_org_idx on google_pending_actions(is_org, status)`,

  // Where the org account's agents build. drive_folder_id → a Shared Drive (or
  // folder) so org files are team-owned; calendar_id → the calendar org events
  // land on (default 'primary'); send_as → a verified send-as alias for org mail.
  `alter table google_org_connection add column if not exists drive_folder_id text`,
  `alter table google_org_connection add column if not exists calendar_id text`,
  `alter table google_org_connection add column if not exists send_as text`,

  // Automated QA judge verdicts on a ticket at the quality-review gate. Advisory:
  // the judge reviews agent-reported work and posts a verdict + issues; the human
  // still decides. verdict: pass | revise | escalate.
  `create table if not exists judge_reviews (
     id uuid primary key default gen_random_uuid(),
     task_id uuid not null references tasks(id) on delete cascade,
     model text,
     verdict text not null,
     summary text not null default '',
     issues jsonb not null default '[]',
     created_at timestamptz not null default now()
   )`,
  `create index if not exists judge_reviews_task_idx on judge_reviews(task_id, created_at desc)`,
  // Per-board judge mode: inherit (global default) | off | advisory. (Enforcing
  // revision-loop is a planned mode; advisory ships first.)
  `alter table boards add column if not exists judge_mode text not null default 'inherit'`,

  // Confab-guard findings — structural checks on model output at the gateway
  // (see server/guardrails.ts). Observe-mode records here without touching the
  // model's output or context.
  `create table if not exists guard_findings (
     id uuid primary key default gen_random_uuid(),
     caller text,
     model text,
     endpoint text,
     mode text not null default 'observe',
     check_type text not null,
     severity text not null default 'medium',
     message text not null default '',
     snippet text not null default '',
     created_at timestamptz not null default now()
   )`,
  `create index if not exists guard_findings_recent_idx on guard_findings(created_at desc)`,
  `alter table guard_findings add column if not exists confidence real not null default 0.5`,
  // Provider API keys, encrypted at rest (secretbox) — the durable, secure store.
  // api_key_env stays as an optional ops override; keys no longer live in configs.
  `alter table llm_endpoints add column if not exists api_key_cipher text`,
  // Envelope encryption: the wrapped data-encryption key(s). One active row; old
  // versions kept for audit. The DEK is never stored unwrapped.
  `create table if not exists secret_keys (
     version int primary key,
     wrapped_dek text not null,
     active boolean not null default true,
     created_at timestamptz not null default now()
   )`,
  // Stable host port per agent, so the app (on the host) reaches each agent's
  // persona gateway directly — no separate bridge/multiplexer container.
  `alter table agent_defs add column if not exists gateway_port int`,
  // Conversation kind — 'chat' (default) or 'plan' (the planning surface).
  `alter table conversations add column if not exists kind text not null default 'chat'`,
  // Ticket/plan templates — an org-wide library of markdown skeletons + prompt
  // guidance. Tickets and plan docs stay markdown; the skeleton IS the schema.
  `create table if not exists templates (
     id uuid primary key default gen_random_uuid(),
     name text not null,
     kind text not null,
     body text not null default '',
     guidance text not null default '',
     created_by text,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  // Which templates a board uses (and its default). Kind 'ticket' bindings only.
  `create table if not exists board_templates (
     board_id uuid not null references boards(id) on delete cascade,
     template_id uuid not null references templates(id) on delete cascade,
     is_default boolean not null default false,
     primary key (board_id, template_id)
   )`,
  // Agent template overrides — "this agent always writes eng tickets". Part of
  // the resolve chain: explicit pick → agent binding → board default → none.
  `alter table agent_defs add column if not exists ticket_template_id uuid references templates(id) on delete set null`,
  `alter table agent_defs add column if not exists plan_template_id uuid references templates(id) on delete set null`,
  // Model blurbs, rewritten once in the org's voice (task-oriented one-liners
  // for pickers) and cached here; new models get theirs on the next sweep.
  `create table if not exists model_blurbs (
     model_id text primary key,
     blurb text not null,
     created_at timestamptz not null default now()
   )`,
  // Rolling replacement: each managed agent runs in one of two compose slots
  // ('a' → agent-<dept>, 'b' → agent-<dept>-b). A roll brings the other slot up
  // on a fresh port, cuts the manifest over after health, then retires the old
  // container — identity/config changes never take an agent away mid-reply.
  `alter table agent_defs add column if not exists active_slot text not null default 'a'`,
  // Comms unification: channels carry a kind — 'channel' (persistent, ambient),
  // 'group' (a Relay: named ad-hoc gathering that concludes and archives), or
  // 'dm' (human↔human direct messages). DMs dedupe on the sorted user-id pair.
  `alter table channels add column if not exists kind text not null default 'channel'`,
  `alter table channels add column if not exists dm_key text`,
  `create unique index if not exists channels_dm_key_idx on channels(dm_key) where dm_key is not null`,
  // Elevated personal assistants: an admin can promote an admin's assistant to
  // org-wide view/edit (all boards, all non-DM channels, implicit editor on
  // non-private KB/artifacts). Only effective while the owner is an admin.
  `alter table agent_defs add column if not exists elevated boolean not null default false`,
  // Per-member read cursor → unread badges in the Comms sidebar.
  `alter table channel_members add column if not exists last_read_seq integer not null default 0`,
]

function ensureMigrated(): Promise<void> {
  if (!g.__talariaMigrated) {
    const sql = getSql()
    g.__talariaMigrated = (async () => {
      for (const stmt of MIGRATIONS) await sql.unsafe(stmt)
      // Load/create the data key so seal()/open() are synchronous thereafter.
      await initSecretbox(sql)
    })()
  }
  return g.__talariaMigrated
}

/** Migrated Postgres handle. `const sql = await db()`. */
export async function db(): Promise<Sql> {
  await ensureMigrated()
  return getSql()
}
