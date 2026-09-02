// Postgres — durable state (users, roles, per-agent access, conversations,
// messages). postgres.js (no native build). Migrations run once per statement
// ever (see schema_migrations below), under an advisory lock — eagerly at boot
// (server-entry.js calls migrate()) and lazily on the first query thereafter.
// Cached on globalThis so HMR doesn't open a new pool each reload.

import { createHash } from 'node:crypto'
import postgres from 'postgres'
import { initSecretbox } from '../secretbox'

type Sql = ReturnType<typeof postgres>
const g = globalThis as unknown as {
  __talariaSql?: Sql
  __talariaMigrated?: Promise<MigrationResult>
  /** MIGRATIONS.length as of the last successful run — see `ensureMigrated`
   *  for why the count rides globalThis beside the promise. */
  __talariaMigrationCount?: number
}

function databaseUrl(): string {
  const url = process.env.DATABASE_URL
  if (!url) throw new Error('DATABASE_URL is not set')
  return url
}

/** The APPLICATION pool. Every request handler and every scheduled job runs on
 *  this one. `max` is env-tunable (TALARIA_UI_PG_POOL_MAX, default 20) so the
 *  UI's ceiling moves with the api's — the two pools share one postgres, and
 *  the sizing law lives in docker/compose.yml. `idle_timeout` lets the driver
 *  reap connections a quiet instance is not using — see runMigrations for why
 *  nothing may `reserve()` it. */
