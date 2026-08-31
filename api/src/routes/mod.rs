// Route handlers, and THE router — built here (not in main.rs) so integration
// tests drive the exact same stack the process serves.

pub mod activity;
pub mod admin_google_client;
pub mod admin_instance;
pub mod admin_model_roles;
pub mod admin_password_accounts;
pub mod admin_permissions;
pub mod admin_rag;
pub mod agent_role_templates;
pub mod agents;
pub mod apps;
pub mod artifact_folders;
pub mod artifact_folders_id;
pub mod artifacts;
pub mod artifacts_for;
pub mod artifacts_id;
pub mod artifacts_id_export_google;
pub mod artifacts_id_links;
pub mod artifacts_public_slug;
pub mod artifacts_public_slug_download;
pub mod auth_claim;
pub mod auth_google;
pub mod auth_google_callback;
pub mod auth_logout;
pub mod auth_password;
pub mod auth_providers;
pub mod auth_session;
pub mod boards;
pub mod boards_id;
pub mod boards_id_agents;
pub mod boards_id_events;
pub mod boards_id_labels;
pub mod boards_id_members;
pub mod boards_id_statuses;
pub mod boards_id_tasks;
pub mod boards_id_templates;
pub mod boards_id_views;
pub mod brief;
pub mod brief_delegate;
pub mod brief_item;
pub mod brief_read;
pub mod brief_reply;
pub mod channels;
pub mod channels_id;
pub mod channels_id_agents;
pub mod channels_id_conclude;
pub mod channels_id_events;
pub mod channels_id_members;
pub mod channels_id_messages;
pub mod channels_id_messages_msgid;
pub mod channels_id_messages_msgid_reactions;
pub mod channels_id_plan;
pub mod channels_id_read;
pub mod chat;
pub mod conversations;
pub mod conversations_id;
pub mod cost;
pub mod dms;
pub mod fleet_create;
pub mod fleet_defs_id_mcp;
pub mod fleet_hires;
pub mod health;
pub mod history;
pub mod inbox_focus;
pub mod inbox_focus_actions;
pub mod inbox_focus_command;
pub mod inbox_focus_conversations;
pub mod inbox_focus_conversations_id;
pub mod inbox_focus_state;
pub mod inbox_focus_summary;
pub mod integrations_google;
pub mod integrations_google_agent_calendar;
pub mod integrations_google_agent_drive;
pub mod integrations_google_agent_gmail;
pub mod integrations_google_agent_gmail_id;
pub mod integrations_google_agent_gmail_labels;
pub mod integrations_google_agent_gmail_organize;
pub mod integrations_google_calendar_events;
pub mod integrations_google_callback;
pub mod integrations_google_connect;
pub mod integrations_google_drive_files;
pub mod integrations_google_drive_import;
pub mod integrations_google_gmail_messages;
pub mod integrations_google_gmail_send;
pub mod integrations_google_org;
pub mod integrations_google_org_callback;
pub mod integrations_google_org_connect;
pub mod integrations_google_org_health;
pub mod integrations_google_org_provision;
pub mod integrations_google_pending;
pub mod integrations_google_pending_id;
pub mod kb_comments_id;
pub mod kb_docs_id;
pub mod kb_docs_id_backlinks;
pub mod kb_docs_id_comments;
pub mod kb_docs_id_live;
pub mod kb_docs_id_move;
pub mod kb_public;
pub mod kb_public_space;
pub mod kb_search;
pub mod kb_spaces;
pub mod kb_spaces_id;
pub mod kb_spaces_id_docs;
pub mod keys;
pub mod keys_id;
pub mod llm_chat;
pub mod llm_models;
pub mod mcp;
pub mod mcp_gw_server;
pub mod mcp_icon;
pub mod mcp_library;
pub mod mcp_oauth_callback;
pub mod mcp_oauth_start;
pub mod mcp_servers;
pub mod mcp_servers_id;
pub mod mcp_test;
pub mod me;
pub mod me_events;
pub mod me_mcp;
pub mod models;
pub mod models_efforts;
pub mod notifications;
pub mod plans_id_draft;
pub mod rag_collections;
pub mod rag_collections_id;
pub mod rag_search;
pub mod research;
pub mod research_id;
pub mod research_id_conversation;
pub mod research_id_decide;
pub mod research_id_members;
pub mod runs_events;
pub mod secrets;
pub mod secrets_folders;
pub mod secrets_git_credential;
pub mod secrets_relay;
pub mod secrets_reveal;
pub mod secrets_share;
pub mod tasks_id;
pub mod tasks_id_comments;
pub mod tasks_id_dependencies;
pub mod tasks_id_review;
pub mod tasks_id_usage;
pub mod tasks_id_watchers;
pub mod teams;
pub mod teams_id;
pub mod teams_id_members;
pub mod uploads;
pub mod uploads_id;
pub mod users;
pub mod workbench;
pub mod workbench_flow;
pub mod workbench_github;
pub mod workbench_harnesses;
pub mod workbench_jobs;
pub mod workbench_repo_requests;
pub mod workbench_repos_agent_id;
pub mod workflows;
pub mod workflows_id;

