// Postgres — durable state (users, roles, per-agent access, conversations,
// messages). postgres.js (no native build). Migrations run once, lazily, on
// first query. Cached on globalThis so HMR doesn't open a new pool each reload.

import postgres from 'postgres'

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
]

function ensureMigrated(): Promise<void> {
  if (!g.__talariaMigrated) {
    const sql = getSql()
    g.__talariaMigrated = (async () => {
      for (const stmt of MIGRATIONS) await sql.unsafe(stmt)
    })()
  }
  return g.__talariaMigrated
}

/** Migrated Postgres handle. `const sql = await db()`. */
export async function db(): Promise<Sql> {
  await ensureMigrated()
  return getSql()
}