export function getSql(): Sql {
  if (!g.__talariaSql) {
    const max = Number.parseInt(process.env.TALARIA_UI_PG_POOL_MAX ?? '20', 10)
    g.__talariaSql = postgres(databaseUrl(), {
      max: Number.isFinite(max) && max > 0 ? max : 20,
      idle_timeout: 20,
      onnotice: () => {},
    })
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
  // the attention fingerprint actually changes. (Superseded — see the drop at
  // the end of this array — when the per-tab briefing panel was removed in
  // favour of the daily brief. Kept verbatim here because this array is
  // append-only: a statement's index is its identity on every database that
  // already ran it.)
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
  // ── SECRET FOLDERS: organisation that belongs to the Secrets view ─────────
  //
  // THE FIRST ATTEMPT FILED SECRETS INTO ARTIFACT FOLDERS, on the theory that
  // one filing system beats two. It is the wrong theory here. A folder in Files
  // is a place for DOCUMENTS — it shows up in the file browser, it carries
  // artifact sharing, and a secret filed into it was invisible from the folder
  // it claimed to be in. What people actually want is to tidy their credentials
  // where their credentials live, and to hand a teammate the whole "Checkout
  // rewrite" set in one gesture rather than six.
  //
  // So these are their own folders, in their own namespace, and `folder_id`
  // above goes away unused.
  `alter table workspace_secrets drop column if exists folder_id`,
  `create table if not exists secret_folders (
     id uuid primary key default gen_random_uuid(),
     name text not null,
     owner_user_id uuid references users(id) on delete cascade,
     created_at timestamptz not null default now()
   )`,
  `alter table workspace_secrets add column if not exists secret_folder_id uuid references secret_folders(id) on delete set null`,
  `create index if not exists workspace_secrets_secret_folder_idx on workspace_secrets(secret_folder_id)`,
  // SHARING A FOLDER SHARES WHAT IS IN IT, now and later. That "and later" is
  // the point: a set somebody is actively working on gains a credential next
  // week, and re-sharing it to the same four people is the step everybody
  // forgets. Access is therefore resolved at READ time as the union of a
  // secret's own grants and its folder's — never copied down onto rows, which
  // would freeze the membership at the moment of sharing.
  `create table if not exists secret_folder_readers (
     folder_id uuid not null references secret_folders(id) on delete cascade,
     user_id uuid not null references users(id) on delete cascade,
     granted_by text,
     granted_at timestamptz not null default now(),
     primary key (folder_id, user_id)
   )`,
  `create table if not exists secret_folder_grants (
     folder_id uuid not null references secret_folders(id) on delete cascade,
     agent_model text not null,
     granted_by text,
     granted_at timestamptz not null default now(),
     primary key (folder_id, agent_model)
   )`,
  // ── RESEARCH BECOMES A CONVERSATION ───────────────────────────────────────
  //
  // A research run was a one-shot: ask a question, get a cited report, and the
  // only thing anyone could do with it afterwards was read it. No way to say
  // "dig into the second point", "this source is stale", or to have that
  // discussion with the two colleagues the run was shared with. A view whose
  // whole content is a document does not need to be a view.
  //
  // So a run gets a conversation, exactly the way a plan has one — the surface
  // that already solves this: everyone talks to the same agent beside the same
  // living document. `kind = 'research'` joins 'chat' and 'plan'.
  //
  // MEMBERSHIP IS NOT DUPLICATED. `research_members` already says who a run is
  // shared with, and conversation access derives from it rather than from a
  // second list in `conversation_members`. Two spellings of "who can see this"
  // is how a person keeps access to the talk after losing it to the report.
  `alter table research_runs add column if not exists conversation_id uuid references conversations(id) on delete set null`,
  `create index if not exists research_runs_conversation_idx on research_runs(conversation_id)`,
  // A FOLLOW-UP EXTENDS THE REPORT IT CAME FROM, rather than minting a second
  // document about the same subject. "Dig into the second point" produced an
  // unrelated run with its own report and its own source numbering, so the
  // answer to one question lived in two places that did not reference each
  // other — and the reader had to assemble it.
  //
  // The child row still exists, because provenance is worth keeping: who asked
  // what, when, and how much it cost. What it does NOT get is its own artifact.
  `alter table research_runs add column if not exists parent_run_id uuid references research_runs(id) on delete set null`,
  `create index if not exists research_runs_parent_idx on research_runs(parent_run_id)`,
  // ── AGENT ROLE TEMPLATES ──────────────────────────────────────────────────
  // The starting point for a new agent, expressed as a BUSINESS ROLE rather
  // than a person. Talaria ships and maintains a common set in code
  // (api/src/agent_role_templates.rs); this table is the org's own additions,
  // which is the half that cannot live in the repo. `slug` is unique so an org
  // template can deliberately shadow a built-in of the same name — the org's
  // version of "Support Agent" should win over ours.
  `create table if not exists agent_role_templates (
     id uuid primary key default gen_random_uuid(),
     slug text not null unique,
     name text not null,
     role text not null,
     department text not null,
     description text not null default '',
     soul text not null,
     created_by text,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  // ── APPENDED, because migration identity is an ARRAY INDEX ───────────────
  // Everything above has already run in real databases at the indices it
  // holds. New statements take the next free ones; splicing renumbers every
  // statement after the splice and every existing install refuses to boot on
  // the checksum mismatch, which is the guard working rather than a bug.
  // ── Durable runs (server/runs/) ────────────────────────────────────────────
  // ONE row per long action, and the noun the codebase has been missing. A run
  // is a SEQUENCE OF STEPS OVER A CHECKPOINT: resuming means re-entering the
  // definition's `step()` with the last PERSISTED checkpoint, which is what
  // makes it survive a tab close, a view change, a restart and a deploy.
  //
  // What each of those cost before this table existed (engines now in Rust —
  // the costs are why the table is shaped like this): a restart mid-research
  // marked the user's run FAILED; the fitness surface was a status blob in
  // app_settings driven by a bare `void fn()`, so a deploy left state
  // 'running' forever with nothing driving it; retrieval migrations had the
  // same fire-and-forget shape; work dispatch tracked liveness in a
  // process-local Set with an explicit TODO(multi-instance).
  //
  // `state` is not a check constraint on purpose: the six values are declared
  // in the runs driver (api/src/runs/define.rs, `RunState`), and a constraint
  // here would make every new state a migration on a hot table for no
  // protection the driver does not already give (every write is a
  // compare-and-set on state + lease_owner).
  //
  // `awaiting` is the state worth naming: a run PARKED on a human decision.
  // `decision` carries the question and, later, the answer, because a question
  // that lives only in the process that raised it is gone the moment you open
  // the approval on another device. `approval_key` is the dedupe handle the
  // Rust approvals engine's announce and nag machinery keys on
  // (api/src/approvals.rs).
  //
  // `lease_owner` / `lease_expires_at` mirror the Redis lease that actually
  // enforces mutual exclusion. Both, deliberately: Redis is what another
  // instance TESTS before taking a run, and these columns are what the reclaim
  // query SCANS — Redis has no index over "every run whose lease expired", and
  // keeping one would mean a second copy of every in-flight run id.
  `create table if not exists runs (
     id uuid primary key default gen_random_uuid(),
     kind text not null,
     owner_user_id uuid references users(id) on delete cascade,
     subject_type text,
     subject_id text,
     state text not null default 'queued',
     phase text not null default '',
     checkpoint jsonb,
     input jsonb not null default '{}'::jsonb,
     result jsonb,
     error text,
     attempt integer not null default 0,
     lease_owner text,
     lease_expires_at timestamptz,
     approval_key text,
     decision jsonb,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now(),
     started_at timestamptz,
     finished_at timestamptz
   )`,
  // THE RECLAIM QUERY: "every run nobody is driving any more". Partial, because
  // the interesting rows are the live ones and a finished run is dead weight in
  // an index that a sweep hits every few seconds forever — this stays the size
  // of the in-flight set, not of the history.
  `create index if not exists runs_reclaim_idx
     on runs (lease_expires_at asc nulls first, created_at asc)
     where state in ('queued', 'running')`,
  // THE OTHER REAL QUERY: "this user's active runs", newest activity first, for
  // the strip and the list. `awaiting` is in the partial predicate because a
  // run parked on a question the user has to answer is the most active thing
  // they have, and leaving it out would hide exactly the row that needs them.
  `create index if not exists runs_owner_active_idx
     on runs (owner_user_id, state, updated_at desc)
     where state in ('queued', 'running', 'awaiting')`,
  // Approvals joins back the other way — one pending decision to the run it
  // parked. Unique so a second row cannot claim the same key: the key is
  // derived from (kind, run id, question key), so a duplicate would mean two
  // runs announcing as one approval and one of them never being decided.
  `create unique index if not exists runs_approval_key_idx
     on runs (approval_key) where approval_key is not null`,

  // ── THE DAILY BRIEF: one document per person per day, WRITTEN ONCE AND ONLY
  //    EVER APPENDED TO ─────────────────────────────────────────────────────
  //
  // The brief opens two hours before the workday (the Rust brief engine,
  // api/src/daily_brief/) and
  // then follows the day: a ticket moves, an approval lands, a DM arrives, and
  // the brief LEARNS about it. What it must never do is rewrite itself. A
  // person who read their brief at 08:00 and comes back at 14:00 has to be
  // able to find the thing they read — and to see what changed under it —
  // which a regenerated document cannot offer at any level of prompt care,
  // because the earlier text no longer exists.
  //
  // So immutability is SCHEMA, not discipline. `daily_brief_entries` rows are
  // insert-only: nothing in the brief engine (api/src/daily_brief/) issues an
  // UPDATE against
  // them, and the one mutable column in the whole feature is the parent row's
  // `read_seq` (how far the reader got), which is about the reader and not
  // about the content. An item that resolves does not get edited or deleted;
  // a NEW row is appended with `supersedes` pointing at the row it retires,
  // and the view is a fold over the log in `seq` order.
  //
  // That is also what makes the surface honest under failure. A sweep that
  // half-completes leaves a shorter log, never a corrupted document, and the
  // next sweep appends what it missed — because the diff is computed against
  // the FOLD, not against a snapshot the sweep is trusted to have written.
  `create table if not exists daily_briefs (
     id uuid primary key default gen_random_uuid(),
     user_id uuid not null references users(id) on delete cascade,
     -- The LOCAL calendar date, resolved in 'zone' at open time. A date rather
     -- than a timestamp because "which brief" is a question about the reader's
     -- day, and the unique constraint below is what makes a double-firing
     -- scheduler tick a no-op instead of a second brief.
     brief_date date not null,
     zone text not null default 'UTC',
     -- The assistant that wrote it, captured at open time. Recorded rather
     -- than joined because the owner may re-point or rename their assistant
     -- later, and a brief is a document with an author, not a live view.
     agent_model text,
     agent_name text,
     -- The mirrored artifact (share, export, public link). DERIVED from the
     -- log and rewritten on every append, so it can be deleted or fail to
     -- write without the brief losing anything.
     artifact_id uuid references artifacts(id) on delete set null,
     -- The append cursor. Held here so a concurrent sweep cannot mint two
     -- entries at the same seq (the unique index below is the enforcement).
     last_seq integer not null default 0,
     -- How far the reader has read, and the ONLY column here that a normal day
     -- updates. New-since-you-looked is a property of the reader.
     read_seq integer not null default 0,
     last_swept_at timestamptz,
     created_at timestamptz not null default now(),
     unique (user_id, brief_date)
   )`,
  `create index if not exists daily_briefs_user_idx on daily_briefs(user_id, brief_date desc)`,
  `create table if not exists daily_brief_entries (
     id uuid primary key default gen_random_uuid(),
     brief_id uuid not null references daily_briefs(id) on delete cascade,
     seq integer not null,
     -- 'lede'     the assistant's opening read on the day (seq 1)
     -- 'item'     something that needs the owner, as it first appeared
     -- 'change'   the same source, materially different — supersedes the last
     -- 'resolved' the source stopped needing them — supersedes the last
     -- 'note'     the assistant narrating a batch of changes
     kind text not null,
     section text not null default 'action',
     -- inbox-focus's keyOf(sourceType, sourceId). The identity the fold
     -- groups on and the diff compares against; null for narrative entries.
     source_key text,
     source_type text,
     source_id text,
     source_href text,
     -- inbox-focus's sourceFingerprint. The whole change detector: same key
     -- + same fingerprint means nothing happened and nothing is appended.
     fingerprint text,
     supersedes uuid references daily_brief_entries(id) on delete set null,
     priority text,
     status_label text,
     badge jsonb,
     title text not null default '',
     body text not null default '',
     evidence jsonb not null default '[]'::jsonb,
     created_at timestamptz not null default now()
   )`,
  // The fold's only query, and the guard that makes `last_seq` a real cursor
  // rather than a hint: two sweeps racing to append cannot both win a seq.
  `create unique index if not exists daily_brief_entries_seq_idx on daily_brief_entries(brief_id, seq)`,
  `create index if not exists daily_brief_entries_key_idx on daily_brief_entries(brief_id, source_key)`,
  // WHICH APPEND WROTE THIS ROW. The timeline groups entries into "at 11:04,
  // three things moved", and the first version of that grouping derived the
  // batch by truncating `created_at` to the second — which is correct exactly
  // as long as two appends never land in the same second. They can: a realtime
  // nudge and a scheduler tick reach `sweepBrief` together, and the whole point
  // of the timeline is that it is an honest record of when things were learned.
  // A batch is a fact about the write, so it is stored rather than inferred.
  `alter table daily_brief_entries add column if not exists batch uuid`,
  `create index if not exists daily_brief_entries_batch_idx on daily_brief_entries(brief_id, batch)`,

  // ── DELEGATED REPLIES: the assistant answering a chat for its owner ───────
  //
  // READING A MESSAGE IS NOT ANSWERING IT, which is the whole reason this
  // exists. The brief's first version resolved a conversation line when the
  // unread count reached zero — so glancing at Priya's question told the
  // document it had been handled, and the one thing the surface is for ("who is
  // still waiting on me") was the thing it got wrong. A conversation is open
  // until somebody REPLIES, and that is derived from the message log rather
  // than from a read cursor.
  //
  // Once a line can stay open on "read, not answered", the obvious next thing
  // is for the assistant to be allowed to close it. That is a delegation from
  // the OWNER to their own assistant — not an admin-granted `Perm`, which
  // describes what a person may do — so it lives here, scoped to one person's
  // conversations and revocable per thread.
  //
  // A GRANT WITH NO `channel_id` IS THE STANDING ONE (every DM); a row with one
  // covers that thread alone. `revoked_at` rather than a delete: who was allowed
  // to speak for someone, and when that stopped, is exactly the history you want
  // when a reply turns out to have been wrong.
  `create table if not exists assistant_reply_grants (
     id uuid primary key default gen_random_uuid(),
     user_id uuid not null references users(id) on delete cascade,
     channel_id uuid references channels(id) on delete cascade,
     granted_at timestamptz not null default now(),
     revoked_at timestamptz
   )`,
  // Two partial indexes, not one constraint, because a NULL `channel_id` is the
  // standing grant and Postgres treats NULLs as distinct in a unique index —
  // so the standing grant needs its own predicate or a person could accumulate
  // five of them and revoking one would look like it did nothing.
  `create unique index if not exists assistant_reply_grants_thread_idx
     on assistant_reply_grants(user_id, channel_id) where channel_id is not null and revoked_at is null`,
  `create unique index if not exists assistant_reply_grants_standing_idx
     on assistant_reply_grants(user_id) where channel_id is null and revoked_at is null`,

  // A reply the assistant WROTE but has not been allowed to send.
  //
  // `in_reply_to_seq` is what makes a draft honest over time: it names the
  // message the reply answers, so a draft written against "can I start outreach?"
  // is visibly stale once the person has sent two more messages. Approving a
  // stale draft would post an answer to a question that has moved on, and
  // without this column there is no way to know that happened.
  `create table if not exists assistant_reply_drafts (
     id uuid primary key default gen_random_uuid(),
     user_id uuid not null references users(id) on delete cascade,
     channel_id uuid not null references channels(id) on delete cascade,
     in_reply_to_seq integer not null,
     agent_model text,
     content text not null,
     -- pending | sent | rejected. A draft overtaken by a new message is not a
     -- fourth status: staleness is DERIVED by comparing 'in_reply_to_seq' to the
     -- channel, so it cannot drift out of date the way a stored flag would.
     status text not null default 'pending',
     -- Set when the assistant sent it under a standing grant rather than an
     -- approval, so "who let this happen" is answerable from the row itself.
     delegated boolean not null default false,
     message_id uuid references channel_messages(id) on delete set null,
     created_at timestamptz not null default now(),
     decided_at timestamptz,
     decided_by uuid references users(id) on delete set null
   )`,
  `create index if not exists assistant_reply_drafts_open_idx
     on assistant_reply_drafts(user_id, channel_id) where status = 'pending'`,

  // ── TALKING TO YOUR ASSISTANT ABOUT A LINE, AND KEEPING IT ────────────────
  //
  // This started ephemeral on purpose — no row, nothing indexed, on the theory
  // that a person asks loose half-formed questions about their own day and
  // would stop if those were minuted. That theory was wrong about the thing
  // people actually do: they ask, get an answer, click into the ticket, come
  // back — and an ephemeral thread is gone by the time they return, so the
  // conversation cannot survive the ONE navigation it exists to prompt.
  //
  // So it persists, scoped to the brief and the line it is about. `source_key`
  // null is the conversation about the whole day; a key ties it to one line, so
  // a question about the ledger ticket is still there when you come back from
  // the ledger ticket.
  //
  // PER BRIEF, NOT PER LINE FOREVER. `brief_id` is in the key, so tomorrow's
  // brief starts its conversations clean even where the same ticket appears
  // again. A brief is a document about one day and its margin notes belong to
  // that day; carrying them forward would mean today's page opening with an
  // argument from Tuesday.
  //
  // CONSEQUENCE, STATED HERE BECAUSE IT IS EASY TO MISS: the reply harness
  // (`briefer:daily-chat`) declared no `redact` on the explicit grounds that
  // nothing was saved. That is no longer true, and it now redacts — a
  // credential quoted out of a ticket title would otherwise sit in this table
  // for the life of the brief. (Later: the table AND the harness are gone —
  // see the drop at the end of this array — but the reasoning is kept because
  // it is the argument for any saved chat redacting.)
  `create table if not exists brief_chat_messages (
     id uuid primary key default gen_random_uuid(),
     brief_id uuid not null references daily_briefs(id) on delete cascade,
     user_id uuid not null references users(id) on delete cascade,
     -- Null = the conversation about the day as a whole.
     source_key text,
     seq integer not null,
     role text not null,
     content text not null default '',
     created_at timestamptz not null default now()
   )`,
  `create unique index if not exists brief_chat_seq_idx
     on brief_chat_messages(brief_id, coalesce(source_key, ''), seq)`,
  `create index if not exists brief_chat_thread_idx
     on brief_chat_messages(brief_id, source_key, seq)`,
  // Preferred reasoning effort — the user's platform default, applied wherever
  // a model they use publishes the level (a level string like 'high', or null
  // for "always the model's own default"). Inert by construction on models
  // without a published ladder: surfaces only offer what the metadata vouches
  // for, so a stale level never reaches a request.
  //
  // APPENDED, NOT FILED NEXT TO preferred_model: a statement's index is its
  // identity in schema_migrations, so anything but the end of the array trips
  // the checksum check on every database that already ran past this point —
  // which is exactly the boot-refusal this line caused the first time it was
  // written.
  `alter table users add column if not exists preferred_effort text`,

  // The per-tab briefing panel is gone — the daily brief is the one summary a
  // person is given — so the briefings cache it wrote has no writer and no
  // reader. Dropped, APPEND-ONLY as ever: the create above stays where it has
  // always been and this runs after it on every database.
  `drop table if exists briefings`,

  // Same for the brief's own chat thread. Asking about a line happens from the
  // sidebar assistant panel now, which carries its own (already-persisted)
  // conversation — a second per-line thread beside it was a second chat to
  // find, and the panel is the one people already have open.
  `drop table if exists brief_chat_messages`,

  // The assistant panel's conversation is SEGMENTED now — many instances per
  // owner, picked from a dropdown — so the one-live-inbox-conversation-per-user
  // guarantee this index enforced is gone. Instances are created explicitly
  // and archived, never deduplicated. (The create above stays, append-only as
  // ever; existing installs keep their single row as their first instance.)
  `drop index if exists conversations_inbox_user_idx`,

  // ── PLAN DRAFTS: a ticket-draft JOB's durable half ────────────────────────
  //
  // The run row (same uuid, kind 'plan-draft') owns the STATE MACHINE —
  // queued/running/done/error, the lease, the reclaim sweep. This table owns
  // the DOMAIN: which conversation, which board the tickets were drafted for,
  // and the proposals themselves as jsonb — which is why a draft survives a
  // closed browser, a reloaded tab and a restarted server: the client asks
  // "is there a draft for this conversation?" and the answer is a row, not a
  // memory. `proposals` is written once by the run (the agent's output) and
  // then by PATCHes from the review walk, so edits survive a reload too.
  //
  // APPENDED, NOT FILED NEXT TO the runs tables it belongs beside — the first
  // version of this change did file it there, and every database that had
  // already migrated past the runs block refused to boot on the checksum
  // mismatch, which is the guard doing its job. Index identity beats topical
  // order; the comment is the map.
  `create table if not exists plan_drafts (
     id uuid primary key default gen_random_uuid(),
     conversation_id uuid not null references conversations(id) on delete cascade,
     created_by uuid references users(id) on delete set null,
     source text not null,
     agent_model text not null,
     routed_model text,
     tier text,
     board_id uuid,
     template_id uuid,
     proposals jsonb not null default '[]'::jsonb,
     note text,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now()
   )`,
  // THE one real query: "the latest draft for this conversation" — the
  // review's way back after a reload. Partial over the live states would not
  // help it (it wants finished drafts most of all), so a plain covering index
  // on the conversation's timeline.
  `create index if not exists plan_drafts_conversation_idx
     on plan_drafts (conversation_id, created_at desc)`,

  // ── ADMIN-DECLARED EFFORT LADDERS ──────────────────────────────────────────
  //
  // The effort feature's contract is "offered only where the model metadata
  // vouches for it", and the only vouching voice was the provider's catalog.
  // That left every minimal OpenAI-compatible self-host (vLLM, Ollama, a
  // hand-rolled gateway — GET /models answering `{id}` and nothing else) with
  // no picker forever, no matter what the weights behind it accept. This
  // column is the second voice: an ADMIN declaring "this endpoint's build of
  // <model> takes these levels" — the same standing a declared capability
  // fact has over a catalog one (a human outranks a provider's silence), with
  // the same honesty rule: the declaration REPLACES the catalog's ladder for
  // that endpoint's model, it never merges with it. Keyed by upstream model
  // id, exactly like model_prices beside it.
  `alter table llm_endpoints add column if not exists model_efforts jsonb not null default '{}'::jsonb`,

  // ── PER-USER TIME ZONE ─────────────────────────────────────────────────────
  //
  // The zone a person's brief opens in and their digest arrives in. Nullable
  // on purpose: null IS the setting "follow the workspace zone" (brief_config
  // / digest_config → TZ env → UTC), so the fallback chain lives in the
  // column's null state rather than a sentinel value that would need its own
  // validation. The value is an IANA name, validated at the profile-PUT
  // boundary and trusted after that; a stale or hand-edited bad name degrades
  // to UTC inside localMoment rather than stopping scheduled work.
  `alter table users add column if not exists timezone text`,

  // ── ORG SHARED DRIVE ────────────────────────────────────────────────────────
  //
  // The Shared Drive provisioning creates (google_org_connection.shared_drive_
  // id) — the team-owned container everyone at the Workspace domain can reach.
  // Kept SEPARATE from drive_folder_id (the export target, which provisioning
  // also points at the drive's root) so an admin can retarget exports into a
  // subfolder without losing track of the drive itself.
  `alter table google_org_connection add column if not exists shared_drive_id text`,

  // ── PER-AGENT EMAIL ALIAS ───────────────────────────────────────────────────
  //
  // An OPTIONAL override of an agent's derived send address. The default is
  // derived, not stored: the org account's plus-address for the agent's slug
  // (org+triage@domain — the Rust aliasing derivation, api/src/google/org.rs),
  // which needs no storage and
  // no Google-side setup. This column exists only for the rare agent whose
  // address should differ — a verified send-as, a differently-named plus-tag —
  // which is why it is null for every row this migration touches.
  `alter table agent_defs add column if not exists email_alias text`,

  // ── BRIEF MIRRORS INTO THE AGENT CABINETS ──────────────────────────────────
  //
  // Brief mirrors used to file at the root of My Files, where they piled up
  // one "Daily brief — <date>" doc per day per person. They now file under
  // Agents/<agent>/Briefs like every other agent output (see
  // agentCategoryFolder), and the mirror self-heals the folder on any later
  // append — but a finished day never appends again, so installs that already
  // have briefs need the move done for them.
  //
  // Two statements, because the Agents root has to exist before anything can
  // point at it. Both are no-ops on installs with no mirrored briefs (fresh
  // ones included), and a brief someone already filed by hand keeps its
  // folder — only rootless mirrors move.
  //
  // THE SECOND STATEMENT WAS REVISED IN PLACE after one dev database had
  // already applied it — the boot-time checksum check refused the edit, which
  // is exactly what it is for. The revision ships as its own statement at the
  // END of this array instead (see "brief mirrors, take two"); this one stays
  // byte-for-byte what that database recorded. Do not "fix" it to match.
  `insert into artifact_folders (name, created_by)
   select 'Agents', 'system'
   where not exists (select 1 from artifact_folders where parent_id is null and name = 'Agents')
     and exists (select 1 from daily_briefs where artifact_id is not null)`,
  `with names as (
     select distinct coalesce(b.agent_name, 'Your assistant') as name
     from daily_briefs b
     where b.artifact_id is not null
   ),
   root as (
     select id from artifact_folders where parent_id is null and name = 'Agents' limit 1
   ),
   inserted as (
     insert into artifact_folders (name, parent_id, created_by)
     select n.name, r.id, 'system'
     from names n cross join root r
     where not exists (
       select 1 from artifact_folders f where f.parent_id = r.id and f.name = n.name
     )
     returning id, name
   ),
   agent_folders as (
     select id, name from inserted
     union
     select f.id, f.name
     from artifact_folders f
     cross join root r
     join names n on f.name = n.name
     where f.parent_id = r.id
   ),
   briefs as (
     insert into artifact_folders (name, parent_id, created_by)
     select 'Briefs', a.id, 'system'
     from agent_folders a
     where not exists (
       select 1 from artifact_folders g where g.parent_id = a.id and g.name = 'Briefs'
     )
     returning id, parent_id
   )
   update artifacts art
   set folder_id = (
     select g.id
     from briefs g
     join agent_folders a on g.parent_id = a.id
     join names n on a.name = n.name
     join daily_briefs b on b.artifact_id = art.id and coalesce(b.agent_name, 'Your assistant') = n.name
     limit 1
   )
   where art.folder_id is null
     and exists (select 1 from daily_briefs b where b.artifact_id = art.id)`,

  // ── ORG-WIDE BOARDS ────────────────────────────────────────────────────────
  //
  // The workspace's own surfaces (the Helpdesk — see the Rust boards engine,
  // api/src/boards.rs). The flag marks ownership; access is materialized as
  // ordinary board_members rows by the ensure and sign-in grants there, so
  // nothing about how boards are listed, shared, or authorized changes.
  `alter table boards add column if not exists org_wide boolean not null default false`,

  // ── BRIEF MIRRORS, TAKE TWO ────────────────────────────────────────────────
  //
  // The reparent above only points a mirror at a Briefs folder it CREATED —
  // its `briefs` CTE is the bare insert. An agent whose Briefs folder the
  // runtime append path had already built (the brief engine creates it on
  // append, so this is any upgrade where a brief landed between the two boots)
  // was missed: the folder existed, the insert created nothing,
  // and that agent's rootless mirrors stayed at My Files forever — the exact
  // install the migration was written for.
  //
  // This is the revision of that statement, with the union arm that also
  // collects the pre-existing Briefs folders. It exists as its own entry, and
  // not as an edit to the one above, because that one had already applied to
  // a dev database when the gap was found — the append-only checksum check
  // refused the in-place edit, correctly, so the fix appends. A no-op
  // everywhere the earlier statement already covered every agent (the folders
  // did not exist when it ran, so its insert covered them all).
  `with names as (
     select distinct coalesce(b.agent_name, 'Your assistant') as name
     from daily_briefs b
     where b.artifact_id is not null
   ),
   root as (
     select id from artifact_folders where parent_id is null and name = 'Agents' limit 1
   ),
   inserted as (
     insert into artifact_folders (name, parent_id, created_by)
     select n.name, r.id, 'system'
     from names n cross join root r
     where not exists (
       select 1 from artifact_folders f where f.parent_id = r.id and f.name = n.name
     )
     returning id, name
   ),
   agent_folders as (
     select id, name from inserted
     union
     select f.id, f.name
     from artifact_folders f
     cross join root r
     join names n on f.name = n.name
     where f.parent_id = r.id
   ),
   briefs_new as (
     insert into artifact_folders (name, parent_id, created_by)
     select 'Briefs', a.id, 'system'
     from agent_folders a
     where not exists (
       select 1 from artifact_folders g where g.parent_id = a.id and g.name = 'Briefs'
     )
     returning id, parent_id
   ),
   briefs as (
     select id, parent_id from briefs_new
     union
     -- An agent whose Briefs folder the code path already built is not in the
     -- insert above, but its mirrors still need to point at that folder.
     select g.id, g.parent_id
     from artifact_folders g
     join agent_folders a on g.parent_id = a.id
     where g.name = 'Briefs'
   )
   update artifacts art
   set folder_id = (
     select g.id
     from briefs g
     join agent_folders a on g.parent_id = a.id
     join names n on a.name = n.name
     join daily_briefs b on b.artifact_id = art.id and coalesce(b.agent_name, 'Your assistant') = n.name
     limit 1
   )
   where art.folder_id is null
     and exists (select 1 from daily_briefs b where b.artifact_id = art.id)`,

  // ── DB-BACKED PASSWORD ACCOUNTS ────────────────────────────────────────────
  // Replaces env AUTH_USERS. One row per account: the scrypt hash produced by
  // hashPassword() in auth/password.ts (format scrypt$N$r$p$salt$hash, params
  // in-band), keyed by the unique lowercased email and linked to the users row
  // it authenticates. user_id is the primary key, so one account per person;
  // deleting the person deletes the account, removing the account keeps the
  // person. Precedents: llm_api_keys, agent_keys, mcp_user_credentials.
  `create table if not exists user_password_credentials (
     user_id uuid primary key references users(id) on delete cascade,
     email text not null unique,
     password_hash text not null,
     created_at timestamptz not null default now(),
     updated_at timestamptz not null default now(),
     last_used_at timestamptz
   )`,
  // ── Full token accounting (usage.ts) ──────────────────────────────────────
  // prompt/completion were never the whole bill. Anthropic prices cache WRITES
  // at 1.25x input and cache READS at 0.1x input, both SEPARATE from
  // input_tokens (so the ledger understated); OpenAI-compatible providers fold
  // cached input into prompt_tokens at the full rate (so it overstated).
  // Reasoning tokens ride inside completion_tokens at the output rate —
  // recorded for visibility, never re-priced. Splitting them out is what makes
  // the ledger match the invoice.
  `alter table usage_events add column if not exists cache_write_tokens integer not null default 0`,
  `alter table usage_events add column if not exists cache_read_tokens integer not null default 0`,
  `alter table usage_events add column if not exists reasoning_tokens integer not null default 0`,

  // ── Per-key gateway policy (#265) ──────────────────────────────────────────
  // A spend cap and a request-rate ceiling attached to the key itself, set by
  // the key's owner in Settings → API keys. The cap rides the #243 budget
  // machinery (see checkBudget in llm-gateway.ts, which min-merges it with any
  // admin ceiling — a key owner can lower their own limit, never raise past an
  // admin's) over the org budget window; the rate ceiling is a fixed-window
  // Redis counter at the llm.v1 route. All three null by default: unlimited.
  `alter table llm_api_keys add column if not exists spend_cap_tokens bigint`,
  `alter table llm_api_keys add column if not exists spend_cap_usd numeric`,
  `alter table llm_api_keys add column if not exists rate_limit_per_minute integer`,

  // ── The plan-draft key is a conversation OR a channel ─────────────────────
  // plan_drafts.conversation_id was born referencing conversations(id), but
  // the same commit that created it also wired the CHANNEL Plan button to
  // store the channel id there (channels are not conversations) — so every
  // channel draft insert died on this constraint: the POST answered 500 with
  // an orphaned queued run behind it, which the sweep then drove (a billed
  // model call whose saveResult updated zero rows). Found while porting the
  // plan-draft plane to Rust. Dropped rather than re-pointed: nothing hard-
  // deletes conversations (the cascade never fired), and channel deletion
  // was never covered by this constraint in the first place.
  `alter table plan_drafts drop constraint if exists plan_drafts_conversation_id_fkey`,

  // ── MERGE THE FORKED USERS (Google sign-in on a claimed email) ───────────
  // A sign-in used to resolve people by sub alone, and subs are per-door
  // (google:<subject> vs password:<email>) — so a Google login for an email
  // that already had a row forked a second users row and the person kept two
  // identities: their admin powers on one, their Google sessions on the
  // other. Sign-in links by email now (users.rs link_by_email); this
  // statement merges the rows the old behavior already forked, at boot,
  // before the api starts. One survivor per lower(email) group — admin
  // first, then lowest id — takes over everything pointing at its
  // duplicates, and the duplicate rows are deleted. Everything runs inside
  // ONE statement on purpose: each element of this array is its own
  // transaction, and a merge split across elements could die half-done with
  // no way to re-run the first half. A second boot finds no groups and
  // no-ops.
  //
  // Deliberately untouched: audit_log.actor, tasks.assigned_to/created_by,
  // channel_messages.author and the other email/name-keyed text columns
  // (history, keyed by what humans read); channels.dm_key (rewriting a
  // merged DM's key could collide with the survivor's own DM channel on
  // channels_dm_key_idx — channel_members is re-pointed, so the history
  // stays visible). The survivor KEEPS its sub: password login reads it off
  // the row, and the next Google sign-in moves it via link_by_email — this
  // statement never guesses which door the person will use next. Membership
  // rows the survivor already holds win; the duplicate's colliding row dies
  // in the final cascade. Live sessions of merged-away rows age out
  // (Redis, 7-day TTL); those people sign in once more and land on the
  // survivor.
  `with
     grp as (
       select id,
              lower(email)                                          as email_key,
              row_number() over (partition by lower(email)
                                 order by (role = 'admin') desc, id) as rn,
              count(*)     over (partition by lower(email))          as n
       from users
       where email is not null and email <> ''
     ),
     pairs as (
       select g.id as dup_id, s.id as survivor_id
       from grp g
       join grp s on s.email_key = g.email_key and s.rn = 1
       where g.rn > 1 and s.n > 1
     ),

     conversations_rp as (
       update conversations c set user_id = p.survivor_id
       from pairs p where c.user_id = p.dup_id
     ),
     boards_rp as (
       update boards b set owner_id = p.survivor_id
       from pairs p where b.owner_id = p.dup_id
     ),
     notifications_rp as (
       update notifications n set user_id = p.survivor_id
       from pairs p where n.user_id = p.dup_id
     ),
     llm_api_keys_rp as (
       update llm_api_keys k set user_id = p.survivor_id
       from pairs p where k.user_id = p.dup_id
     ),
     rag_collections_rp as (
       update rag_collections r set owner_user_id = p.survivor_id
       from pairs p where r.owner_user_id = p.dup_id
     ),
     outreach_rp as (
       update outreach_events e set user_id = p.survivor_id
       from pairs p where e.user_id = p.dup_id
     ),
     inbox_decisions_rp as (
       update inbox_decisions d set user_id = p.survivor_id
       from pairs p where d.user_id = p.dup_id
     ),
     runs_rp as (
       update runs r set owner_user_id = p.survivor_id
       from pairs p where r.owner_user_id = p.dup_id
     ),
     secret_folders_rp as (
       update secret_folders f set owner_user_id = p.survivor_id
       from pairs p where f.owner_user_id = p.dup_id
     ),
     agent_defs_rp as (
       update agent_defs d set owner_user_id = p.survivor_id
       from pairs p where d.owner_user_id = p.dup_id
     ),

     teams_rp as (
       update teams t set created_by = p.survivor_id
       from pairs p where t.created_by = p.dup_id
     ),
     invites_rp as (
       update invites i set accepted_user_id = p.survivor_id
       from pairs p where i.accepted_user_id = p.dup_id
     ),
     channels_rp as (
       update channels c set created_by = p.survivor_id
       from pairs p where c.created_by = p.dup_id
     ),
     uploads_rp as (
       update uploads u set uploaded_by = p.survivor_id
       from pairs p where u.uploaded_by = p.dup_id
     ),
     kb_comments_rp as (
       update kb_comments c set author_user_id = p.survivor_id
       from pairs p where c.author_user_id = p.dup_id
     ),
     kb_docs_rp as (
       update kb_docs d set owner_user_id = p.survivor_id
       from pairs p where d.owner_user_id = p.dup_id
     ),
     kb_spaces_rp as (
       update kb_spaces s set owner_user_id = p.survivor_id
       from pairs p where s.owner_user_id = p.dup_id
     ),
     artifacts_rp as (
       update artifacts a set owner_user_id = p.survivor_id
       from pairs p where a.owner_user_id = p.dup_id
     ),
     artifact_folders_rp as (
       update artifact_folders f set owner_user_id = p.survivor_id
       from pairs p where f.owner_user_id = p.dup_id
     ),
     google_org_conn_rp as (
       update google_org_connection g set connected_by = p.survivor_id
       from pairs p where g.connected_by = p.dup_id
     ),
     google_pending_owner_rp as (
       update google_pending_actions a set owner_user_id = p.survivor_id
       from pairs p where a.owner_user_id = p.dup_id
     ),
     google_pending_decided_rp as (
       update google_pending_actions a set decided_by = p.survivor_id
       from pairs p where a.decided_by = p.dup_id
     ),
     messages_rp as (
       update messages m set author_user_id = p.survivor_id
       from pairs p where m.author_user_id = p.dup_id
     ),
     research_runs_rp as (
       update research_runs r set owner_user_id = p.survivor_id
       from pairs p where r.owner_user_id = p.dup_id
     ),
     workspace_secrets_rp as (
       update workspace_secrets s set owner_user_id = p.survivor_id
       from pairs p where s.owner_user_id = p.dup_id
     ),
     drafts_rp as (
       update assistant_reply_drafts d set user_id = p.survivor_id
       from pairs p where d.user_id = p.dup_id
     ),
     drafts_decided_rp as (
       update assistant_reply_drafts d set decided_by = p.survivor_id
       from pairs p where d.decided_by = p.dup_id
     ),
     plan_drafts_rp as (
       update plan_drafts d set created_by = p.survivor_id
       from pairs p where d.created_by = p.dup_id
     ),

     user_agent_access_rp as (
       update user_agent_access a set user_id = p.survivor_id
       from pairs p
       where a.user_id = p.dup_id
         and not exists (select 1 from user_agent_access x
                         where x.user_id = p.survivor_id
                           and x.agent_model = a.agent_model)
     ),
     team_members_rp as (
       update team_members m set user_id = p.survivor_id
       from pairs p
       where m.user_id = p.dup_id
         and not exists (select 1 from team_members x
                         where x.user_id = p.survivor_id
                           and x.team_id = m.team_id)
     ),
     board_members_rp as (
       update board_members m set user_id = p.survivor_id
       from pairs p
       where m.user_id = p.dup_id
         and not exists (select 1 from board_members x
                         where x.user_id = p.survivor_id
                           and x.board_id = m.board_id)
     ),
     mcp_user_access_rp as (
       update mcp_user_access a set user_id = p.survivor_id
       from pairs p
       where a.user_id = p.dup_id
         and not exists (select 1 from mcp_user_access x
                         where x.user_id = p.survivor_id
                           and x.server_id = a.server_id)
     ),
     mcp_user_credentials_rp as (
       update mcp_user_credentials c set user_id = p.survivor_id
       from pairs p
       where c.user_id = p.dup_id
         and not exists (select 1 from mcp_user_credentials x
                         where x.user_id = p.survivor_id
                           and x.server_id = c.server_id)
     ),
     user_permissions_rp as (
       update user_permissions u set user_id = p.survivor_id
       from pairs p
       where u.user_id = p.dup_id
         and not exists (select 1 from user_permissions x
                         where x.user_id = p.survivor_id
                           and x.perm = u.perm)
     ),
     channel_members_rp as (
       update channel_members m set user_id = p.survivor_id
       from pairs p
       where m.user_id = p.dup_id
         and not exists (select 1 from channel_members x
                         where x.user_id = p.survivor_id
                           and x.channel_id = m.channel_id)
     ),
     conversation_members_rp as (
       update conversation_members m set user_id = p.survivor_id
       from pairs p
       where m.user_id = p.dup_id
         and not exists (select 1 from conversation_members x
                         where x.user_id = p.survivor_id
                           and x.conversation_id = m.conversation_id)
     ),
     research_members_rp as (
       update research_members m set user_id = p.survivor_id
       from pairs p
       where m.user_id = p.dup_id
         and not exists (select 1 from research_members x
                         where x.user_id = p.survivor_id
                           and x.run_id = m.run_id)
     ),
     inbox_focus_state_rp as (
       update inbox_focus_state f set user_id = p.survivor_id
       from pairs p
       where f.user_id = p.dup_id
         and not exists (select 1 from inbox_focus_state x
                         where x.user_id = p.survivor_id
                           and x.source_type = f.source_type
                           and x.source_id = f.source_id)
     ),
     secret_readers_rp as (
       update workspace_secret_readers r set user_id = p.survivor_id
       from pairs p
       where r.user_id = p.dup_id
         and not exists (select 1 from workspace_secret_readers x
                         where x.user_id = p.survivor_id
                           and x.secret_id = r.secret_id)
     ),
     folder_readers_rp as (
       update secret_folder_readers r set user_id = p.survivor_id
       from pairs p
       where r.user_id = p.dup_id
         and not exists (select 1 from secret_folder_readers x
                         where x.user_id = p.survivor_id
                           and x.folder_id = r.folder_id)
     ),
     daily_briefs_rp as (
       update daily_briefs b set user_id = p.survivor_id
       from pairs p
       where b.user_id = p.dup_id
         and not exists (select 1 from daily_briefs x
                         where x.user_id = p.survivor_id
                           and x.brief_date = b.brief_date)
     ),
     grants_thread_rp as (
       update assistant_reply_grants g set user_id = p.survivor_id
       from pairs p
       where g.user_id = p.dup_id
         and g.channel_id is not null
         and g.revoked_at is null
         and not exists (select 1 from assistant_reply_grants x
                         where x.user_id = p.survivor_id
                           and x.channel_id = g.channel_id
                           and x.revoked_at is null)
     ),
     grants_standing_rp as (
       update assistant_reply_grants g set user_id = p.survivor_id
       from pairs p
       where g.user_id = p.dup_id
         and g.channel_id is null
         and g.revoked_at is null
         and not exists (select 1 from assistant_reply_grants x
                         where x.user_id = p.survivor_id
                           and x.channel_id is null
                           and x.revoked_at is null)
     ),
     grants_revoked_rp as (
       update assistant_reply_grants g set user_id = p.survivor_id
       from pairs p
       where g.user_id = p.dup_id
         and g.revoked_at is not null
     ),

     google_conn_dupe as (
       delete from google_connections c using pairs p
       where c.user_id = p.dup_id
         and exists (select 1 from google_connections x
                     where x.user_id = p.survivor_id)
     ),
     google_conn_rp as (
       update google_connections c set user_id = p.survivor_id
       from pairs p
       where c.user_id = p.dup_id
         and not exists (select 1 from google_connections x
                         where x.user_id = p.survivor_id)
     ),
     password_creds_dupe as (
       delete from user_password_credentials c using pairs p
       where c.user_id = p.dup_id
         and exists (select 1 from user_password_credentials x
                     where x.user_id = p.survivor_id)
     ),
     password_creds_rp as (
       update user_password_credentials c set user_id = p.survivor_id
       from pairs p
       where c.user_id = p.dup_id
         and not exists (select 1 from user_password_credentials x
                         where x.user_id = p.survivor_id)
     ),

     kb_editors_rp as (
       update kb_editors e set principal_id = p.survivor_id::text
       from pairs p
       where e.principal_type = 'user'
         and e.principal_id = p.dup_id::text
         and not exists (select 1 from kb_editors x
                         where x.item_type = e.item_type
                           and x.item_id = e.item_id
                           and x.principal_type = 'user'
                           and x.principal_id = p.survivor_id::text)
     ),
     rag_access_rp as (
       update rag_collection_access a set principal_id = p.survivor_id::text
       from pairs p
       where a.principal_type = 'user'
         and a.principal_id = p.dup_id::text
         and not exists (select 1 from rag_collection_access x
                         where x.collection_id = a.collection_id
                           and x.principal_type = 'user'
                           and x.principal_id = p.survivor_id::text)
     ),

     gone as (
       delete from users u using pairs p where u.id = p.dup_id returning u.id
     )
   select count(*) as merged_rows from gone`,

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

async function runMigrations(): Promise<MigrationResult> {
  const sql = migrationSql()
  try {
    await sql`select pg_advisory_lock(${MIGRATION_LOCK})`
    await sql.unsafe(SCHEMA_MIGRATIONS)
    const rows = (await sql`select id, checksum from schema_migrations`) as unknown as Array<{ id: number; checksum: string }>
    const applied = new Map(rows.map((r) => [r.id, r.checksum]))
    let fresh = 0
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
      fresh += 1
    }
    // Load/create the data key so seal()/open() are synchronous thereafter.
    await initSecretbox(sql)
    g.__talariaMigrationCount = MIGRATIONS.length
    return { applied: fresh, total: MIGRATIONS.length }
  } finally {
    // Ending the pool ends the session, which releases the advisory lock; the
    // explicit unlock is so a slow close does not hold up another instance.
    await sql`select pg_advisory_unlock(${MIGRATION_LOCK})`.catch(() => {})
    await sql.end({ timeout: 5 }).catch(() => {})
  }
}

/** What a migration pass did: how many statements it applied fresh, and the
 *  size of the array it reconciled against. `applied: 0` = already current. */
export type MigrationResult = { applied: number; total: number }

function ensureMigrated(): Promise<MigrationResult> {
  // THE COUNT IS CACHED WITH THE PROMISE, and both ride globalThis — because a
  // vite dev SSR module reload keeps globalThis while swapping the code. The
  // promise alone would then answer "migrated" for an array that GREW since
  // the process booted, and every query touching the new column would 500
  // ("column does not exist") until somebody restarted the dev server —
  // exactly the trap `preferred_effort` fell into. Growing is the only change
  // the append-only rule permits, so growth is the only thing that re-arms
  // the run: the statements already applied are no-ops against
  // schema_migrations, the appended ones apply, and an edit to an APPLIED
  // statement is still caught by the checksum check inside.
  if (g.__talariaMigrated && MIGRATIONS.length > (g.__talariaMigrationCount ?? -1)) {
    g.__talariaMigrated = undefined
  }
  if (!g.__talariaMigrated) {
    const run: Promise<MigrationResult> = runMigrations().catch((e) => {
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

/** Run (or join) the migration pass and report what it did. The lazy path via
 *  db() only fires on a table-backed query — and since the api cutover nothing
 *  in the boot path issues one (healthz is connectivity-only, and the api owns
 *  the tables but no DDL), so boot calls this explicitly: without it, a fresh
 *  database never migrates. See server-entry.js's boot step for the story. */
export function migrate(): Promise<MigrationResult> {
  return ensureMigrated()
}