use crate::state::AppState;
use axum::Router;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use std::time::Duration;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// app.ts's 405: the body is fixed, and the allow header is the TS route
/// file's handler keys in DECLARATION order (Object.keys of the handlers
/// object) — the order is pinned, not alphabetical.
fn method_not_allowed(allow: &'static str) -> Response {
    let mut res = crate::error::house_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    res.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static(allow));
    res
}

/// app.ts's API 404: `/api` and everything under `/api/` answer the JSON
/// sentence; anything else is not this server's to describe (the SPA shell is
/// a TS-side concern until cutover).
async fn api_not_found(uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        return crate::error::house_error(StatusCode::NOT_FOUND, "not found");
    }
    StatusCode::NOT_FOUND.into_response()
}

pub fn router(state: AppState) -> Router {
    // Two stacks, one reason: some routes STREAM. chat/completions' legitimate
    // lifetime is bounded by the upstream's own 10-minute budget
    // (UPSTREAM_TIMEOUT_MS), and the SSE event streams' by the client's
    // attention — neither by a handler timeout. They must NOT sit under the
    // 30s TimeoutLayer the request/response routes use — widening that layer
    // for everyone would be the wrong trade.
    let timed = Router::new()
        .route(
            "/api/healthz",
            get(health::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/llm/v1/models",
            get(llm_models::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/session",
            get(auth_session::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/logout",
            post(auth_logout::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/password",
            post(auth_password::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/providers",
            get(auth_providers::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/claim",
            post(auth_claim::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/google",
            get(auth_google::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/google/callback",
            get(auth_google_callback::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/users",
            get(users::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/agents",
            get(agents::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/apps",
            get(apps::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/activity",
            get(activity::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/models",
            get(models::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/models/efforts",
            get(models_efforts::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/cost",
            get(cost::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/keys",
            get(keys::get)
                .post(keys::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards",
            get(boards::get)
                .post(boards::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards/{id}",
            axum::routing::patch(boards_id::patch)
                .delete(boards_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/boards/{id}/members",
            get(boards_id_members::get)
                .post(boards_id_members::post)
                .delete(boards_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/boards/{id}/labels",
            get(boards_id_labels::get)
                .post(boards_id_labels::post)
                .put(boards_id_labels::put)
                .delete(boards_id_labels::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/boards/{id}/statuses",
            get(boards_id_statuses::get)
                .post(boards_id_statuses::post)
                .put(boards_id_statuses::put)
                .delete(boards_id_statuses::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/boards/{id}/tasks",
            get(boards_id_tasks::get)
                .post(boards_id_tasks::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards/{id}/agents",
            get(boards_id_agents::get)
                .put(boards_id_agents::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/boards/{id}/templates",
            get(boards_id_templates::get)
                .put(boards_id_templates::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/boards/{id}/views",
            get(boards_id_views::get)
                .post(boards_id_views::post)
                .put(boards_id_views::put)
                .delete(boards_id_views::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/boards/{id}/events",
            get(boards_id_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/tasks/{id}",
            get(tasks_id::get)
                .put(tasks_id::put)
                .delete(tasks_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/tasks/{id}/comments",
            get(tasks_id_comments::get)
                .post(tasks_id_comments::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/tasks/{id}/dependencies",
            axum::routing::post(tasks_id_dependencies::post)
                .delete(tasks_id_dependencies::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/tasks/{id}/review",
            axum::routing::post(tasks_id_review::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/tasks/{id}/usage",
            get(tasks_id_usage::get)
                .post(tasks_id_usage::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/tasks/{id}/watchers",
            axum::routing::post(tasks_id_watchers::post)
                .delete(tasks_id_watchers::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/keys/{id}",
            axum::routing::delete(keys_id::delete)
                .put(keys_id::put)
                .fallback(|| async { method_not_allowed("DELETE, PUT") }),
        )
        .route(
            "/api/teams",
            get(teams::get)
                .post(teams::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/teams/{id}",
            axum::routing::patch(teams_id::patch)
                .delete(teams_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/teams/{id}/members",
            get(teams_id_members::get)
                .post(teams_id_members::post)
                .delete(teams_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/workflows",
            get(workflows::get)
                .post(workflows::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/notifications",
            get(notifications::get)
                .put(notifications::put)
                .patch(notifications::patch)
                .fallback(|| async { method_not_allowed("GET, PUT, PATCH") }),
        )
        .route(
            "/api/history",
            get(history::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The comms plane: channels/relays/DMs and their messages (with edit,
        // delete, reactions, threads, read cursors), membership, fleet-agent
        // membership, and the Relay conclude. The SSE attach point
        // ({id}/events) rides the streaming stack below, like every stream.
        .route(
            "/api/channels",
            get(channels::get)
                .post(channels::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/channels/{id}",
            get(channels_id::get)
                .put(channels_id::put)
                .delete(channels_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/channels/{id}/agents",
            post(channels_id_agents::post)
                .delete(channels_id_agents::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/channels/{id}/conclude",
            post(channels_id_conclude::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/channels/{id}/members",
            post(channels_id_members::post)
                .delete(channels_id_members::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/channels/{id}/messages",
            get(channels_id_messages::get)
                .post(channels_id_messages::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/channels/{id}/messages/{msgId}",
            axum::routing::patch(channels_id_messages_msgid::patch)
                .delete(channels_id_messages_msgid::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/channels/{id}/messages/{msgId}/reactions",
            post(channels_id_messages_msgid_reactions::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/channels/{id}/read",
            post(channels_id_read::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/channels/{id}/plan",
            get(channels_id_plan::get)
                .post(channels_id_plan::post)
                .patch(channels_id_plan::patch)
                .delete(channels_id_plan::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH, DELETE") }),
        )
        .route(
            "/api/chat",
            post(chat::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/conversations",
            get(conversations::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/conversations/{id}",
            get(conversations_id::get)
                .patch(conversations_id::patch)
                .fallback(|| async { method_not_allowed("GET, PATCH") }),
        )
        .route(
            "/api/dms",
            post(dms::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The Inbox focus family (queue, summary, state, actions, the
        // segmented conversations). The SSE command route lives in the
        // streaming stack below.
        .route(
            "/api/inbox/focus",
            get(inbox_focus::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/inbox/focus/summary",
            get(inbox_focus_summary::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/inbox/focus/state",
            put(inbox_focus_state::put).fallback(|| async { method_not_allowed("PUT") }),
        )
        .route(
            "/api/inbox/focus/actions",
            post(inbox_focus_actions::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/inbox/focus/conversations",
            get(inbox_focus_conversations::get)
                .post(inbox_focus_conversations::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/inbox/focus/conversations/{id}",
            get(inbox_focus_conversations_id::get)
                .delete(inbox_focus_conversations_id::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        // The brief family — the assistant's morning document, its read
        // cursor, the owner's verdict on a line, and the delegation trio.
        .route(
            "/api/brief",
            get(brief::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/brief/delegate",
            get(brief_delegate::get)
                .post(brief_delegate::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/brief/item",
            post(brief_item::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/brief/read",
            post(brief_read::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/brief/reply",
            post(brief_reply::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The secrets family — the sealed vault's six surfaces: the working
        // secrets a person saves and reads back, their folders, the one
        // reveal verb, sharing, the one-shot relay, and git's own way in.
        .route(
            "/api/secrets",
            get(secrets::get)
                .post(secrets::post)
                .patch(secrets::patch)
                .delete(secrets::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH, DELETE") }),
        )
        .route(
            "/api/secrets/folders",
            get(secrets_folders::get)
                .post(secrets_folders::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/secrets/git-credential",
            post(secrets_git_credential::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/relay",
            post(secrets_relay::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/reveal",
            post(secrets_reveal::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/share",
            post(secrets_share::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The integrations/google family — the personal AND org Google planes:
        // connect/callback pairs, the org targets + provisioning + health, the
        // pending-action approval queue, and the per-surface reads/mutations
        // (calendar, drive, gmail) in both flavors — as the signed-in user,
        // and as the agent acting for its owner or the org.
        .route(
            "/api/integrations/google",
            get(integrations_google::get)
                .delete(integrations_google::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        .route(
            "/api/integrations/google/connect",
            get(integrations_google_connect::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/callback",
            get(integrations_google_callback::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org",
            get(integrations_google_org::get)
                .put(integrations_google_org::put)
                .delete(integrations_google_org::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/integrations/google/org/connect",
            get(integrations_google_org_connect::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org/callback",
            get(integrations_google_org_callback::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org/health",
            get(integrations_google_org_health::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org/provision",
            get(integrations_google_org_provision::get)
                .post(integrations_google_org_provision::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/pending",
            get(integrations_google_pending::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/pending/{id}",
            post(integrations_google_pending_id::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/calendar/events",
            get(integrations_google_calendar_events::get)
                .post(integrations_google_calendar_events::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/drive/files",
            get(integrations_google_drive_files::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/drive/import",
            post(integrations_google_drive_import::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/gmail/messages",
            get(integrations_google_gmail_messages::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/gmail/send",
            post(integrations_google_gmail_send::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/agent/calendar",
            get(integrations_google_agent_calendar::get)
                .post(integrations_google_agent_calendar::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/agent/drive",
            get(integrations_google_agent_drive::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/agent/gmail",
            get(integrations_google_agent_gmail::get)
                .post(integrations_google_agent_gmail::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/agent/gmail/labels",
            get(integrations_google_agent_gmail_labels::get)
                .post(integrations_google_agent_gmail_labels::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/agent/gmail/organize",
            post(integrations_google_agent_gmail_organize::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/agent/gmail/{id}",
            get(integrations_google_agent_gmail_id::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/plans/{id}/draft",
            get(plans_id_draft::get)
                .post(plans_id_draft::post)
                .patch(plans_id_draft::patch)
                .delete(plans_id_draft::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH, DELETE") }),
        )
        .route(
            "/api/research",
            get(research::get)
                .post(research::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/research/{id}",
            get(research_id::get)
                .delete(research_id::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        .route(
            "/api/research/{id}/members",
            get(research_id_members::get)
                .post(research_id_members::post)
                .delete(research_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/research/{id}/conversation",
            post(research_id_conversation::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/research/{id}/decide",
            post(research_id_decide::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/me",
            get(me::get)
                .put(me::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workflows/{id}",
            put(workflows_id::put)
                .delete(workflows_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/agent-role-templates",
            get(agent_role_templates::get)
                .put(agent_role_templates::put)
                .delete(agent_role_templates::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/admin/password-accounts",
            get(admin_password_accounts::get)
                .post(admin_password_accounts::post)
                .put(admin_password_accounts::put)
                .delete(admin_password_accounts::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/admin/google-client",
            get(admin_google_client::get)
                .put(admin_google_client::put)
                .delete(admin_google_client::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/admin/google-client/login",
            put(admin_google_client::put_login).fallback(|| async { method_not_allowed("PUT") }),
        )
        .route(
            "/api/admin/instance",
            get(admin_instance::get)
                .put(admin_instance::put)
                .post(admin_instance::post)
                .fallback(|| async { method_not_allowed("GET, PUT, POST") }),
        )
        .route(
            "/api/admin/permissions",
            get(admin_permissions::get)
                .put(admin_permissions::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/model-roles",
            get(admin_model_roles::get)
                .put(admin_model_roles::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        // The retrieval console — the route that kicks the rag-backfill and
        // rag-reindex runs and reads their projections.
        .route(
            "/api/admin/rag",
            get(admin_rag::get)
                .put(admin_rag::put)
                .post(admin_rag::post)
                .fallback(|| async { method_not_allowed("GET, POST, PUT") }),
        )
        // The rag family proper: the collection registry (list/create, then
        // one collection's bindings/delete) and the search the MCP
        // search_knowledge tool rides. Crossed together — the registry is
        // what the search resolves principals against.
        .route(
            "/api/rag/collections",
            get(rag_collections::get)
                .post(rag_collections::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        // The knowledgebase plane: folders (spaces), their doc trees, the
        // docs themselves (with comments/backlinks/move/live-presence
        // sub-routes), full-text search, and the two no-auth public slug
        // reads. One family under /api/kb — the whole tree crossed together.
        .route(
            "/api/kb/spaces",
            get(kb_spaces::get)
                .post(kb_spaces::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/kb/spaces/{id}",
            get(kb_spaces_id::get)
                .put(kb_spaces_id::put)
                .delete(kb_spaces_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/kb/spaces/{id}/docs",
            get(kb_spaces_id_docs::get)
                .post(kb_spaces_id_docs::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/kb/docs/{id}",
            get(kb_docs_id::get)
                .put(kb_docs_id::put)
                .delete(kb_docs_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/kb/docs/{id}/comments",
            get(kb_docs_id_comments::get)
                .post(kb_docs_id_comments::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/kb/docs/{id}/backlinks",
            get(kb_docs_id_backlinks::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/kb/docs/{id}/move",
            post(kb_docs_id_move::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/kb/docs/{id}/live",
            get(kb_docs_id_live::get)
                .put(kb_docs_id_live::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/kb/comments/{id}",
            patch(kb_comments_id::patch)
                .delete(kb_comments_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/kb/search",
            get(kb_search::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/kb/public/space/{slug}",
            get(kb_public_space::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/kb/public/{slug}",
            get(kb_public::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The artifacts plane — the Files surface. The list/create pair, one
        // artifact's whole state machine (sharing, official curation, brain
        // routing), its links and Drive export, folder CRUD, the no-auth
        // public slug reads, and the uploads the file artifacts point at.
        // Static paths (/for, /public/…) must beat /{id}, and in matchit they
        // always do; the two PREFIXES families ('/api/artifacts' does not
        // cover '/api/artifact-folders') are separate proxy entries.
        .route(
            "/api/artifacts",
            get(artifacts::get)
                .post(artifacts::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/artifacts/for",
            get(artifacts_for::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/artifacts/public/{slug}",
            get(artifacts_public_slug::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/artifacts/public/{slug}/download",
            get(artifacts_public_slug_download::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/artifacts/{id}",
            get(artifacts_id::get)
                .put(artifacts_id::put)
                .delete(artifacts_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/artifacts/{id}/links",
            post(artifacts_id_links::post)
                .delete(artifacts_id_links::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/artifacts/{id}/export/google",
            post(artifacts_id_export_google::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/artifact-folders",
            get(artifact_folders::get)
                .post(artifact_folders::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/artifact-folders/{id}",
            get(artifact_folders_id::get)
                .put(artifact_folders_id::put)
                .delete(artifact_folders_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/uploads",
            post(uploads::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/uploads/{id}",
            get(uploads_id::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/rag/collections/{id}",
            axum::routing::delete(rag_collections_id::delete)
                .put(rag_collections_id::put)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/rag/search",
            post(rag_search::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The two fleet routes that own the hire lifecycle (the other 18
        // fleet.* files — status, stop/start, logs, env — still serve from TS;
        // they are read-plane surface on machinery this crate exposes, and
        // each crosses with its caller).
        .route(
            "/api/fleet/create",
            post(fleet_create::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/hires",
            get(fleet_hires::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The workbench family — the agent sandbox plane: the profile
        // registry (env masked for members, infra fields admin-only), the
        // per-repo git flow, the org GitHub connection (status/installations/
        // patch/disconnect — admin, it holds org credentials), the harness
        // registry (merged builtin+custom defs), the human side of workbench
        // jobs (the ticket strip + approve/reject/merge-to-testing), the repo
        // -creation approval queue, and the per-agent repo grants.
        .route(
            "/api/workbench",
            get(workbench::get)
                .put(workbench::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/flow",
            get(workbench_flow::get)
                .put(workbench_flow::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/github",
            get(workbench_github::get)
                .put(workbench_github::put)
                .delete(workbench_github::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/workbench/harnesses",
            get(workbench_harnesses::get)
                .put(workbench_harnesses::put)
                .delete(workbench_harnesses::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/workbench/jobs",
            get(workbench_jobs::get)
                .put(workbench_jobs::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/repo-requests",
            get(workbench_repo_requests::get)
                .put(workbench_repo_requests::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/repos/{agentId}",
            get(workbench_repos_agent_id::get)
                .put(workbench_repos_agent_id::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        // The MCP family — the registry plane: the roster read (agent wire +
        // the fleet's own config), server CRUD with oauth sniffing, the
        // per-user connect surface, the fleet version-edit hook, the
        // marketplace library/icon pair, the admin probe, and the OAuth
        // start/callback pair (the callback is unauthenticated by design —
        // identity is bound into the single-use state row).
        .route(
            "/api/mcp",
            get(mcp::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/servers",
            get(mcp_servers::get)
                .post(mcp_servers::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/mcp/servers/{id}",
            put(mcp_servers_id::put)
                .delete(mcp_servers_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/me/mcp",
            get(me_mcp::get)
                .put(me_mcp::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/fleet/defs/{id}/mcp",
            post(fleet_defs_id_mcp::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/mcp/library",
            get(mcp_library::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/icon",
            get(mcp_icon::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/test",
            post(mcp_test::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/mcp/oauth/start",
            get(mcp_oauth_start::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/oauth/callback",
            get(mcp_oauth_callback::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .fallback(api_not_found)
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        // 0.7 deprecated the 408-defaulting constructor: name the status we
        // actually want a timed-out JSON call to return.
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            Duration::from_secs(30),
        ))
        .layer(CatchPanicLayer::new())
        .with_state(state.clone());

    let streaming = Router::new()
        .route(
            "/api/llm/v1/chat/completions",
            post(llm_chat::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The two SSE attach points (realtime.ts's streams). Same stack as
        // chat: a watch stream's legitimate lifetime is the client's, not a
        // handler's — the 30s layer would cut every live view off at half a
        // minute.
        .route(
            "/api/runs/{id}/events",
            get(runs_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/me/events",
            get(me_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // A channel's live events — the multiplayer-chat SSE stream.
        .route(
            "/api/channels/{id}/events",
            get(channels_id_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The MCP gateway — both verbs. A tools/call may legitimately hold
        // for the upstream's own 120s timeout, and the GET is a streamable-
        // HTTP notification stream whose lifetime is the client's: neither
        // belongs under the 30s layer.
        .route(
            "/api/mcp/gw/{server}",
            post(mcp_gw_server::post)
                .get(mcp_gw_server::get)
                .fallback(|| async { method_not_allowed("POST, GET") }),
        )
        // The Inbox panel's command run — named SSE events for one assistant
        // turn. Same stack as the watch streams: the turn's lifetime is the
        // model's, and the Inbox lock rides inside the stream task.
        .route(
            "/api/inbox/focus/command",
            post(inbox_focus_command::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .with_state(state);

    timed.merge(streaming)
}
