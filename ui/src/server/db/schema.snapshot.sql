-- Talaria schema snapshot — the schema a fresh database reaches after the
-- full MIGRATIONS array in ui/src/server/db/pg.ts runs. Regenerate after an
-- intentional migration: cd ui && bun run migrations:snapshot
-- (then commit the diff — it is the PR's schema change, in review form.)
SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;
SET default_tablespace = '';
SET default_table_access_method = heap;
CREATE TABLE public.agent_defs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    slug text NOT NULL,
    department text NOT NULL,
    model text NOT NULL,
    display_name text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    managed boolean DEFAULT false NOT NULL,
    current_version integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    source text DEFAULT 'imported'::text NOT NULL,
    role text,
    owner_user_id uuid,
    gateway_port integer,
    ticket_template_id uuid,
    plan_template_id uuid,
    active_slot text DEFAULT 'a'::text NOT NULL,
    elevated boolean DEFAULT false NOT NULL,
    workbench text DEFAULT 'auto'::text NOT NULL,
    workbench_profile text,
    workbench_harness text,
    workbench_models jsonb DEFAULT '{}'::jsonb NOT NULL,
    proactive boolean DEFAULT false NOT NULL,
    email_alias text
);
CREATE TABLE public.agent_keys (
    agent_id uuid NOT NULL,
    key_hash text NOT NULL,
    key_enc text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone
);
CREATE TABLE public.agent_role_templates (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    slug text NOT NULL,
    name text NOT NULL,
    role text NOT NULL,
    department text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    soul text NOT NULL,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.agent_secrets (
    agent_id uuid NOT NULL,
    name text NOT NULL,
    value_enc text NOT NULL,
    updated_by text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.agent_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    agent_id uuid NOT NULL,
    version integer NOT NULL,
    soul text DEFAULT ''::text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    note text,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.app_data (
    app text NOT NULL,
    collection text NOT NULL,
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    data jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.app_settings (
    key text NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.artifact_folders (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text DEFAULT 'Untitled'::text NOT NULL,
    icon text,
    parent_id uuid,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    visibility text DEFAULT 'org'::text NOT NULL,
    edit_policy text DEFAULT 'org'::text NOT NULL,
    owner_user_id uuid
);
CREATE TABLE public.artifact_links (
    artifact_id uuid NOT NULL,
    target_type text NOT NULL,
    target_id text NOT NULL,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.artifacts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    kind text DEFAULT 'doc'::text NOT NULL,
    title text DEFAULT 'Untitled'::text NOT NULL,
    icon text,
    body text DEFAULT ''::text NOT NULL,
    content_type text,
    storage_ref text,
    visibility text DEFAULT 'private'::text NOT NULL,
    edit_policy text DEFAULT 'owner'::text NOT NULL,
    public_slug text,
    official boolean DEFAULT false NOT NULL,
    kb_doc_id uuid,
    owner_user_id uuid,
    created_by text,
    updated_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    folder_id uuid,
    google_file_id text,
    google_file_url text,
    rag_routing text DEFAULT 'auto'::text NOT NULL
);
CREATE TABLE public.assistant_reply_drafts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    channel_id uuid NOT NULL,
    in_reply_to_seq integer NOT NULL,
    agent_model text,
    content text NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    delegated boolean DEFAULT false NOT NULL,
    message_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    decided_at timestamp with time zone,
    decided_by uuid
);
CREATE TABLE public.assistant_reply_grants (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    channel_id uuid,
    granted_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone
);
CREATE TABLE public.audit_log (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    actor text NOT NULL,
    action text NOT NULL,
    target_type text NOT NULL,
    target_id text,
    target_label text,
    before jsonb,
    after jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.board_agent_requests (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    board_id uuid NOT NULL,
    agent_model text NOT NULL,
    requested_by_user_id uuid,
    reason text,
    status text DEFAULT 'open'::text NOT NULL,
    decided_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    decided_at timestamp with time zone
);
CREATE TABLE public.board_agents (
    board_id uuid NOT NULL,
    agent_model text NOT NULL
);
CREATE TABLE public.board_labels (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    board_id uuid NOT NULL,
    name text NOT NULL,
    color text DEFAULT 'slate'::text NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.board_members (
    board_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text DEFAULT 'editor'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.board_statuses (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    board_id uuid NOT NULL,
    key text NOT NULL,
    label text NOT NULL,
    color text DEFAULT 'slate'::text NOT NULL,
    category text DEFAULT 'active'::text NOT NULL,
    agent_start boolean DEFAULT false NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.board_templates (
    board_id uuid NOT NULL,
    template_id uuid NOT NULL,
    is_default boolean DEFAULT false NOT NULL
);
CREATE TABLE public.board_views (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    board_id uuid NOT NULL,
    name text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_by text NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.boards (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    owner_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    team_id uuid,
    ticket_prefix text,
    ticket_seq integer DEFAULT 0 NOT NULL,
    allow_all_agents boolean DEFAULT false NOT NULL,
    archived_at timestamp with time zone,
    judge_mode text DEFAULT 'inherit'::text NOT NULL,
    org_wide boolean DEFAULT false NOT NULL
);
CREATE TABLE public.capability_gaps (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    signature text NOT NULL,
    kind text NOT NULL,
    board_id uuid,
    agent_model text NOT NULL,
    missing text NOT NULL,
    needs text DEFAULT ''::text NOT NULL,
    example_task_id uuid,
    seen_count integer DEFAULT 1 NOT NULL,
    status text DEFAULT 'open'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.channel_agents (
    channel_id uuid NOT NULL,
    agent_model text NOT NULL
);
CREATE TABLE public.channel_members (
    channel_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text DEFAULT 'member'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_read_seq integer DEFAULT 0 NOT NULL
);
CREATE TABLE public.channel_message_reactions (
    message_id uuid NOT NULL,
    emoji text NOT NULL,
    actor text NOT NULL,
    actor_type text DEFAULT 'user'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.channel_messages (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    channel_id uuid NOT NULL,
    seq integer NOT NULL,
    author_type text NOT NULL,
    author text NOT NULL,
    content text DEFAULT ''::text NOT NULL,
    status text DEFAULT 'complete'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    thread_root_id uuid,
    edited_at timestamp with time zone,
    attachments jsonb DEFAULT '[]'::jsonb NOT NULL,
    guard jsonb
);
CREATE TABLE public.channels (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    topic text,
    created_by uuid,
    msg_seq integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    archived_at timestamp with time zone,
    kind text DEFAULT 'channel'::text NOT NULL,
    dm_key text
);
CREATE TABLE public.conversation_members (
    conversation_id uuid NOT NULL,
    user_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.conversation_reads (
    conversation_id uuid NOT NULL,
    user_id uuid NOT NULL,
    last_read_seq integer DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.conversations (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    agent_model text NOT NULL,
    title text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    archived boolean DEFAULT false NOT NULL,
    kind text DEFAULT 'chat'::text NOT NULL,
    plan_template_id uuid
);
CREATE TABLE public.daily_brief_entries (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    brief_id uuid NOT NULL,
    seq integer NOT NULL,
    kind text NOT NULL,
    section text DEFAULT 'action'::text NOT NULL,
    source_key text,
    source_type text,
    source_id text,
    source_href text,
    fingerprint text,
    supersedes uuid,
    priority text,
    status_label text,
    badge jsonb,
    title text DEFAULT ''::text NOT NULL,
    body text DEFAULT ''::text NOT NULL,
    evidence jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    batch uuid
);
CREATE TABLE public.daily_briefs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    brief_date date NOT NULL,
    zone text DEFAULT 'UTC'::text NOT NULL,
    agent_model text,
    agent_name text,
    artifact_id uuid,
    last_seq integer DEFAULT 0 NOT NULL,
    read_seq integer DEFAULT 0 NOT NULL,
    last_swept_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.fitness_transcripts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    model text NOT NULL,
    run_started_at timestamp with time zone NOT NULL,
    harness text NOT NULL,
    case_name text NOT NULL,
    band text DEFAULT 'standard'::text NOT NULL,
    verdict text DEFAULT 'pass'::text NOT NULL,
    prompt text,
    raw text,
    turns jsonb,
    tool_calls jsonb,
    upstream jsonb,
    latency_ms integer DEFAULT 0 NOT NULL,
    prompt_tokens integer DEFAULT 0 NOT NULL,
    completion_tokens integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    wall_ms integer DEFAULT 0 NOT NULL
);
CREATE TABLE public.fleet_agents (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    role text DEFAULT 'agent'::text NOT NULL,
    status text DEFAULT 'offline'::text NOT NULL,
    last_seen timestamp with time zone,
    last_activity text,
    framework text,
    capabilities jsonb DEFAULT '[]'::jsonb NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.google_connections (
    user_id uuid NOT NULL,
    google_sub text NOT NULL,
    email text,
    scope text DEFAULT ''::text NOT NULL,
    refresh_token_enc text,
    access_token_enc text,
    access_expires_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.google_org_connection (
    id integer DEFAULT 1 NOT NULL,
    google_sub text NOT NULL,
    email text,
    scope text DEFAULT ''::text NOT NULL,
    refresh_token_enc text,
    access_token_enc text,
    access_expires_at timestamp with time zone,
    connected_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    drive_folder_id text,
    calendar_id text,
    send_as text,
    shared_drive_id text,
    CONSTRAINT google_org_connection_id_check CHECK ((id = 1))
);
CREATE TABLE public.google_pending_actions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    kind text NOT NULL,
    payload jsonb NOT NULL,
    summary text,
    agent_model text,
    owner_user_id uuid,
    status text DEFAULT 'pending'::text NOT NULL,
    result jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    decided_at timestamp with time zone,
    decided_by uuid,
    is_org boolean DEFAULT false NOT NULL
);
CREATE TABLE public.guard_findings (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    caller text,
    model text,
    endpoint text,
    mode text DEFAULT 'observe'::text NOT NULL,
    check_type text NOT NULL,
    severity text DEFAULT 'medium'::text NOT NULL,
    message text DEFAULT ''::text NOT NULL,
    snippet text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    confidence real DEFAULT 0.5 NOT NULL
);
CREATE TABLE public.harness_runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    harness text NOT NULL,
    model text,
    chain_step text,
    widened boolean DEFAULT false NOT NULL,
    repairs integer DEFAULT 0 NOT NULL,
    schema_valid boolean DEFAULT false NOT NULL,
    latency_ms integer DEFAULT 0 NOT NULL,
    findings integer DEFAULT 0 NOT NULL,
    caller text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    error text
);
CREATE TABLE public.inbox_decisions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    source_type text NOT NULL,
    source_id text NOT NULL,
    instruction text,
    action_id text,
    agent_model text,
    delegate_model text,
    status text NOT NULL,
    proposal jsonb,
    outcome jsonb,
    confirmation_token_hash text,
    expires_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    confirmed_at timestamp with time zone,
    completed_at timestamp with time zone,
    conversation_id uuid,
    user_message_id uuid,
    assistant_message_id uuid,
    focus_context jsonb
);
CREATE TABLE public.inbox_focus_state (
    user_id uuid NOT NULL,
    source_type text NOT NULL,
    source_id text NOT NULL,
    snoozed_until timestamp with time zone,
    viewed_at timestamp with time zone,
    content_fingerprint text,
    brief jsonb,
    brief_generated_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.internal_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    kind text NOT NULL,
    owner_key text NOT NULL,
    content text NOT NULL,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.invites (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    email text NOT NULL,
    token text NOT NULL,
    invited_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    accepted_at timestamp with time zone,
    accepted_user_id uuid,
    revoked_at timestamp with time zone
);
CREATE TABLE public.judge_reviews (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    task_id uuid NOT NULL,
    model text,
    verdict text NOT NULL,
    summary text DEFAULT ''::text NOT NULL,
    issues jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.kb_comments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    doc_id uuid NOT NULL,
    parent_id uuid,
    author_user_id uuid,
    author text NOT NULL,
    quote text,
    content text NOT NULL,
    resolved boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.kb_docs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    space_id uuid NOT NULL,
    parent_id uuid,
    title text DEFAULT 'Untitled'::text NOT NULL,
    body text DEFAULT ''::text NOT NULL,
    kind text DEFAULT 'human'::text NOT NULL,
    official boolean DEFAULT false NOT NULL,
    visibility text DEFAULT 'org'::text NOT NULL,
    public_slug text,
    sort integer DEFAULT 0 NOT NULL,
    created_by text,
    updated_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    okf text,
    icon text,
    owner_user_id uuid,
    edit_policy text DEFAULT 'org'::text NOT NULL,
    perms_inherited boolean DEFAULT true NOT NULL,
    rag_routing text DEFAULT 'auto'::text NOT NULL
);
CREATE TABLE public.kb_editors (
    item_type text NOT NULL,
    item_id uuid NOT NULL,
    principal_type text NOT NULL,
    principal_id text NOT NULL,
    role text DEFAULT 'editor'::text NOT NULL
);
CREATE TABLE public.kb_spaces (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    description text,
    icon text,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    okf_doc_id uuid,
    body text DEFAULT ''::text NOT NULL,
    owner_user_id uuid,
    visibility text DEFAULT 'org'::text NOT NULL,
    public_slug text,
    edit_policy text DEFAULT 'org'::text NOT NULL,
    rag_collection_id uuid
);
CREATE TABLE public.llm_api_keys (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    key_hash text NOT NULL,
    prefix text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone,
    revoked_at timestamp with time zone,
    spend_cap_tokens bigint,
    spend_cap_usd numeric,
    rate_limit_per_minute integer
);
CREATE TABLE public.llm_endpoints (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    provider text NOT NULL,
    base_url text,
    class text DEFAULT 'cloud'::text NOT NULL,
    api_key_env text,
    context_length integer,
    price_in_per_mtok numeric,
    price_out_per_mtok numeric,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    models jsonb DEFAULT '[]'::jsonb NOT NULL,
    auto_prices jsonb DEFAULT '{}'::jsonb NOT NULL,
    model_prices jsonb DEFAULT '{}'::jsonb NOT NULL,
    request_defaults jsonb DEFAULT '{}'::jsonb NOT NULL,
    api_key_cipher text,
    model_efforts jsonb DEFAULT '{}'::jsonb NOT NULL
);
CREATE TABLE public.mcp_oauth_states (
    state text NOT NULL,
    server_id uuid NOT NULL,
    subject text NOT NULL,
    verifier text NOT NULL,
    redirect_uri text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.mcp_oauth_tokens (
    server_id uuid NOT NULL,
    subject text NOT NULL,
    tokens_enc text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.mcp_server_agents (
    server_id uuid NOT NULL,
    agent_model text NOT NULL,
    tools text[]
);
CREATE TABLE public.mcp_servers (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    label text NOT NULL,
    description text,
    url text NOT NULL,
    headers jsonb DEFAULT '{}'::jsonb NOT NULL,
    timeout_secs integer,
    enabled boolean DEFAULT true NOT NULL,
    all_agents boolean DEFAULT false NOT NULL,
    auth_mode text DEFAULT 'org'::text NOT NULL,
    tools jsonb DEFAULT '[]'::jsonb NOT NULL,
    tools_refreshed_at timestamp with time zone,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    required_headers jsonb DEFAULT '[]'::jsonb NOT NULL,
    builtin boolean DEFAULT false NOT NULL,
    oauth jsonb,
    app_slug text
);
CREATE TABLE public.mcp_user_access (
    server_id uuid NOT NULL,
    user_id uuid NOT NULL,
    allowed boolean DEFAULT true NOT NULL,
    tools text[]
);
CREATE TABLE public.mcp_user_credentials (
    server_id uuid NOT NULL,
    user_id uuid NOT NULL,
    headers_enc text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.messages (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    conversation_id uuid NOT NULL,
    seq integer NOT NULL,
    role text NOT NULL,
    content text DEFAULT ''::text NOT NULL,
    reasoning text DEFAULT ''::text NOT NULL,
    tools jsonb DEFAULT '[]'::jsonb NOT NULL,
    status text DEFAULT 'complete'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    attachments jsonb DEFAULT '[]'::jsonb NOT NULL,
    author_user_id uuid,
    guard jsonb,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL
);
CREATE TABLE public.model_blurbs (
    model_id text NOT NULL,
    blurb text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.notifications (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    kind text NOT NULL,
    title text NOT NULL,
    body text DEFAULT ''::text NOT NULL,
    href text DEFAULT ''::text NOT NULL,
    read_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.org_domains (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    domain text NOT NULL,
    verified boolean DEFAULT false NOT NULL,
    verification_token text NOT NULL,
    added_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    verified_at timestamp with time zone
);
CREATE TABLE public.outreach_events (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    agent_model text NOT NULL,
    kind text NOT NULL,
    user_id uuid,
    conversation_id uuid,
    note text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.plan_drafts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    conversation_id uuid NOT NULL,
    created_by uuid,
    source text NOT NULL,
    agent_model text NOT NULL,
    routed_model text,
    tier text,
    board_id uuid,
    template_id uuid,
    proposals jsonb DEFAULT '[]'::jsonb NOT NULL,
    note text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.push_subscriptions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    endpoint text NOT NULL,
    p256dh text NOT NULL,
    auth text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.quality_reviews (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    task_id uuid NOT NULL,
    reviewer text NOT NULL,
    status text NOT NULL,
    notes text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.rag_collection_access (
    collection_id uuid NOT NULL,
    principal_type text NOT NULL,
    principal_id text
);
CREATE TABLE public.rag_collections (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    kind text NOT NULL,
    qdrant_name text NOT NULL,
    description text,
    auto boolean DEFAULT false NOT NULL,
    embed_dim integer,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    owner_user_id uuid,
    schema_version integer DEFAULT 1 NOT NULL
);
CREATE TABLE public.rag_points (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    collection_id uuid NOT NULL,
    source_type text NOT NULL,
    source_id text NOT NULL,
    point_ids jsonb DEFAULT '[]'::jsonb NOT NULL,
    content_hash text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.research_members (
    run_id uuid NOT NULL,
    user_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.research_runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    owner_user_id uuid,
    requested_by text NOT NULL,
    agent_model text NOT NULL,
    mode text NOT NULL,
    question text NOT NULL,
    status text DEFAULT 'queued'::text NOT NULL,
    phase text,
    artifact_id uuid,
    error text,
    stats jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    title text,
    conversation_id uuid,
    parent_run_id uuid
);
CREATE TABLE public.research_sources (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    run_id uuid NOT NULL,
    idx integer NOT NULL,
    url text NOT NULL,
    title text,
    snippet text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    kind text NOT NULL,
    owner_user_id uuid,
    subject_type text,
    subject_id text,
    state text DEFAULT 'queued'::text NOT NULL,
    phase text DEFAULT ''::text NOT NULL,
    checkpoint jsonb,
    input jsonb DEFAULT '{}'::jsonb NOT NULL,
    result jsonb,
    error text,
    attempt integer DEFAULT 0 NOT NULL,
    lease_owner text,
    lease_expires_at timestamp with time zone,
    approval_key text,
    decision jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone
);
CREATE TABLE public.schema_migrations (
    id integer NOT NULL,
    checksum text NOT NULL,
    applied_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.secret_folder_grants (
    folder_id uuid NOT NULL,
    agent_model text NOT NULL,
    granted_by text,
    granted_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.secret_folder_readers (
    folder_id uuid NOT NULL,
    user_id uuid NOT NULL,
    granted_by text,
    granted_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.secret_folders (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    owner_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.secret_keys (
    version integer NOT NULL,
    wrapped_dek text NOT NULL,
    active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.skill_summaries (
    owner text NOT NULL,
    name text NOT NULL,
    hash text NOT NULL,
    summary text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.task_activity (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    task_id uuid NOT NULL,
    actor text NOT NULL,
    type text NOT NULL,
    description text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.task_dependencies (
    task_id uuid NOT NULL,
    depends_on_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.task_watchers (
    task_id uuid NOT NULL,
    watcher text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.task_workflows (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    match jsonb DEFAULT '{}'::jsonb NOT NULL,
    skills jsonb DEFAULT '[]'::jsonb NOT NULL,
    toolkits jsonb DEFAULT '[]'::jsonb NOT NULL,
    env jsonb DEFAULT '{}'::jsonb NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.tasks (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    board_id uuid NOT NULL,
    title text NOT NULL,
    description text,
    status text DEFAULT 'inbox'::text NOT NULL,
    priority text DEFAULT 'medium'::text NOT NULL,
    assigned_to text,
    created_by text DEFAULT 'user'::text NOT NULL,
    result text,
    tags jsonb DEFAULT '[]'::jsonb NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    due_date timestamp with time zone,
    ticket_no integer,
    estimated_hours numeric,
    actual_hours numeric,
    outcome text,
    resolution text,
    error_message text,
    completed_at timestamp with time zone,
    archived_at timestamp with time zone,
    effort text,
    assignees jsonb DEFAULT '[]'::jsonb NOT NULL,
    time_spent_seconds bigint DEFAULT 0 NOT NULL,
    attachments jsonb DEFAULT '[]'::jsonb NOT NULL,
    parent_id uuid,
    start_date timestamp with time zone,
    color text,
    conversation_id uuid
);
CREATE TABLE public.team_members (
    team_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text DEFAULT 'member'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.teams (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.templates (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    kind text NOT NULL,
    body text DEFAULT ''::text NOT NULL,
    guidance text DEFAULT ''::text NOT NULL,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.uploads (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    filename text NOT NULL,
    mime text NOT NULL,
    size integer NOT NULL,
    path text NOT NULL,
    uploaded_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.usage_events (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    agent_model text NOT NULL,
    source text NOT NULL,
    ref_id uuid,
    prompt_tokens integer DEFAULT 0 NOT NULL,
    completion_tokens integer DEFAULT 0 NOT NULL,
    estimated boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    endpoint_class text,
    llm_model text,
    endpoint text,
    task_id uuid,
    cache_write_tokens integer DEFAULT 0 NOT NULL,
    cache_read_tokens integer DEFAULT 0 NOT NULL,
    reasoning_tokens integer DEFAULT 0 NOT NULL
);
CREATE TABLE public.user_agent_access (
    user_id uuid NOT NULL,
    agent_model text NOT NULL
);
CREATE TABLE public.user_password_credentials (
    user_id uuid NOT NULL,
    email text NOT NULL,
    password_hash text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone
);
CREATE TABLE public.user_permissions (
    user_id uuid NOT NULL,
    perm text NOT NULL,
    allowed boolean NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.users (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    sub text NOT NULL,
    email text,
    name text,
    picture text,
    role text DEFAULT 'member'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    allowed_manage_views text[] DEFAULT '{}'::text[] NOT NULL,
    can_mint_keys boolean DEFAULT false NOT NULL,
    denied_views text[] DEFAULT '{}'::text[] NOT NULL,
    preferred_model text,
    notify_prefs jsonb DEFAULT '{}'::jsonb NOT NULL,
    preferred_effort text,
    timezone text
);
CREATE TABLE public.workbench_harness_defs (
    slug text NOT NULL,
    definition jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.workbench_jobs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    agent_id uuid NOT NULL,
    agent_model text NOT NULL,
    task_id uuid,
    repo text NOT NULL,
    branch text NOT NULL,
    effort text DEFAULT 'standard'::text NOT NULL,
    plan text DEFAULT ''::text NOT NULL,
    status text DEFAULT 'started'::text NOT NULL,
    pr_url text,
    summary text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    merged_testing_at timestamp with time zone
);
CREATE TABLE public.workbench_profiles (
    slug text NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    image text DEFAULT ''::text NOT NULL,
    env jsonb DEFAULT '{}'::jsonb NOT NULL,
    mounts jsonb DEFAULT '[]'::jsonb NOT NULL,
    harnesses jsonb DEFAULT '[]'::jsonb NOT NULL,
    auto_attach jsonb DEFAULT '{}'::jsonb NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.workbench_repo_flow (
    repo text NOT NULL,
    base_branch text,
    testing_branch text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.workbench_repo_requests (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    agent_id uuid NOT NULL,
    agent_model text NOT NULL,
    org text NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    why text DEFAULT ''::text NOT NULL,
    task_id uuid,
    status text DEFAULT 'pending'::text NOT NULL,
    decided_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.workbench_repos (
    agent_id uuid NOT NULL,
    repo text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.workspace_secret_entries (
    secret_id uuid NOT NULL,
    key text NOT NULL,
    label text NOT NULL,
    value_cipher text NOT NULL
);
CREATE TABLE public.workspace_secret_grants (
    secret_id uuid NOT NULL,
    agent_model text NOT NULL,
    granted_by text,
    granted_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.workspace_secret_readers (
    secret_id uuid NOT NULL,
    user_id uuid NOT NULL,
    granted_by text,
    granted_at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.workspace_secrets (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    title text NOT NULL,
    kind text DEFAULT 'vault'::text NOT NULL,
    note text,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone,
    uses_remaining integer,
    last_used_at timestamp with time zone,
    allowed_hosts text[],
    revealable boolean DEFAULT false NOT NULL,
    owner_user_id uuid,
    secret_folder_id uuid
);
ALTER TABLE ONLY public.agent_defs
    ADD CONSTRAINT agent_defs_model_key UNIQUE (model);
ALTER TABLE ONLY public.agent_defs
    ADD CONSTRAINT agent_defs_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.agent_defs
    ADD CONSTRAINT agent_defs_slug_key UNIQUE (slug);
ALTER TABLE ONLY public.agent_keys
    ADD CONSTRAINT agent_keys_key_hash_key UNIQUE (key_hash);
ALTER TABLE ONLY public.agent_keys
    ADD CONSTRAINT agent_keys_pkey PRIMARY KEY (agent_id);
ALTER TABLE ONLY public.agent_role_templates
    ADD CONSTRAINT agent_role_templates_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.agent_role_templates
    ADD CONSTRAINT agent_role_templates_slug_key UNIQUE (slug);
ALTER TABLE ONLY public.agent_secrets
    ADD CONSTRAINT agent_secrets_pkey PRIMARY KEY (agent_id, name);
ALTER TABLE ONLY public.agent_versions
    ADD CONSTRAINT agent_versions_agent_id_version_key UNIQUE (agent_id, version);
ALTER TABLE ONLY public.agent_versions
    ADD CONSTRAINT agent_versions_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.app_data
    ADD CONSTRAINT app_data_pkey PRIMARY KEY (app, collection, id);
ALTER TABLE ONLY public.app_settings
    ADD CONSTRAINT app_settings_pkey PRIMARY KEY (key);
ALTER TABLE ONLY public.artifact_folders
    ADD CONSTRAINT artifact_folders_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.artifact_links
    ADD CONSTRAINT artifact_links_pkey PRIMARY KEY (artifact_id, target_type, target_id);
ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_public_slug_key UNIQUE (public_slug);
ALTER TABLE ONLY public.assistant_reply_drafts
    ADD CONSTRAINT assistant_reply_drafts_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.assistant_reply_grants
    ADD CONSTRAINT assistant_reply_grants_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.audit_log
    ADD CONSTRAINT audit_log_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.board_agent_requests
    ADD CONSTRAINT board_agent_requests_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.board_agents
    ADD CONSTRAINT board_agents_pkey PRIMARY KEY (board_id, agent_model);
ALTER TABLE ONLY public.board_labels
    ADD CONSTRAINT board_labels_board_id_name_key UNIQUE (board_id, name);
ALTER TABLE ONLY public.board_labels
    ADD CONSTRAINT board_labels_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.board_members
    ADD CONSTRAINT board_members_pkey PRIMARY KEY (board_id, user_id);
ALTER TABLE ONLY public.board_statuses
    ADD CONSTRAINT board_statuses_board_id_key_key UNIQUE (board_id, key);
ALTER TABLE ONLY public.board_statuses
    ADD CONSTRAINT board_statuses_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.board_templates
    ADD CONSTRAINT board_templates_pkey PRIMARY KEY (board_id, template_id);
ALTER TABLE ONLY public.board_views
    ADD CONSTRAINT board_views_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.boards
    ADD CONSTRAINT boards_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.capability_gaps
    ADD CONSTRAINT capability_gaps_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.capability_gaps
    ADD CONSTRAINT capability_gaps_signature_key UNIQUE (signature);
ALTER TABLE ONLY public.channel_agents
    ADD CONSTRAINT channel_agents_pkey PRIMARY KEY (channel_id, agent_model);
ALTER TABLE ONLY public.channel_members
    ADD CONSTRAINT channel_members_pkey PRIMARY KEY (channel_id, user_id);
ALTER TABLE ONLY public.channel_message_reactions
    ADD CONSTRAINT channel_message_reactions_pkey PRIMARY KEY (message_id, emoji, actor);
ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT channel_messages_channel_id_seq_key UNIQUE (channel_id, seq);
ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT channel_messages_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.channels
    ADD CONSTRAINT channels_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.conversation_members
    ADD CONSTRAINT conversation_members_pkey PRIMARY KEY (conversation_id, user_id);
ALTER TABLE ONLY public.conversation_reads
    ADD CONSTRAINT conversation_reads_pkey PRIMARY KEY (conversation_id, user_id);
ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.daily_brief_entries
    ADD CONSTRAINT daily_brief_entries_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.daily_briefs
    ADD CONSTRAINT daily_briefs_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.daily_briefs
    ADD CONSTRAINT daily_briefs_user_id_brief_date_key UNIQUE (user_id, brief_date);
ALTER TABLE ONLY public.fitness_transcripts
    ADD CONSTRAINT fitness_transcripts_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.fleet_agents
    ADD CONSTRAINT fleet_agents_name_key UNIQUE (name);
ALTER TABLE ONLY public.fleet_agents
    ADD CONSTRAINT fleet_agents_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.google_connections
    ADD CONSTRAINT google_connections_pkey PRIMARY KEY (user_id);
ALTER TABLE ONLY public.google_org_connection
    ADD CONSTRAINT google_org_connection_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.google_pending_actions
    ADD CONSTRAINT google_pending_actions_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.guard_findings
    ADD CONSTRAINT guard_findings_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.harness_runs
    ADD CONSTRAINT harness_runs_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.inbox_decisions
    ADD CONSTRAINT inbox_decisions_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.inbox_focus_state
    ADD CONSTRAINT inbox_focus_state_pkey PRIMARY KEY (user_id, source_type, source_id);
ALTER TABLE ONLY public.internal_versions
    ADD CONSTRAINT internal_versions_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_token_key UNIQUE (token);
ALTER TABLE ONLY public.judge_reviews
    ADD CONSTRAINT judge_reviews_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.kb_comments
    ADD CONSTRAINT kb_comments_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.kb_docs
    ADD CONSTRAINT kb_docs_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.kb_docs
    ADD CONSTRAINT kb_docs_public_slug_key UNIQUE (public_slug);
ALTER TABLE ONLY public.kb_editors
    ADD CONSTRAINT kb_editors_pkey PRIMARY KEY (item_type, item_id, principal_type, principal_id);
ALTER TABLE ONLY public.kb_spaces
    ADD CONSTRAINT kb_spaces_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.kb_spaces
    ADD CONSTRAINT kb_spaces_public_slug_key UNIQUE (public_slug);
ALTER TABLE ONLY public.llm_api_keys
    ADD CONSTRAINT llm_api_keys_key_hash_key UNIQUE (key_hash);
ALTER TABLE ONLY public.llm_api_keys
    ADD CONSTRAINT llm_api_keys_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.llm_endpoints
    ADD CONSTRAINT llm_endpoints_name_key UNIQUE (name);
ALTER TABLE ONLY public.llm_endpoints
    ADD CONSTRAINT llm_endpoints_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.mcp_oauth_states
    ADD CONSTRAINT mcp_oauth_states_pkey PRIMARY KEY (state);
ALTER TABLE ONLY public.mcp_oauth_tokens
    ADD CONSTRAINT mcp_oauth_tokens_pkey PRIMARY KEY (server_id, subject);
ALTER TABLE ONLY public.mcp_server_agents
    ADD CONSTRAINT mcp_server_agents_pkey PRIMARY KEY (server_id, agent_model);
ALTER TABLE ONLY public.mcp_servers
    ADD CONSTRAINT mcp_servers_name_key UNIQUE (name);
ALTER TABLE ONLY public.mcp_servers
    ADD CONSTRAINT mcp_servers_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.mcp_user_access
    ADD CONSTRAINT mcp_user_access_pkey PRIMARY KEY (server_id, user_id);
ALTER TABLE ONLY public.mcp_user_credentials
    ADD CONSTRAINT mcp_user_credentials_pkey PRIMARY KEY (server_id, user_id);
ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_conversation_id_seq_key UNIQUE (conversation_id, seq);
ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.model_blurbs
    ADD CONSTRAINT model_blurbs_pkey PRIMARY KEY (model_id);
ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.org_domains
    ADD CONSTRAINT org_domains_domain_key UNIQUE (domain);
ALTER TABLE ONLY public.org_domains
    ADD CONSTRAINT org_domains_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.outreach_events
    ADD CONSTRAINT outreach_events_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.plan_drafts
    ADD CONSTRAINT plan_drafts_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.push_subscriptions
    ADD CONSTRAINT push_subscriptions_endpoint_key UNIQUE (endpoint);
ALTER TABLE ONLY public.push_subscriptions
    ADD CONSTRAINT push_subscriptions_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.quality_reviews
    ADD CONSTRAINT quality_reviews_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.rag_collection_access
    ADD CONSTRAINT rag_collection_access_collection_id_principal_type_principa_key UNIQUE (collection_id, principal_type, principal_id);
ALTER TABLE ONLY public.rag_collections
    ADD CONSTRAINT rag_collections_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.rag_collections
    ADD CONSTRAINT rag_collections_qdrant_name_key UNIQUE (qdrant_name);
ALTER TABLE ONLY public.rag_points
    ADD CONSTRAINT rag_points_collection_id_source_type_source_id_key UNIQUE (collection_id, source_type, source_id);
ALTER TABLE ONLY public.rag_points
    ADD CONSTRAINT rag_points_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.research_members
    ADD CONSTRAINT research_members_pkey PRIMARY KEY (run_id, user_id);
ALTER TABLE ONLY public.research_runs
    ADD CONSTRAINT research_runs_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_run_id_idx_key UNIQUE (run_id, idx);
ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.schema_migrations
    ADD CONSTRAINT schema_migrations_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.secret_folder_grants
    ADD CONSTRAINT secret_folder_grants_pkey PRIMARY KEY (folder_id, agent_model);
ALTER TABLE ONLY public.secret_folder_readers
    ADD CONSTRAINT secret_folder_readers_pkey PRIMARY KEY (folder_id, user_id);
ALTER TABLE ONLY public.secret_folders
    ADD CONSTRAINT secret_folders_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.secret_keys
    ADD CONSTRAINT secret_keys_pkey PRIMARY KEY (version);
ALTER TABLE ONLY public.skill_summaries
    ADD CONSTRAINT skill_summaries_pkey PRIMARY KEY (owner, name);
ALTER TABLE ONLY public.task_activity
    ADD CONSTRAINT task_activity_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT task_dependencies_pkey PRIMARY KEY (task_id, depends_on_id);
ALTER TABLE ONLY public.task_watchers
    ADD CONSTRAINT task_watchers_pkey PRIMARY KEY (task_id, watcher);
ALTER TABLE ONLY public.task_workflows
    ADD CONSTRAINT task_workflows_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.team_members
    ADD CONSTRAINT team_members_pkey PRIMARY KEY (team_id, user_id);
ALTER TABLE ONLY public.teams
    ADD CONSTRAINT teams_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.templates
    ADD CONSTRAINT templates_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.uploads
    ADD CONSTRAINT uploads_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.usage_events
    ADD CONSTRAINT usage_events_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.user_agent_access
    ADD CONSTRAINT user_agent_access_pkey PRIMARY KEY (user_id, agent_model);
ALTER TABLE ONLY public.user_password_credentials
    ADD CONSTRAINT user_password_credentials_email_key UNIQUE (email);
ALTER TABLE ONLY public.user_password_credentials
    ADD CONSTRAINT user_password_credentials_pkey PRIMARY KEY (user_id);
ALTER TABLE ONLY public.user_permissions
    ADD CONSTRAINT user_permissions_pkey PRIMARY KEY (user_id, perm);
ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_sub_key UNIQUE (sub);
ALTER TABLE ONLY public.workbench_harness_defs
    ADD CONSTRAINT workbench_harness_defs_pkey PRIMARY KEY (slug);
ALTER TABLE ONLY public.workbench_jobs
    ADD CONSTRAINT workbench_jobs_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.workbench_profiles
    ADD CONSTRAINT workbench_profiles_pkey PRIMARY KEY (slug);
ALTER TABLE ONLY public.workbench_repo_flow
    ADD CONSTRAINT workbench_repo_flow_pkey PRIMARY KEY (repo);
ALTER TABLE ONLY public.workbench_repo_requests
    ADD CONSTRAINT workbench_repo_requests_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.workbench_repos
    ADD CONSTRAINT workbench_repos_pkey PRIMARY KEY (agent_id, repo);
ALTER TABLE ONLY public.workspace_secret_entries
    ADD CONSTRAINT workspace_secret_entries_pkey PRIMARY KEY (secret_id, key);
ALTER TABLE ONLY public.workspace_secret_grants
    ADD CONSTRAINT workspace_secret_grants_pkey PRIMARY KEY (secret_id, agent_model);
ALTER TABLE ONLY public.workspace_secret_readers
    ADD CONSTRAINT workspace_secret_readers_pkey PRIMARY KEY (secret_id, user_id);
ALTER TABLE ONLY public.workspace_secrets
    ADD CONSTRAINT workspace_secrets_name_key UNIQUE (name);
ALTER TABLE ONLY public.workspace_secrets
    ADD CONSTRAINT workspace_secrets_pkey PRIMARY KEY (id);
CREATE INDEX app_data_updated_idx ON public.app_data USING btree (app, collection, updated_at DESC);
CREATE INDEX artifact_folders_owner_idx ON public.artifact_folders USING btree (owner_user_id);
CREATE INDEX artifact_links_target_idx ON public.artifact_links USING btree (target_type, target_id);
CREATE INDEX artifacts_folder_idx ON public.artifacts USING btree (folder_id);
CREATE INDEX artifacts_owner_idx ON public.artifacts USING btree (owner_user_id);
CREATE INDEX assistant_reply_drafts_open_idx ON public.assistant_reply_drafts USING btree (user_id, channel_id) WHERE (status = 'pending'::text);
CREATE UNIQUE INDEX assistant_reply_grants_standing_idx ON public.assistant_reply_grants USING btree (user_id) WHERE ((channel_id IS NULL) AND (revoked_at IS NULL));
CREATE UNIQUE INDEX assistant_reply_grants_thread_idx ON public.assistant_reply_grants USING btree (user_id, channel_id) WHERE ((channel_id IS NOT NULL) AND (revoked_at IS NULL));
CREATE INDEX audit_log_created_idx ON public.audit_log USING btree (created_at DESC);
CREATE INDEX audit_log_target_idx ON public.audit_log USING btree (target_type, target_id);
CREATE UNIQUE INDEX board_agent_requests_one_open ON public.board_agent_requests USING btree (board_id, agent_model) WHERE (status = 'open'::text);
CREATE INDEX board_views_board_idx ON public.board_views USING btree (board_id, "position");
CREATE INDEX channel_messages_idx ON public.channel_messages USING btree (channel_id, seq);
CREATE INDEX channel_messages_streaming_idx ON public.channel_messages USING btree (created_at) WHERE (status = 'streaming'::text);
CREATE INDEX channel_messages_thread_idx ON public.channel_messages USING btree (thread_root_id) WHERE (thread_root_id IS NOT NULL);
CREATE UNIQUE INDEX channels_dm_key_idx ON public.channels USING btree (dm_key) WHERE (dm_key IS NOT NULL);
CREATE INDEX conversations_user_agent_idx ON public.conversations USING btree (user_id, agent_model, updated_at DESC);
CREATE INDEX daily_brief_entries_batch_idx ON public.daily_brief_entries USING btree (brief_id, batch);
CREATE INDEX daily_brief_entries_key_idx ON public.daily_brief_entries USING btree (brief_id, source_key);
CREATE UNIQUE INDEX daily_brief_entries_seq_idx ON public.daily_brief_entries USING btree (brief_id, seq);
CREATE INDEX daily_briefs_user_idx ON public.daily_briefs USING btree (user_id, brief_date DESC);
CREATE INDEX fitness_transcripts_model_run_idx ON public.fitness_transcripts USING btree (model, run_started_at DESC, harness, case_name);
CREATE INDEX google_pending_org_idx ON public.google_pending_actions USING btree (is_org, status);
CREATE INDEX google_pending_owner_idx ON public.google_pending_actions USING btree (owner_user_id, status);
CREATE INDEX guard_findings_recent_idx ON public.guard_findings USING btree (created_at DESC);
CREATE INDEX harness_runs_harness_model_idx ON public.harness_runs USING btree (harness, model, created_at DESC);
CREATE INDEX inbox_decisions_confirmation_idx ON public.inbox_decisions USING btree (user_id, confirmation_token_hash) WHERE (status = 'proposed'::text);
CREATE INDEX inbox_decisions_conversation_timeline_idx ON public.inbox_decisions USING btree (conversation_id, created_at DESC, id DESC) WHERE (conversation_id IS NOT NULL);
CREATE INDEX inbox_decisions_user_idx ON public.inbox_decisions USING btree (user_id, created_at DESC);
CREATE INDEX inbox_focus_state_snooze_idx ON public.inbox_focus_state USING btree (user_id, snoozed_until);
CREATE INDEX internal_versions_idx ON public.internal_versions USING btree (kind, owner_key, created_at DESC);
CREATE INDEX invites_email_idx ON public.invites USING btree (email);
CREATE INDEX judge_reviews_task_idx ON public.judge_reviews USING btree (task_id, created_at DESC);
CREATE INDEX kb_comments_doc_idx ON public.kb_comments USING btree (doc_id, created_at);
CREATE INDEX kb_docs_fts_idx ON public.kb_docs USING gin (to_tsvector('english'::regconfig, ((COALESCE(title, ''::text) || ' '::text) || COALESCE(body, ''::text))));
CREATE INDEX kb_docs_space_idx ON public.kb_docs USING btree (space_id, parent_id, sort);
CREATE INDEX kb_editors_item_idx ON public.kb_editors USING btree (item_type, item_id);
CREATE INDEX messages_conv_idx ON public.messages USING btree (conversation_id, seq);
CREATE INDEX messages_conversation_timeline_idx ON public.messages USING btree (conversation_id, created_at DESC, id DESC);
CREATE INDEX messages_streaming_idx ON public.messages USING btree (created_at) WHERE (status = 'streaming'::text);
CREATE INDEX notifications_unread_idx ON public.notifications USING btree (user_id) WHERE (read_at IS NULL);
CREATE INDEX notifications_user_idx ON public.notifications USING btree (user_id, created_at DESC);
CREATE INDEX outreach_events_agent_idx ON public.outreach_events USING btree (agent_model, kind, created_at DESC);
CREATE INDEX plan_drafts_conversation_idx ON public.plan_drafts USING btree (conversation_id, created_at DESC);
CREATE INDEX push_subscriptions_user_idx ON public.push_subscriptions USING btree (user_id);
CREATE INDEX research_runs_conversation_idx ON public.research_runs USING btree (conversation_id);
CREATE INDEX research_runs_created_idx ON public.research_runs USING btree (created_at DESC);
CREATE INDEX research_runs_parent_idx ON public.research_runs USING btree (parent_run_id);
CREATE UNIQUE INDEX runs_approval_key_idx ON public.runs USING btree (approval_key) WHERE (approval_key IS NOT NULL);
CREATE INDEX runs_owner_active_idx ON public.runs USING btree (owner_user_id, state, updated_at DESC) WHERE (state = ANY (ARRAY['queued'::text, 'running'::text, 'awaiting'::text]));
CREATE INDEX runs_reclaim_idx ON public.runs USING btree (lease_expires_at NULLS FIRST, created_at) WHERE (state = ANY (ARRAY['queued'::text, 'running'::text]));
CREATE INDEX task_activity_task_idx ON public.task_activity USING btree (task_id, created_at DESC);
CREATE INDEX tasks_assignee_idx ON public.tasks USING btree (assigned_to);
CREATE INDEX tasks_board_idx ON public.tasks USING btree (board_id, status, updated_at DESC);
CREATE INDEX tasks_conversation_idx ON public.tasks USING btree (conversation_id);
CREATE INDEX tasks_parent_idx ON public.tasks USING btree (parent_id);
CREATE INDEX usage_events_agent_idx ON public.usage_events USING btree (agent_model, created_at DESC);
CREATE INDEX usage_events_created_idx ON public.usage_events USING btree (created_at DESC);
CREATE INDEX usage_events_task_idx ON public.usage_events USING btree (task_id) WHERE (task_id IS NOT NULL);
CREATE INDEX workspace_secrets_secret_folder_idx ON public.workspace_secrets USING btree (secret_folder_id);
ALTER TABLE ONLY public.agent_defs
    ADD CONSTRAINT agent_defs_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.agent_defs
    ADD CONSTRAINT agent_defs_plan_template_id_fkey FOREIGN KEY (plan_template_id) REFERENCES public.templates(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.agent_defs
    ADD CONSTRAINT agent_defs_ticket_template_id_fkey FOREIGN KEY (ticket_template_id) REFERENCES public.templates(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.agent_keys
    ADD CONSTRAINT agent_keys_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agent_defs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.agent_secrets
    ADD CONSTRAINT agent_secrets_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agent_defs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.agent_versions
    ADD CONSTRAINT agent_versions_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agent_defs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.artifact_folders
    ADD CONSTRAINT artifact_folders_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.artifact_folders
    ADD CONSTRAINT artifact_folders_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.artifact_folders(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.artifact_links
    ADD CONSTRAINT artifact_links_artifact_id_fkey FOREIGN KEY (artifact_id) REFERENCES public.artifacts(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_folder_id_fkey FOREIGN KEY (folder_id) REFERENCES public.artifact_folders(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_kb_doc_id_fkey FOREIGN KEY (kb_doc_id) REFERENCES public.kb_docs(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.assistant_reply_drafts
    ADD CONSTRAINT assistant_reply_drafts_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.assistant_reply_drafts
    ADD CONSTRAINT assistant_reply_drafts_decided_by_fkey FOREIGN KEY (decided_by) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.assistant_reply_drafts
    ADD CONSTRAINT assistant_reply_drafts_message_id_fkey FOREIGN KEY (message_id) REFERENCES public.channel_messages(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.assistant_reply_drafts
    ADD CONSTRAINT assistant_reply_drafts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.assistant_reply_grants
    ADD CONSTRAINT assistant_reply_grants_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.assistant_reply_grants
    ADD CONSTRAINT assistant_reply_grants_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.board_agent_requests
    ADD CONSTRAINT board_agent_requests_board_id_fkey FOREIGN KEY (board_id) REFERENCES public.boards(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.board_agent_requests
    ADD CONSTRAINT board_agent_requests_decided_by_fkey FOREIGN KEY (decided_by) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.board_agent_requests
    ADD CONSTRAINT board_agent_requests_requested_by_user_id_fkey FOREIGN KEY (requested_by_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.board_agents
    ADD CONSTRAINT board_agents_board_id_fkey FOREIGN KEY (board_id) REFERENCES public.boards(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.board_labels
    ADD CONSTRAINT board_labels_board_id_fkey FOREIGN KEY (board_id) REFERENCES public.boards(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.board_members
    ADD CONSTRAINT board_members_board_id_fkey FOREIGN KEY (board_id) REFERENCES public.boards(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.board_members
    ADD CONSTRAINT board_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.board_statuses
    ADD CONSTRAINT board_statuses_board_id_fkey FOREIGN KEY (board_id) REFERENCES public.boards(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.board_templates
    ADD CONSTRAINT board_templates_board_id_fkey FOREIGN KEY (board_id) REFERENCES public.boards(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.board_templates
    ADD CONSTRAINT board_templates_template_id_fkey FOREIGN KEY (template_id) REFERENCES public.templates(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.board_views
    ADD CONSTRAINT board_views_board_id_fkey FOREIGN KEY (board_id) REFERENCES public.boards(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.boards
    ADD CONSTRAINT boards_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.boards
    ADD CONSTRAINT boards_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.teams(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.capability_gaps
    ADD CONSTRAINT capability_gaps_board_id_fkey FOREIGN KEY (board_id) REFERENCES public.boards(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.capability_gaps
    ADD CONSTRAINT capability_gaps_example_task_id_fkey FOREIGN KEY (example_task_id) REFERENCES public.tasks(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.channel_agents
    ADD CONSTRAINT channel_agents_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.channel_members
    ADD CONSTRAINT channel_members_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.channel_members
    ADD CONSTRAINT channel_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.channel_message_reactions
    ADD CONSTRAINT channel_message_reactions_message_id_fkey FOREIGN KEY (message_id) REFERENCES public.channel_messages(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT channel_messages_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT channel_messages_thread_root_id_fkey FOREIGN KEY (thread_root_id) REFERENCES public.channel_messages(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.channels
    ADD CONSTRAINT channels_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.conversation_members
    ADD CONSTRAINT conversation_members_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.conversation_members
    ADD CONSTRAINT conversation_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.conversation_reads
    ADD CONSTRAINT conversation_reads_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.conversation_reads
    ADD CONSTRAINT conversation_reads_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_plan_template_id_fkey FOREIGN KEY (plan_template_id) REFERENCES public.templates(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.daily_brief_entries
    ADD CONSTRAINT daily_brief_entries_brief_id_fkey FOREIGN KEY (brief_id) REFERENCES public.daily_briefs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.daily_brief_entries
    ADD CONSTRAINT daily_brief_entries_supersedes_fkey FOREIGN KEY (supersedes) REFERENCES public.daily_brief_entries(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.daily_briefs
    ADD CONSTRAINT daily_briefs_artifact_id_fkey FOREIGN KEY (artifact_id) REFERENCES public.artifacts(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.daily_briefs
    ADD CONSTRAINT daily_briefs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.google_connections
    ADD CONSTRAINT google_connections_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.google_org_connection
    ADD CONSTRAINT google_org_connection_connected_by_fkey FOREIGN KEY (connected_by) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.google_pending_actions
    ADD CONSTRAINT google_pending_actions_decided_by_fkey FOREIGN KEY (decided_by) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.google_pending_actions
    ADD CONSTRAINT google_pending_actions_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.inbox_decisions
    ADD CONSTRAINT inbox_decisions_assistant_message_id_fkey FOREIGN KEY (assistant_message_id) REFERENCES public.messages(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.inbox_decisions
    ADD CONSTRAINT inbox_decisions_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.inbox_decisions
    ADD CONSTRAINT inbox_decisions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.inbox_decisions
    ADD CONSTRAINT inbox_decisions_user_message_id_fkey FOREIGN KEY (user_message_id) REFERENCES public.messages(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.inbox_focus_state
    ADD CONSTRAINT inbox_focus_state_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_accepted_user_id_fkey FOREIGN KEY (accepted_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.judge_reviews
    ADD CONSTRAINT judge_reviews_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.kb_comments
    ADD CONSTRAINT kb_comments_author_user_id_fkey FOREIGN KEY (author_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.kb_comments
    ADD CONSTRAINT kb_comments_doc_id_fkey FOREIGN KEY (doc_id) REFERENCES public.kb_docs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.kb_comments
    ADD CONSTRAINT kb_comments_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.kb_comments(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.kb_docs
    ADD CONSTRAINT kb_docs_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.kb_docs
    ADD CONSTRAINT kb_docs_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.kb_docs(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.kb_docs
    ADD CONSTRAINT kb_docs_space_id_fkey FOREIGN KEY (space_id) REFERENCES public.kb_spaces(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.kb_spaces
    ADD CONSTRAINT kb_spaces_okf_doc_id_fkey FOREIGN KEY (okf_doc_id) REFERENCES public.kb_docs(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.kb_spaces
    ADD CONSTRAINT kb_spaces_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.kb_spaces
    ADD CONSTRAINT kb_spaces_rag_collection_id_fkey FOREIGN KEY (rag_collection_id) REFERENCES public.rag_collections(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.llm_api_keys
    ADD CONSTRAINT llm_api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.mcp_oauth_states
    ADD CONSTRAINT mcp_oauth_states_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.mcp_servers(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.mcp_oauth_tokens
    ADD CONSTRAINT mcp_oauth_tokens_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.mcp_servers(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.mcp_server_agents
    ADD CONSTRAINT mcp_server_agents_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.mcp_servers(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.mcp_user_access
    ADD CONSTRAINT mcp_user_access_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.mcp_servers(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.mcp_user_access
    ADD CONSTRAINT mcp_user_access_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.mcp_user_credentials
    ADD CONSTRAINT mcp_user_credentials_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.mcp_servers(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.mcp_user_credentials
    ADD CONSTRAINT mcp_user_credentials_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_author_user_id_fkey FOREIGN KEY (author_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.outreach_events
    ADD CONSTRAINT outreach_events_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.outreach_events
    ADD CONSTRAINT outreach_events_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.plan_drafts
    ADD CONSTRAINT plan_drafts_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.push_subscriptions
    ADD CONSTRAINT push_subscriptions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.quality_reviews
    ADD CONSTRAINT quality_reviews_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.rag_collection_access
    ADD CONSTRAINT rag_collection_access_collection_id_fkey FOREIGN KEY (collection_id) REFERENCES public.rag_collections(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.rag_collections
    ADD CONSTRAINT rag_collections_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.rag_points
    ADD CONSTRAINT rag_points_collection_id_fkey FOREIGN KEY (collection_id) REFERENCES public.rag_collections(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.research_members
    ADD CONSTRAINT research_members_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.research_runs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.research_members
    ADD CONSTRAINT research_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.research_runs
    ADD CONSTRAINT research_runs_artifact_id_fkey FOREIGN KEY (artifact_id) REFERENCES public.artifacts(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.research_runs
    ADD CONSTRAINT research_runs_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.research_runs
    ADD CONSTRAINT research_runs_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.research_runs
    ADD CONSTRAINT research_runs_parent_run_id_fkey FOREIGN KEY (parent_run_id) REFERENCES public.research_runs(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.research_runs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.secret_folder_grants
    ADD CONSTRAINT secret_folder_grants_folder_id_fkey FOREIGN KEY (folder_id) REFERENCES public.secret_folders(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.secret_folder_readers
    ADD CONSTRAINT secret_folder_readers_folder_id_fkey FOREIGN KEY (folder_id) REFERENCES public.secret_folders(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.secret_folder_readers
    ADD CONSTRAINT secret_folder_readers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.secret_folders
    ADD CONSTRAINT secret_folders_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.task_activity
    ADD CONSTRAINT task_activity_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT task_dependencies_depends_on_id_fkey FOREIGN KEY (depends_on_id) REFERENCES public.tasks(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT task_dependencies_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.task_watchers
    ADD CONSTRAINT task_watchers_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_board_id_fkey FOREIGN KEY (board_id) REFERENCES public.boards(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.tasks(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.team_members
    ADD CONSTRAINT team_members_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.teams(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.team_members
    ADD CONSTRAINT team_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.teams
    ADD CONSTRAINT teams_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.uploads
    ADD CONSTRAINT uploads_uploaded_by_fkey FOREIGN KEY (uploaded_by) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.user_agent_access
    ADD CONSTRAINT user_agent_access_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.user_password_credentials
    ADD CONSTRAINT user_password_credentials_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.user_permissions
    ADD CONSTRAINT user_permissions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.workbench_jobs
    ADD CONSTRAINT workbench_jobs_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agent_defs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.workbench_jobs
    ADD CONSTRAINT workbench_jobs_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.workbench_repo_requests
    ADD CONSTRAINT workbench_repo_requests_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agent_defs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.workbench_repo_requests
    ADD CONSTRAINT workbench_repo_requests_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.workbench_repos
    ADD CONSTRAINT workbench_repos_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agent_defs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.workspace_secret_entries
    ADD CONSTRAINT workspace_secret_entries_secret_id_fkey FOREIGN KEY (secret_id) REFERENCES public.workspace_secrets(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.workspace_secret_grants
    ADD CONSTRAINT workspace_secret_grants_secret_id_fkey FOREIGN KEY (secret_id) REFERENCES public.workspace_secrets(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.workspace_secret_readers
    ADD CONSTRAINT workspace_secret_readers_secret_id_fkey FOREIGN KEY (secret_id) REFERENCES public.workspace_secrets(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.workspace_secret_readers
    ADD CONSTRAINT workspace_secret_readers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.workspace_secrets
    ADD CONSTRAINT workspace_secrets_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.workspace_secrets
    ADD CONSTRAINT workspace_secrets_secret_folder_id_fkey FOREIGN KEY (secret_folder_id) REFERENCES public.secret_folders(id) ON DELETE SET NULL;
