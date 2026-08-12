// Postgres — durable state (users, roles, per-agent access, conversations,
// messages). postgres.js (no native build). Migrations run lazily on the first
// query, once per statement ever (see schema_migrations below), under an
// advisory lock. Cached on globalThis so HMR doesn't open a new pool each reload.

import { createHash } from 'node:crypto'
import postgres from 'postgres'
import { initSecretbox } from '../secretbox'

type Sql = ReturnType<typeof postgres>
const g = globalThis as unknown as { __talariaSql?: Sql; __talariaMigrated?: Promise<void> }

function databaseUrl(): string {
  const url = process.env.DATABASE_URL
  if (!url) throw new Error('DATABASE_URL is not set')
  return url
}

/** The APPLICATION pool. Every request handler and every scheduled job runs on
 *  this one. `idle_timeout` lets the driver reap connections a quiet instance
 *  is not using — see runMigrations for why nothing may `reserve()` it. */
export function getSql(): Sql {
  if (!g.__talariaSql) {
    g.__talariaSql = postgres(databaseUrl(), { max: 10, idle_timeout: 20, onnotice: () => {} })
  }
  return g.__talariaSql
}

// One statement per entry (postgres.js extended protocol is one-statement).
// APPEND-ONLY: a statement's index is its identity in schema_migrations, so new
// migrations go at the END — never inserted next to related ones. Editing or
// inserting mid-array trips the checksum check and the app refuses to boot.
// Each entry runs exactly once per database; DML here is a one-shot backfill,
// not a rule that re-asserts itself on every start.
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
  // Activity/audit log for a task (created, status change, assigned, comment, ).
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
  `alter table users add column if not exists allowed_manage_views text[] not null default '{}'`,
  `create table if not exists invites (
    id uuid primary key default gen_random_uuid(),
    email text not null,
    token text not null unique,
    invited_by text,
    created_at timestamptz not null default now(),
    expires_at timestamptz not null,
    accepted_at timestamptz,
    accepted_user_id uuid references users(id) on delete set null,
    revoked_at timestamptz
  )`,
  `create index if not exists invites_email_idx on invites(email)`,
  `create table if not exists app_data (
    app text not null,
    collection text not null,
    id uuid not null default gen_random_uuid(),
    data jsonb not null default '{}',
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (app, collection, id)
  )`,
  `create index if not exists app_data_updated_idx on app_data(app, collection, updated_at desc)`,
  `create table if not exists org_domains (
    id uuid primary key default gen_random_uuid(),
    domain text not null unique,
    verified boolean not null default false,
    verification_token text not null,
    added_by text,
    created_at timestamptz not null default now(),
    verified_at timestamptz
  )`,
  `create table if not exists mcp_servers (
    id uuid primary key default gen_random_uuid(),
    name text not null unique,
    label text not null,
    description text,
    url text not null,
    headers jsonb not null default '{}',
    timeout_secs int,
    enabled boolean not null default true,
    all_agents boolean not null default false,
    auth_mode text not null default 'org',
    tools jsonb not null default '[]',
    tools_refreshed_at timestamptz,
    created_by text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
  )`,
  `alter table mcp_servers add column if not exists required_headers jsonb not null default '[]'`,
  `alter table mcp_servers add column if not exists builtin boolean not null default false`,
  `alter table mcp_servers add column if not exists oauth jsonb`,
  `alter table mcp_servers add column if not exists app_slug text`,
  `create table if not exists mcp_oauth_states (
    state text primary key,
    server_id uuid not null references mcp_servers(id) on delete cascade,
    subject text not null,
    verifier text not null,
    redirect_uri text not null,
    created_at timestamptz not null default now()
  )`,
  `create table if not exists mcp_oauth_tokens (
    server_id uuid not null references mcp_servers(id) on delete cascade,
    subject text not null,
    tokens_enc text not null,
    updated_at timestamptz not null default now(),
    primary key (server_id, subject)
  )`,
  `create table if not exists mcp_server_agents (
    server_id uuid not null references mcp_servers(id) on delete cascade,
    agent_model text not null,
    tools text[],
    primary key (server_id, agent_model)
  )`,
  `create table if not exists mcp_user_access (
    server_id uuid not null references mcp_servers(id) on delete cascade,
    user_id uuid not null references users(id) on delete cascade,
    allowed boolean not null default true,
    tools text[],
    primary key (server_id, user_id)
  )`,
  `create table if not exists mcp_user_credentials (
    server_id uuid not null references mcp_servers(id) on delete cascade,
    user_id uuid not null references users(id) on delete cascade,
    headers_enc text not null,
    updated_at timestamptz not null default now(),
    primary key (server_id, user_id)
  )`,
  `create table if not exists user_permissions (
    user_id uuid not null references users(id) on delete cascade,
    perm text not null,
    allowed boolean not null,
    created_at timestamptz not null default now(),
    primary key (user_id, perm)
  )`,
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
  // Threads hang off a root message (a reply carries the root's id); edited_at
  // marks a message the author revised in place.
  `alter table channel_messages add column if not exists thread_root_id uuid references channel_messages(id) on delete cascade`,
  `alter table channel_messages add column if not exists edited_at timestamptz`,
  `create index if not exists channel_messages_thread_idx on channel_messages(thread_root_id) where thread_root_id is not null`,
  // Emoji reactions; actor is the user's email or the agent model.
  `create table if not exists channel_message_reactions (
    message_id uuid not null references channel_messages(id) on delete cascade,
    emoji text not null,
    actor text not null,
    actor_type text not null default 'user',
    created_at timestamptz not null default now(),
    primary key (message_id, emoji, actor)
  )`,
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
  // agent_defs is the registry of record: drop 1:1 chat grants for models that
  // are no longer defined.
  `delete from user_agent_access where not exists (select 1 from agent_defs d where d.model = agent_model)`,
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
  // Carry the flag into the permission catalog, which superseded it.
  `insert into user_permissions (user_id, perm, allowed)
     select id, 'models.mint-keys', true from users where can_mint_keys
     on conflict (user_id, perm) do nothing`,
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
  // A space's OKF concept doc — the machine-readable summary of the space,
  // kept as a normal (hidden) doc so the Librarian edits it like any other.
  `alter table kb_spaces add column if not exists okf_doc_id uuid references kb_docs(id) on delete set null`,
  // Hidden agent-facing OKF body on a doc. Only a space's OKF doc is truly
  // 'agent' kind; everything else that predates the split is human.
  `alter table kb_docs add column if not exists okf text`,
  `update kb_docs set kind='human' where kind='agent' and id not in (select okf_doc_id from kb_spaces where okf_doc_id is not null)`,
  // Inline comments on a doc — threaded, optionally anchored to a quote.
  `create table if not exists kb_comments (
    id uuid primary key default gen_random_uuid(),
    doc_id uuid not null references kb_docs(id) on delete cascade,
    parent_id uuid references kb_comments(id) on delete cascade,
    author_user_id uuid references users(id) on delete set null,
    author text not null,
    quote text,
    content text not null,
    resolved boolean not null default false,
    created_at timestamptz not null default now()
  )`,
  `create index if not exists kb_comments_doc_idx on kb_comments(doc_id, created_at)`,
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
  // channel, ). target_type namespaces the id.
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
  // Multiplayer plans: collaborators on a plan conversation (the owner stays
  // conversations.user_id). Chats remain strictly private — members are only
  // ever consulted for kind='plan'.
  `create table if not exists conversation_members (
     conversation_id uuid not null references conversations(id) on delete cascade,
     user_id uuid not null references users(id) on delete cascade,
     created_at timestamptz not null default now(),
     primary key (conversation_id, user_id)
   )`,
  // Who wrote a user turn — multiplayer plans need voices told apart.
  `alter table messages add column if not exists author_user_id uuid references users(id) on delete set null`,
  // Research runs: cited research pipelines (Recon / Brief / Expedition). The
  // report itself is a doc artifact; sources carry the [n] citation registry.
  `create table if not exists research_runs (
     id uuid primary key default gen_random_uuid(),
     owner_user_id uuid references users(id) on delete set null,
     requested_by text not null,
     agent_model text not null,
     mode text not null,
     question text not null,
     status text not null default 'queued',
     phase text,
     artifact_id uuid references artifacts(id) on delete set null,
     error text,
     stats jsonb not null default '{}',
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now(),
     completed_at timestamptz
   )`,
  `create index if not exists research_runs_created_idx on research_runs(created_at desc)`,
  // A generated short title for the run; the UI shows the raw question until
  // it lands.
  `alter table research_runs add column if not exists title text`,
  `create table if not exists research_sources (
     id uuid primary key default gen_random_uuid(),
     run_id uuid not null references research_runs(id) on delete cascade,
     idx integer not null,
     url text not null,
     title text,
     snippet text,
     created_at timestamptz not null default now(),
     unique (run_id, idx)
   )`,
  // RAG curation: a KB space can feed a specific brain (custom collection) —
  // every non-private doc in the space indexes there instead of the org brain.
  `alter table kb_spaces add column if not exists rag_collection_id uuid references rag_collections(id) on delete set null`,
  // Per-doc routing override: 'auto' (space binding / org rules), 'none'
  // (never index), or a collection uuid (explicit brain assignment).
  `alter table kb_docs add column if not exists rag_routing text not null default 'auto'`,
  // The same control on artifacts: 'auto' (plan/research activity flows +
  // officialize pipeline), 'none', or an explicit brain.
  `alter table artifacts add column if not exists rag_routing text not null default 'auto'`,
  // Confab-guard findings pinned to the reply they flagged (annotate/strict
  // modes). Metadata only — rendered as a caveat in the UI, never part of the
  // content column, so transcripts rebuilt from content stay uncontaminated.
  `alter table messages add column if not exists guard jsonb`,
  `alter table channel_messages add column if not exists guard jsonb`,
  // Retrieval schema generation: 1 = legacy unnamed dense vector; 2 = hybrid
  // (named dense + IDF sparse). The guided reindex upgrades collections in place.
  `alter table rag_collections add column if not exists schema_version integer not null default 1`,
  // Ticket attachments: same shape as message attachments (uploads + ref chips).
  `alter table tasks add column if not exists attachments jsonb not null default '[]'`,
  // Sub-tasks: one level deep (a parent cannot itself be a child — enforced in
  // code). Deleting a parent releases its children back to top level.
  `alter table tasks add column if not exists parent_id uuid references tasks(id) on delete set null`,
  `create index if not exists tasks_parent_idx on tasks(parent_id)`,
  // Gantt scheduling: optional start (bars run start → due).
  `alter table tasks add column if not exists start_date timestamptz`,
  // Ticket color-coding (palette key; null = status/priority defaults).
  `alter table tasks add column if not exists color text`,
  // Task workflows — task-classified hooks: when an agent picks up a ticket
  // that MATCHES (labels / boards / title keywords), the workflow rides along
  // with the work: skills (Hermes skills that ARE the flow), declared
  // toolkits (MCP servers / tool subsets the work expects), and a reserved
  // env block for sandbox profiles (the future custom-runtime layer).
  `create table if not exists task_workflows (
    id uuid primary key default gen_random_uuid(),
    name text not null,
    description text not null default '',
    enabled boolean not null default true,
    match jsonb not null default '{}',
    skills jsonb not null default '[]',
    toolkits jsonb not null default '[]',
    env jsonb not null default '{}',
    position int not null default 0,
    created_by text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
  )`,
  // Workflows bind to Hermes SKILLS (the flow content lives in the skill
  // library the agents already mount) — no freeform instruction prose here.
  `alter table task_workflows add column if not exists skills jsonb not null default '[]'`,
  `alter table task_workflows drop column if exists instructions`,
  // Capability gaps — the honesty loop. An agent that genuinely can't do
  // assigned work properly reports the gap ONCE per work-shape (signature);
  // repeats only bump seen_count (frequency = ranking, never re-notification).
  // The Studio's Suggested queue turns open gaps into skill/workflow drafts.
  // Workbench runtime profiles — the role-agnostic sandbox methodology.
  // A profile is a chassis overlay: image + env + mounts + the harnesses it
  // preinstalls, plus autoAttach fit rules (departments/roles/toolkits) and
  // room for the later phases (creds scoping, toolkit, effort routing) in
  // config. 'dev' ships seeded; designer/data/marketing ride the same table.
  `create table if not exists workbench_profiles (
    slug text primary key,
    name text not null,
    description text not null default '',
    image text not null default '',
    env jsonb not null default '{}',
    mounts jsonb not null default '[]',
    harnesses jsonb not null default '[]',
    auto_attach jsonb not null default '{}',
    config jsonb not null default '{}',
    enabled boolean not null default true,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
  )`,
  // Per-agent workbench control — THE simple setting: off | auto | on.
  `alter table agent_defs add column if not exists workbench text not null default 'auto'`,
  `alter table agent_defs add column if not exists workbench_profile text`,
  // Per-agent harness pick (null = profile default) and per-agent
  // effort→model overrides ({light,standard,heavy}; unset keys fall back to
  // the global Workbench model roles).
  `alter table agent_defs add column if not exists workbench_harness text`,
  `alter table agent_defs add column if not exists workbench_models jsonb not null default '{}'`,
  // Workbench repo grants — explicit per-agent GitHub repo access, like MCP
  // assignment: connecting GitHub grants nothing until an admin grants repos.
  `create table if not exists workbench_repos (
    agent_id uuid not null references agent_defs(id) on delete cascade,
    repo text not null,
    created_at timestamptz not null default now(),
    primary key (agent_id, repo)
  )`,
  // Workbench jobs — the platform-owned execution lifecycle. One row per
  // start_job: the branch is Talaria's (cut via API at start), the PR opens
  // at finish with the templated ticket-linked body. Agents never touch
  // origin outside this flow.
  `create table if not exists workbench_jobs (
    id uuid primary key default gen_random_uuid(),
    agent_id uuid not null references agent_defs(id) on delete cascade,
    agent_model text not null,
    task_id uuid references tasks(id) on delete set null,
    repo text not null,
    branch text not null,
    effort text not null default 'standard',
    plan text not null default '',
    status text not null default 'started',
    pr_url text,
    summary text not null default '',
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
  )`,
  // Per-repo git flow: which branch PRs target (null = the repo's default)
  // and an optional TESTING branch features can be merged into for
  // integration testing before the PR merges. Org-level — a repo's flow is
  // the repo's flow, not per agent.
  `create table if not exists workbench_repo_flow (
    repo text primary key,
    base_branch text,
    testing_branch text,
    updated_at timestamptz not null default now()
  )`,
  `alter table workbench_jobs add column if not exists merged_testing_at timestamptz`,
  // Admin-registered custom workbench harnesses (declarative JSON matching
  // the SDK HarnessDefinition — no code). Merged over builtin + app-shipped
  // definitions by slug.
  `create table if not exists workbench_harness_defs (
    slug text primary key,
    definition jsonb not null,
    enabled boolean not null default true,
    created_by text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
  )`,
  // Agent repo-creation requests — agents propose, humans ratify. Approval
  // creates the repo via the App (needs org Administration permission),
  // auto-grants it to the requester, and audits everything.
  `create table if not exists workbench_repo_requests (
    id uuid primary key default gen_random_uuid(),
    agent_id uuid not null references agent_defs(id) on delete cascade,
    agent_model text not null,
    org text not null,
    name text not null,
    description text not null default '',
    why text not null default '',
    task_id uuid references tasks(id) on delete set null,
    status text not null default 'pending',
    decided_by text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
  )`,
  // Persistent skill summaries — one generated line per skill, keyed to a
  // hash of its SKILL.md so it only regenerates when the content changes.
  `create table if not exists skill_summaries (
    owner text not null,
    name text not null,
    hash text not null,
    summary text not null,
    updated_at timestamptz not null default now(),
    primary key (owner, name)
  )`,
  `create table if not exists capability_gaps (
    id uuid primary key default gen_random_uuid(),
    signature text not null unique,
    kind text not null,
    board_id uuid references boards(id) on delete set null,
    agent_model text not null,
    missing text not null,
    needs text not null default '',
    example_task_id uuid references tasks(id) on delete set null,
    seen_count int not null default 1,
    status text not null default 'open',
    created_at timestamptz not null default now(),
    last_seen timestamptz not null default now()
  )`,
  // Custom board statuses. category carries the workflow semantics: open
  // (intake), active (working), review (the agent-review catch), done
  // (terminal). agent_start marks columns that constitute assignment approval
  // for agents (heartbeat pickup). BLOCKED is a system status — always
  // present, never stored here. No rows = the shipped default set.
  `create table if not exists board_statuses (
    id uuid primary key default gen_random_uuid(),
    board_id uuid not null references boards(id) on delete cascade,
    key text not null,
    label text not null,
    color text not null default 'slate',
    category text not null default 'active',
    agent_start boolean not null default false,
    position int not null default 0,
    created_at timestamptz not null default now(),
    unique (board_id, key)
  )`,
  // First-class board labels: named + colored, scoped to a board. Task.tags
  // stays a string array of label NAMES (existing data keeps working); the
  // registry makes them colorable and manageable. Backfilled from live tags.
  `create table if not exists board_labels (
    id uuid primary key default gen_random_uuid(),
    board_id uuid not null references boards(id) on delete cascade,
    name text not null,
    color text not null default 'slate',
    position int not null default 0,
    created_at timestamptz not null default now(),
    unique (board_id, name)
  )`,
  `insert into board_labels (board_id, name)
    select distinct t.board_id, e.name from tasks t, jsonb_array_elements_text(t.tags) as e(name)
    on conflict (board_id, name) do nothing`,
  // Saved board views: named filter/layout presets shared with the board.
  `create table if not exists board_views (
    id uuid primary key default gen_random_uuid(),
    board_id uuid not null references boards(id) on delete cascade,
    name text not null,
    config jsonb not null default '{}',
    created_by text not null,
    position int not null default 0,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
  )`,
  `create index if not exists board_views_board_idx on board_views(board_id, position)`,
  // Explicit per-plan template pick (mirrors agent_defs.plan_template_id, which
  // is the fallback). Set at creation; resolveTemplate treats it as the highest
  // link. Dead refs degrade to the agent binding via on-delete-set-null.
  `alter table conversations add column if not exists plan_template_id uuid references templates(id) on delete set null`,
  // Proactive outreach (#59): per-agent opt-in, plus one log of agent-initiated
  // contact (sweep check-ins + DMs) powering rate caps, repeat-avoidance
  // context, and admin visibility.
  `alter table agent_defs add column if not exists proactive boolean not null default false`,
  `create table if not exists outreach_events (
     id uuid primary key default gen_random_uuid(),
     agent_model text not null,
     kind text not null,
     user_id uuid references users(id) on delete cascade,
     conversation_id uuid references conversations(id) on delete set null,
     note text,
     created_at timestamptz not null default now()
   )`,
  `create index if not exists outreach_events_agent_idx on outreach_events (agent_model, kind, created_at desc)`,
  // Live inference dashboard (#48): "generating right now" scans for
  // streaming rows — tiny partial indexes keep that O(in-flight).
  `create index if not exists messages_streaming_idx on messages (created_at) where status = 'streaming'`,
  `create index if not exists channel_messages_streaming_idx on channel_messages (created_at) where status = 'streaming'`,
  // Research is multiplayer like plans: members see the run + report.
  `create table if not exists research_members (
     run_id uuid not null references research_runs(id) on delete cascade,
     user_id uuid not null references users(id) on delete cascade,
     created_at timestamptz not null default now(),
     primary key (run_id, user_id)
   )`,
  // Inbox briefing: the assistant's attention summary, regenerated only when
  // the attention fingerprint actually changes.
  `create table if not exists briefings (
     user_id uuid not null references users(id) on delete cascade,
     scope text not null default 'inbox',
     fingerprint text not null,
     conversation_id uuid references conversations(id) on delete set null,
     message_id uuid,
     summary text not null default '',
     generating boolean not null default false,
     generated_at timestamptz not null default now(),
     primary key (user_id, scope)
   )`,
  // Per-agent credentials: the secret an agent presents to prove WHO it is
  // (agent-auth), replacing the org-wide key + self-declared x-agent-name.
  // sha256 for lookup (auth never decrypts); a sealed copy so a wiped
  // fleet/.env can be re-materialized without rotating every container.
  `create table if not exists agent_keys (
     agent_id uuid primary key references agent_defs(id) on delete cascade,
     key_hash text not null unique,
     key_enc text not null,
     created_at timestamptz not null default now(),
     last_used_at timestamptz
   )`,

  // ── Merged from origin/main #202 (Mercury workspace + focus queue) ──
  // APPENDED, not merged in place. Their branch fixed the same fresh-install P0
  // by moving the forward-referencing statements to the TAIL; ours moved each
  // after its dependency and added index-keyed checksums. Adopting their order
  // would shift every index and make the checksum guard refuse to boot on any
  // database that already ran ours. Their ordering fix is therefore dropped as
  // redundant, and only their genuinely NEW statements land here — at the end,
  // which is the append-only rule this runner enforces.
  `create index if not exists messages_conversation_timeline_idx
     on messages(conversation_id, created_at desc, id desc)`,
  `alter table messages add column if not exists metadata jsonb not null default '{}'`,
  `create table if not exists inbox_focus_state (
     user_id uuid not null references users(id) on delete cascade,
     source_type text not null,
     source_id text not null,
     snoozed_until timestamptz,
     viewed_at timestamptz,
     content_fingerprint text,
     brief jsonb,
     brief_generated_at timestamptz,
     updated_at timestamptz not null default now(),
     primary key (user_id, source_type, source_id)
   )`,
  `create index if not exists inbox_focus_state_snooze_idx
     on inbox_focus_state(user_id, snoozed_until)`,
  `create table if not exists inbox_decisions (
     id uuid primary key default gen_random_uuid(),
     user_id uuid not null references users(id) on delete cascade,
     source_type text not null,
     source_id text not null,
     instruction text,
     action_id text,
     agent_model text,
     delegate_model text,
     status text not null,
     proposal jsonb,
     outcome jsonb,
     confirmation_token_hash text,
     expires_at timestamptz,
     created_at timestamptz not null default now(),
     confirmed_at timestamptz,
     completed_at timestamptz
   )`,
  `create index if not exists inbox_decisions_user_idx
     on inbox_decisions(user_id, created_at desc)`,
  `create index if not exists inbox_decisions_confirmation_idx
     on inbox_decisions(user_id, confirmation_token_hash) where status = 'proposed'`,
  `alter table inbox_decisions add column if not exists conversation_id uuid references conversations(id) on delete set null`,
  `alter table inbox_decisions add column if not exists user_message_id uuid references messages(id) on delete set null`,
  `alter table inbox_decisions add column if not exists assistant_message_id uuid references messages(id) on delete set null`,
  `alter table inbox_decisions add column if not exists focus_context jsonb`,
  `create unique index if not exists conversations_inbox_user_idx
     on conversations(user_id) where kind = 'inbox' and archived = false`,
  `create index if not exists inbox_decisions_conversation_timeline_idx
     on inbox_decisions(conversation_id, created_at desc, id desc) where conversation_id is not null`,
  // Per-user notification routing: {"<class>":"in_app"|"email"|"both"}, keyed by
  // the classes in lib/notifications.ts. `{}` means "never touched it" — the
  // defaults live in code (NOTIFY_CLASSES.fallback), not in this column, so
  // tuning them later reaches every user who hasn't formed an opinion.
  `alter table users add column if not exists notify_prefs jsonb not null default '{}'::jsonb`,
  // One row per model call that went through `runHarness` (server/harness/run.ts),
  // including the ones that never reached a model — no routable model, or a
  // refusal on a capability the harness declared it cannot work without.
  //
  // This is the PRODUCTION GROUND TRUTH behind model fitness. A bench score
  // says what a model did on ten fixtures; these rows say what it is doing on
  // your org's real work, and the two questions the model picker has to answer
  // are both aggregates over this table:
  //   contract rate  schema_valid, per harness per model, over time
  //   repair rate    repairs > 0 — a model at 40% first-pass and 95% after one
  //                  repair is USABLE; one at 40/45 is not, and nothing in
  //                  Talaria could tell those apart before this table existed.
  // `chain_step` carries which fallback actually won, so a subsystem quietly
  // limping along on 'first-routable' for a month becomes visible instead of
  // being invisible by construction. `model` is nullable because "nothing
  // routed" is a real, and important, outcome to record.
  `create table if not exists harness_runs (
     id uuid primary key default gen_random_uuid(),
     harness text not null,
     model text,
     chain_step text,
     widened boolean not null default false,
     repairs integer not null default 0,
     schema_valid boolean not null default false,
     latency_ms integer not null default 0,
     findings integer not null default 0,
     caller text not null default '',
     created_at timestamptz not null default now()
   )`,
  // Every fitness query is "this harness, this model, recently" — and the
  // matrix runs one per cell, so the ordering has to come from the index
  // rather than from a sort over the harness's whole history.
  `create index if not exists harness_runs_harness_model_idx
     on harness_runs (harness, model, created_at desc)`,
  // The sentence behind a red cell. Contract rate answers "how often does this
  // model hold the contract"; the first question anyone asks of a bad number is
  // "failed how?", and without this the row could not say — a refusal below the
  // capability floor, a reply that never closed its JSON value, and a gateway
  // 503 were all just `schema_valid = false`. The runner redacts and bounds the
  // text before it lands here (see `runError` in harness/run.ts); the model's
  // RAW reply deliberately stays out of the table, since it can be large and
  // this row is kept forever.
  `alter table harness_runs add column if not exists error text`,
  // ── WHY THE NEXT TWO STATEMENTS PREDATE THE BRANCH THAT WROTE THEM ────────
  //
  // These two were written here on `files-surface` and APPLIED to the shared
  // development database before the fitness work appended anything. Migration
  // identity is an ARRAY INDEX, so two branches that both append claim the same
  // ids, and whichever boots second refuses to start: index 232 in its array is
  // not the statement index 232 in the database ran. That is the append-only
  // rule working exactly as designed, across a boundary it cannot see.
  //
  // So the fitness branch copied them forward byte for byte and put its own
  // statements after them, which is where they still sit below. This merge
  // therefore adds nothing here — the two copies were identical and collapsed —
  // and the indices already recorded in every database stay pinned to the
  // statements that actually ran.
  // Agent filing cabinets moved from the root into one "Agents" folder
  // (see agentCategoryFolder). Existing installs already have one root folder
  // per agent, which is exactly the wall of names the move exists to clear, so
  // reparent them rather than leaving old fleets looking different from new.
  //
  // A cabinet is a ROOT folder that is either named after a live agent, or —
  // for agents since retired, whose display name is gone from agent_defs —
  // carries the category folders only agentCategoryFolder ever writes, filed by
  // the same creator, with no loose artifacts of its own (a cabinet holds
  // category folders, never files directly).
  //
  // `created_by` deliberately does NOT discriminate here: it records whoever
  // TRIGGERED the filing, so a cabinet built by a research run carries the
  // requesting human's email, not the agent's name.
  //
  // Two statements, because the root has to exist before anything can point at
  // it. Both are no-ops once no root cabinets remain.
  `insert into artifact_folders (name, created_by)
   select 'Agents', 'system'
   where not exists (select 1 from artifact_folders where parent_id is null and name = 'Agents')
     and exists (
       select 1 from artifact_folders f
       where f.parent_id is null
         and f.name <> 'Agents'
         and (
           exists (select 1 from agent_defs d where d.display_name = f.name)
           or (
             exists (
               select 1 from artifact_folders c
               where c.parent_id = f.id
                 and c.name in ('Documents', 'Media', 'Chat summaries', 'Plans', 'Research')
                 and c.created_by is not distinct from f.created_by
             )
             and not exists (select 1 from artifacts a where a.folder_id = f.id)
           )
         )
     )`,
  `update artifact_folders f
   set parent_id = (select id from artifact_folders where parent_id is null and name = 'Agents' order by created_at asc limit 1)
   where f.parent_id is null
     and f.name <> 'Agents'
     and exists (select 1 from artifact_folders where parent_id is null and name = 'Agents')
     and (
       exists (select 1 from agent_defs d where d.display_name = f.name)
       or (
         exists (
           select 1 from artifact_folders c
           where c.parent_id = f.id
             and c.name in ('Documents', 'Media', 'Chat summaries', 'Plans', 'Research')
             and c.created_by is not distinct from f.created_by
         )
         and not exists (select 1 from artifacts a where a.folder_id = f.id)
       )
     )`,
  // EVERY CASE OF EVERY FITNESS RUN, IN FULL — the audit trail the settings-row
  // archive could never be.
  //
  // WHY IT IS NOT IN `app_settings` WITH THE REST OF THE REPORT. The archived
  // record keeps transcripts only for cases that FAILED something, capped at
  // thirty, because it is one JSON row read whole on every page load. That is
  // the right shape for a drill-down and the wrong shape for verification: the
  // question "did this model actually do the work, or did our fixture just
  // accept something weak" can only be answered from a PASSING transcript, and
  // those were exactly the ones discarded. Several fixtures rewritten this
  // month were rewritten because a passing transcript turned out to show the
  // model being failed for obeying us, or passing for the wrong reason — and
  // each time the evidence had to be re-bought by re-running the sweep.
  //
  // A table can hold them: one row per case, written as it lands, pruned by
  // run rather than by size.
  `create table if not exists fitness_transcripts (
     id uuid primary key default gen_random_uuid(),
     model text not null,
     run_started_at timestamptz not null,
     harness text not null,
     case_name text not null,
     band text not null default 'standard',
     verdict text not null default 'pass',
     prompt text,
     raw text,
     turns jsonb,
     tool_calls jsonb,
     upstream jsonb,
     latency_ms integer not null default 0,
     prompt_tokens integer not null default 0,
     completion_tokens integer not null default 0,
     created_at timestamptz not null default now()
   )`,
  // Every read is "this model, this run" or "this model, latest run", and the
  // prune is "this model, older than N runs". One index serves all three.
  `create index if not exists fitness_transcripts_model_run_idx
     on fitness_transcripts (model, run_started_at desc, harness, case_name)`,
  // WHEN THE CASE STARTED, AND WHAT IT COST THE SWEEP.
  //
  // `latency_ms` is the runner's own measure of the FINAL attempt — render, the
  // model turns, the repair round-trip, the guard pass — and it is the number
  // the observed-vs-tested comparison is computed from, so it has to stay
  // exactly what production records. It therefore cannot answer either question
  // a speed comparison actually asks:
  //
  //   WHAT DID THIS CASE COST? A case whose first two requests vanished and
  //   whose third took four seconds has `latency_ms = 4000` and spent two
  //   minutes of the sweep. `wall_ms` is that two minutes.
  //
  //   WHAT WAS RUNNING ALONGSIDE IT? Under concurrency a latency figure cannot
  //   distinguish a slow model from four fast cases queued behind each other.
  //   `started_at` makes the run a timeline instead of a bag of durations.
  `alter table fitness_transcripts
     add column if not exists started_at timestamptz,
     add column if not exists wall_ms integer not null default 0`,
  // Folders became shareable. They were org-wide containers with no access of
  // their own, which made "share this folder with the team" — the commonest
  // thing a non-technical person does in a file browser — impossible to express.
  // Same three columns, same vocabulary, same kb_editors grants as docs, spaces
  // and artifacts, so folder access means exactly what it already means
  // everywhere else.
  //
  // Defaults are deliberately 'org': every folder that exists today is visible
  // to the whole workspace, and a migration must not make anyone's folders
  // vanish from anyone else's browser.
  `alter table artifact_folders add column if not exists visibility text not null default 'org'`,
  `alter table artifact_folders add column if not exists edit_policy text not null default 'org'`,
  `alter table artifact_folders add column if not exists owner_user_id uuid references users(id) on delete set null`,
  `create index if not exists artifact_folders_owner_idx on artifact_folders(owner_user_id)`,
  // Give the existing folders an owner, resolved from the `created_by` string
  // they already carry. Without this, every pre-existing human folder is
  // ownerless and only an admin could ever re-share it. Agent-created folders
  // (the Agents/ cabinets) match no user and stay ownerless on purpose — those
  // belong to the workspace, and canGovern already knows how to handle them.
  `update artifact_folders f
   set owner_user_id = (
     select u.id from users u
     where u.email = f.created_by or u.name = f.created_by
     order by u.id limit 1
   )
   where f.owner_user_id is null
     and f.created_by is not null
     and exists (select 1 from users u where u.email = f.created_by or u.name = f.created_by)`,
  // WORKSPACE SECRETS — the credentials an agent may USE without ever reading.
  //
  // A DOC, NOT A ROW, because that is how they actually arrive. A deploy needs a
  // PAT, a registry password and a signing key together; making somebody create
  // three unrelated secrets and remember which three go together is how the
  // wrong one gets used. One doc holds one or more entries, and a single secret
  // is simply a doc with one — so there is one shape to grant, audit and revoke
  // rather than two.
  //
  // `kind` separates the two lifetimes. A 'vault' doc is durable and belongs to
  // the workspace. A 'relay' is a ONE-SHOT: somebody pastes a credential into
  // chat so an agent can do one thing with it, and it is consumed on first
  // resolve and never persisted anywhere a model can reach.
  `create table if not exists workspace_secrets (
     id uuid primary key default gen_random_uuid(),
     name text unique not null,
     title text not null,
     kind text not null default 'vault',
     note text,
     created_by text,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now(),
     expires_at timestamptz,
     uses_remaining integer,
     last_used_at timestamptz
   )`,
  // The entries. `value_cipher` is sealed with the same envelope
  // `llm_endpoints.api_key_cipher` uses (secretbox.ts), so a database dump is not
  // a credential dump and one key rotation covers both. Nothing else in the row
  // is sensitive: the KEY is what a model sees.
  `create table if not exists workspace_secret_entries (
     secret_id uuid not null references workspace_secrets(id) on delete cascade,
     key text not null,
     label text not null,
     value_cipher text not null,
     primary key (secret_id, key)
   )`,
  // WHO MAY RESOLVE IT. No grant means nobody — a secret somebody creates and
  // forgets is inert, which is the safe direction for this table to fail in.
  `create table if not exists workspace_secret_grants (
     secret_id uuid not null references workspace_secrets(id) on delete cascade,
     agent_model text not null,
     granted_by text,
     granted_at timestamptz not null default now(),
     primary key (secret_id, agent_model)
   )`,
  // WHERE A CREDENTIAL MAY BE SPENT. Substitution was blind to destination: a
  // handle resolved wherever it appeared, so an agent talked into pushing to
  // somebody else's mirror handed over a live token and every layer downstream
  // saw an ordinary tool call. The model was the only boundary.
  //
  // NULL/EMPTY MEANS UNRESTRICTED, which is what every secret created before
  // this has — the check is opt-in, so nothing that worked yesterday stops. A
  // non-empty list is an operator saying "this credential is for these hosts",
  // and `resolveHandles` then refuses anything else AND anything whose
  // destination it cannot read at all.
  `alter table workspace_secrets add column if not exists allowed_hosts text[]`,
  // ── WORKING SECRETS: the ones a PERSON needs back ─────────────────────────
  //
  // Everything above this line is a credential an agent SPENDS and nobody ever
  // reads — no reveal verb, not for an admin, not once. That property is load-
  // bearing and these columns do not weaken it: `revealable` defaults FALSE, so
  // every credential that already exists, and every one the admin panel creates,
  // keeps exactly the guarantee it had.
  //
  // What is new is a different noun sharing the same store. Somebody building a
  // feature has a staging key their two teammates also need, and today the
  // options are Slack, a sticky note, or a .env passed around — all of which are
  // worse than a place that seals it, shares it deliberately and writes down
  // every look. `revealable = true` marks those, and ONLY those, as readable by
  // the people named in `workspace_secret_readers`.
  //
  // IT IS NOT AN ARTIFACT ROW, deliberately, though it wears one's clothes in
  // the Files browser. An artifact body is indexed for retrieval, exported to
  // Google, downloadable, and reachable unauthenticated at /api/artifacts/
  // public/$slug. A credential in that pipeline is a credential on the open
  // internet one visibility click later. So the value stays here, sealed, with
  // no content path — and only the PLACEMENT is artifact-shaped.
  `alter table workspace_secrets add column if not exists revealable boolean not null default false`,
  `alter table workspace_secrets add column if not exists owner_user_id uuid references users(id) on delete set null`,
  `alter table workspace_secrets add column if not exists folder_id uuid references artifact_folders(id) on delete set null`,
  // WHO MAY LOOK. Distinct from `workspace_secret_grants`, which is who may
  // SPEND — two verbs, two audiences, and conflating them is how an agent ends
  // up holding a value it only ever needed to use. A human here can reveal; an
  // agent there can spend and can never read.
  `create table if not exists workspace_secret_readers (
     secret_id uuid not null references workspace_secrets(id) on delete cascade,
     user_id uuid not null references users(id) on delete cascade,
     granted_by text,
     granted_at timestamptz not null default now(),
     primary key (secret_id, user_id)
   )`,
  `create index if not exists workspace_secrets_folder_idx on workspace_secrets(folder_id)`,
]

// One row per APPLIED statement, keyed by its index in MIGRATIONS. The checksum
// is what makes the append-only rule enforceable rather than aspirational.
const SCHEMA_MIGRATIONS = `create table if not exists schema_migrations (
   id integer primary key,
   checksum text not null,
   applied_at timestamptz not null default now()
 )`

// Fixed advisory-lock key: two instances booting at once queue instead of
// racing. `create table if not exists` is not atomic against a concurrent
// create, and initSecretbox's select-then-insert would double-insert v1.
const MIGRATION_LOCK = 8_314_207

// Whitespace-insensitive, so reindenting a statement is not a schema change.
function checksum(stmt: string): string {
  return createHash('sha256').update(stmt.replace(/\s+/g, ' ').trim()).digest('hex')
}

// The migration run gets a POOL OF ITS OWN, and it is not an optimisation.
//
// Advisory locks are session-scoped, so the migration run needs one connection
// it can hold for the whole pass. The obvious way to get one — `sql.reserve()`
// on the application pool — is a process-killing trap, and this is the shape of
// it (postgres.js 3.4, src/index.js):
//
//   reserve() registers its pending request TWICE. It pushes `{reserve}` onto
//   the pool's shared `queries` list AND, if a connection is currently closed,
//   hands the same object to `connect()`. Only one of those two paths ever
//   settles the promise: `onopen()` resolves it by shifting it back off
//   `queries` (connection.js deliberately does NOT execute a `reserve` request
//   handed to it as `initial` — see `initial && !initial.reserve && execute`).
//   So if anything drains `queries` in between, the request is dispatched to a
//   second connection, and when both connections open they each find `queries`
//   empty and quietly return. Nothing rejects. Nothing times out. `reserve()`
//   never settles, for the life of the process.
//
//   The thing that drains `queries` is `onclose()`: `queries.length &&
//   connect(c, queries.shift())`. A connection reaching `onclose` at that
//   moment is not exotic — it is what `idle_timeout` (and `max_lifetime`, which
//   postgres.js enables BY DEFAULT at a random 30-60 minutes) does to every
//   idle pooled connection, on a timer.
//
// Which made this the M1 cold-boot wedge, deterministically rather than as a
// rare race. On an instance serving ZERO requests the ordering is fixed:
// server-entry's boot probe runs `getSql()\`select 1\`` (healthz, which
// deliberately does not migrate), leaving one connection idle with a 20s
// `idle_timeout` armed; the probe returning is also what arms every job's
// first-run timer; and the first job due — `notification-mail`, at
// firstRunDelayMs 20_000 — was therefore the process's first `db()` call, i.e.
// the first `reserve()`, landing in the same millisecond as the reap of the
// connection the probe left behind. Then `ensureMigrated()` caches that
// never-settling promise on globalThis, so EVERY later `db()` — every route,
// every other job — awaits it forever. Not a mail outage: a dead process that
// still answers static files and 200s on /api/healthz.
//
// The fix is not to move the 20s. Two timers that no longer collide are one
// dependency bump from colliding again, and `max_lifetime` would have found the
// same window hours later with no constant to blame. The fix is that the
// application pool is never `reserve()`d at all:
//
//   · `max: 1` — the pool IS the one session, so `pg_advisory_lock` and the
//     statements it guards run on the same connection without reserving.
//   · `idle_timeout: 0`, `max_lifetime: 0` — nothing reaps a connection under
//     this pool, so `onclose` never races anything. It lives for one run.
//   · ended in `finally` — the session (and with it the advisory lock) goes
//     away even if a statement throws.
//
// Cost: one extra connection, once, for as long as migrating takes.
function migrationSql(): Sql {
  return postgres(databaseUrl(), { max: 1, idle_timeout: 0, max_lifetime: 0, onnotice: () => {} })
}

async function runMigrations(): Promise<void> {
  const sql = migrationSql()
  try {
    await sql`select pg_advisory_lock(${MIGRATION_LOCK})`
    await sql.unsafe(SCHEMA_MIGRATIONS)
    const rows = (await sql`select id, checksum from schema_migrations`) as unknown as Array<{ id: number; checksum: string }>
    const applied = new Map(rows.map((r) => [r.id, r.checksum]))
    for (const [i, stmt] of MIGRATIONS.entries()) {
      const sum = checksum(stmt)
      const prev = applied.get(i)
      if (prev === sum) continue
      // A statement that changed under an id we already applied means the array
      // was edited in place or something was inserted mid-array — every index
      // after it now describes a different statement than the one this database
      // ran. Refuse to boot rather than apply the rest against a schema the
      // array no longer describes.
      if (prev)
        throw new Error(
          `[pg] migration ${i} changed after it was applied — MIGRATIONS is append-only: add new statements at the END of the array`,
        )
      // Its own transaction: a failure part-way leaves every earlier statement
      // applied AND recorded, so the next boot resumes here instead of replaying.
      await sql.begin(async (tx) => {
        await tx.unsafe(stmt)
        await tx`insert into schema_migrations (id, checksum) values (${i}, ${sum})`
      })
    }
    // Load/create the data key so seal()/open() are synchronous thereafter.
    await initSecretbox(sql)
  } finally {
    // Ending the pool ends the session, which releases the advisory lock; the
    // explicit unlock is so a slow close does not hold up another instance.
    await sql`select pg_advisory_unlock(${MIGRATION_LOCK})`.catch(() => {})
    await sql.end({ timeout: 5 }).catch(() => {})
  }
}

function ensureMigrated(): Promise<void> {
  if (!g.__talariaMigrated) {
    // Drop the cached promise if the run fails, so the next db() retries: a
    // cached rejection poisons every later call for the life of the process.
    const run: Promise<void> = runMigrations().catch((e) => {
      if (g.__talariaMigrated === run) g.__talariaMigrated = undefined
      throw e
    })
    g.__talariaMigrated = run
  }
  return g.__talariaMigrated
}

/** Migrated Postgres handle. `const sql = await db()`. */
export async function db(): Promise<Sql> {
  await ensureMigrated()
  return getSql()
}
