// Route handlers, and THE router — built here (not in main.rs) so integration
// tests drive the exact same stack the process serves.

pub mod account;
pub mod activity;
pub mod admin;
pub mod agents;
pub mod apps;
pub mod boards;
pub mod brief;
pub mod comms;
pub mod files;
pub mod fleet;
pub mod inbox;
pub mod integrations;
pub mod knowledge;
pub mod llm;
pub mod mcp;
pub mod models;
pub mod plans;
pub mod research;
pub mod secrets;
pub mod system;
pub mod tasks;
pub mod teams;
pub mod workbench;

// One dir per subsystem (the docs/api group of the same name is the map).
// Handler paths below are group-qualified — the table names the system.

use crate::state::AppState;
use axum::Router;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
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
    // One stack, no request timeout. The TS server this replaces never timed
    // a request out, and neither does this one: agent turns, tool-call
    // chains, Google syncs and SSE watches have unbounded legitimate
    // lifetimes, and a blanket timer converts "working slowly" into
    // "failed". Requests fail on errors — panics, refused guards, upstream
    // call budgets — never on a clock.
    Router::new()
        .route(
            "/api/healthz",
            get(system::health::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/llm/v1/models",
            get(llm::llm_models::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/session",
            get(account::auth_session::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/logout",
            post(account::auth_logout::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/password",
            post(account::auth_password::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/providers",
            get(account::auth_providers::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/claim",
            post(account::auth_claim::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/google",
            get(account::auth_google::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/google/callback",
            get(account::auth_google_callback::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/users",
            get(account::users::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/agents",
            get(agents::agents::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The fleet-plane pair agents themselves call (MC-compatible):
        // register with the org key, then heartbeat for assigned work.
        .route(
            "/api/agents/register",
            post(agents::agents_register::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/agents/{id}/heartbeat",
            get(agents::agents_id_heartbeat::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/apps",
            get(apps::apps::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/activity",
            get(activity::activity::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/models",
            get(models::models::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/models/efforts",
            get(models::models_efforts::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/cost",
            get(activity::cost::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/keys",
            get(models::keys::get)
                .post(models::keys::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards",
            get(boards::boards::get)
                .post(boards::boards::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards/{id}",
            axum::routing::patch(boards::boards_id::patch)
                .delete(boards::boards_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/boards/{id}/members",
            get(boards::boards_id_members::get)
                .post(boards::boards_id_members::post)
                .delete(boards::boards_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/boards/{id}/labels",
            get(boards::boards_id_labels::get)
                .post(boards::boards_id_labels::post)
                .put(boards::boards_id_labels::put)
                .delete(boards::boards_id_labels::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/boards/{id}/statuses",
            get(boards::boards_id_statuses::get)
                .post(boards::boards_id_statuses::post)
                .put(boards::boards_id_statuses::put)
                .delete(boards::boards_id_statuses::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/boards/{id}/tasks",
            get(boards::boards_id_tasks::get)
                .post(boards::boards_id_tasks::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards/{id}/agents",
            get(boards::boards_id_agents::get)
                .put(boards::boards_id_agents::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/boards/{id}/agents/self",
            post(boards::boards_id_agents::post_self)
                .delete(boards::boards_id_agents::delete_self)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/boards/{id}/agent-requests",
            get(boards::boards_id_agent_requests::get)
                .post(boards::boards_id_agent_requests::post)
                .put(boards::boards_id_agent_requests::put)
                .fallback(|| async { method_not_allowed("GET, POST, PUT") }),
        )
        .route(
            "/api/boards/{id}/templates",
            get(boards::boards_id_templates::get)
                .put(boards::boards_id_templates::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/boards/{id}/views",
            get(boards::boards_id_views::get)
                .post(boards::boards_id_views::post)
                .put(boards::boards_id_views::put)
                .delete(boards::boards_id_views::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/boards/{id}/events",
            get(boards::boards_id_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/tasks/{id}",
            get(tasks::tasks_id::get)
                .put(tasks::tasks_id::put)
                .delete(tasks::tasks_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/tasks/{id}/comments",
            get(tasks::tasks_id_comments::get)
                .post(tasks::tasks_id_comments::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/tasks/{id}/dependencies",
            axum::routing::post(tasks::tasks_id_dependencies::post)
                .delete(tasks::tasks_id_dependencies::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/tasks/{id}/review",
            axum::routing::post(tasks::tasks_id_review::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/tasks/{id}/usage",
            get(tasks::tasks_id_usage::get)
                .post(tasks::tasks_id_usage::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/tasks/{id}/watchers",
            axum::routing::post(tasks::tasks_id_watchers::post)
                .delete(tasks::tasks_id_watchers::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/keys/{id}",
            axum::routing::delete(models::keys_id::delete)
                .put(models::keys_id::put)
                .fallback(|| async { method_not_allowed("DELETE, PUT") }),
        )
        .route(
            "/api/teams",
            get(teams::teams::get)
                .post(teams::teams::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/teams/{id}",
            axum::routing::patch(teams::teams_id::patch)
                .delete(teams::teams_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/teams/{id}/members",
            get(teams::teams_id_members::get)
                .post(teams::teams_id_members::post)
                .delete(teams::teams_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/workflows",
            get(tasks::workflows::get)
                .post(tasks::workflows::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/notifications",
            get(activity::notifications::get)
                .put(activity::notifications::put)
                .patch(activity::notifications::patch)
                .fallback(|| async { method_not_allowed("GET, PUT, PATCH") }),
        )
        .route(
            "/api/history",
            get(activity::history::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The comms plane: channels/relays/DMs and their messages (with edit,
        // delete, reactions, threads, read cursors), membership, fleet-agent
        // membership, and the Relay conclude. The SSE attach point
        // ({id}/events) rides the streaming stack below, like every stream.
        .route(
            "/api/channels",
            get(comms::channels::get)
                .post(comms::channels::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/channels/{id}",
            get(comms::channels_id::get)
                .put(comms::channels_id::put)
                .delete(comms::channels_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/channels/{id}/agents",
            post(comms::channels_id_agents::post)
                .delete(comms::channels_id_agents::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/channels/{id}/conclude",
            post(comms::channels_id_conclude::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/channels/{id}/members",
            post(comms::channels_id_members::post)
                .delete(comms::channels_id_members::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/channels/{id}/messages",
            get(comms::channels_id_messages::get)
                .post(comms::channels_id_messages::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/channels/{id}/messages/{msgId}",
            axum::routing::patch(comms::channels_id_messages_msgid::patch)
                .delete(comms::channels_id_messages_msgid::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/channels/{id}/messages/{msgId}/reactions",
            post(comms::channels_id_messages_msgid_reactions::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/channels/{id}/read",
            post(comms::channels_id_read::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/channels/{id}/plan",
            get(comms::channels_id_plan::get)
                .post(comms::channels_id_plan::post)
                .patch(comms::channels_id_plan::patch)
                .delete(comms::channels_id_plan::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH, DELETE") }),
        )
        .route(
            "/api/chat",
            post(comms::chat::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/conversations",
            get(comms::conversations::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/conversations/{id}",
            get(comms::conversations_id::get)
                .patch(comms::conversations_id::patch)
                .fallback(|| async { method_not_allowed("GET, PATCH") }),
        )
        .route(
            "/api/conversations/{id}/read",
            post(comms::conversations_id_read::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/dms",
            post(comms::dms::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The Inbox focus family (queue, summary, state, actions, the
        // segmented conversations). The SSE command route lives in the
        // streaming stack below.
        .route(
            "/api/inbox/focus",
            get(inbox::inbox_focus::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/inbox/focus/summary",
            get(inbox::inbox_focus_summary::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/inbox/focus/state",
            put(inbox::inbox_focus_state::put).fallback(|| async { method_not_allowed("PUT") }),
        )
        .route(
            "/api/inbox/focus/actions",
            post(inbox::inbox_focus_actions::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/inbox/focus/conversations",
            get(inbox::inbox_focus_conversations::get)
                .post(inbox::inbox_focus_conversations::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/inbox/focus/conversations/{id}",
            get(inbox::inbox_focus_conversations_id::get)
                .delete(inbox::inbox_focus_conversations_id::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        // The brief family — the assistant's morning document, its read
        // cursor, the owner's verdict on a line, and the delegation trio.
        .route(
            "/api/brief",
            get(brief::brief::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/brief/delegate",
            get(brief::brief_delegate::get)
                .post(brief::brief_delegate::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/brief/item",
            post(brief::brief_item::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/brief/read",
            post(brief::brief_read::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/brief/reply",
            post(brief::brief_reply::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The secrets family — the sealed vault's six surfaces: the working
        // secrets a person saves and reads back, their folders, the one
        // reveal verb, sharing, the one-shot relay, and git's own way in.
        .route(
            "/api/secrets",
            get(secrets::secrets::get)
                .post(secrets::secrets::post)
                .patch(secrets::secrets::patch)
                .delete(secrets::secrets::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH, DELETE") }),
        )
        .route(
            "/api/secrets/folders",
            get(secrets::secrets_folders::get)
                .post(secrets::secrets_folders::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/secrets/git-credential",
            post(secrets::secrets_git_credential::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/relay",
            post(secrets::secrets_relay::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/reveal",
            post(secrets::secrets_reveal::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/share",
            post(secrets::secrets_share::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The integrations/google family — the personal AND org Google planes:
        // connect/callback pairs, the org targets + provisioning + health, the
        // pending-action approval queue, and the per-surface reads/mutations
        // (calendar, drive, gmail) in both flavors — as the signed-in user,
        // and as the agent acting for its owner or the org.
        .route(
            "/api/integrations/google",
            get(integrations::integrations_google::get)
                .delete(integrations::integrations_google::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        .route(
            "/api/integrations/google/connect",
            get(integrations::integrations_google_connect::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/callback",
            get(integrations::integrations_google_callback::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org",
            get(integrations::integrations_google_org::get)
                .put(integrations::integrations_google_org::put)
                .delete(integrations::integrations_google_org::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/integrations/google/org/connect",
            get(integrations::integrations_google_org_connect::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org/callback",
            get(integrations::integrations_google_org_callback::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org/health",
            get(integrations::integrations_google_org_health::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org/provision",
            get(integrations::integrations_google_org_provision::get)
                .post(integrations::integrations_google_org_provision::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/pending",
            get(integrations::integrations_google_pending::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/pending/{id}",
            post(integrations::integrations_google_pending_id::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/calendar/events",
            get(integrations::integrations_google_calendar_events::get)
                .post(integrations::integrations_google_calendar_events::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/drive/files",
            get(integrations::integrations_google_drive_files::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/drive/import",
            post(integrations::integrations_google_drive_import::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/gmail/messages",
            get(integrations::integrations_google_gmail_messages::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/gmail/send",
            post(integrations::integrations_google_gmail_send::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/agent/calendar",
            get(integrations::integrations_google_agent_calendar::get)
                .post(integrations::integrations_google_agent_calendar::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/agent/drive",
            get(integrations::integrations_google_agent_drive::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/agent/gmail",
            get(integrations::integrations_google_agent_gmail::get)
                .post(integrations::integrations_google_agent_gmail::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/agent/gmail/labels",
            get(integrations::integrations_google_agent_gmail_labels::get)
                .post(integrations::integrations_google_agent_gmail_labels::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/agent/gmail/organize",
            post(integrations::integrations_google_agent_gmail_organize::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/agent/gmail/{id}",
            get(integrations::integrations_google_agent_gmail_id::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/plans/{id}/draft",
            get(plans::plans_id_draft::get)
                .post(plans::plans_id_draft::post)
                .patch(plans::plans_id_draft::patch)
                .delete(plans::plans_id_draft::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH, DELETE") }),
        )
        .route(
            "/api/research",
            get(research::research::get)
                .post(research::research::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/research/{id}",
            get(research::research_id::get)
                .delete(research::research_id::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        .route(
            "/api/research/{id}/members",
            get(research::research_id_members::get)
                .post(research::research_id_members::post)
                .delete(research::research_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/research/{id}/conversation",
            post(research::research_id_conversation::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/research/{id}/decide",
            post(research::research_id_decide::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/me",
            get(account::me::get)
                .put(account::me::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workflows/{id}",
            put(tasks::workflows_id::put)
                .delete(tasks::workflows_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/agent-role-templates",
            get(agents::agent_role_templates::get)
                .put(agents::agent_role_templates::put)
                .delete(agents::agent_role_templates::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/admin/password-accounts",
            get(admin::admin_password_accounts::get)
                .post(admin::admin_password_accounts::post)
                .put(admin::admin_password_accounts::put)
                .delete(admin::admin_password_accounts::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/admin/google-client",
            get(admin::admin_google_client::get)
                .put(admin::admin_google_client::put)
                .delete(admin::admin_google_client::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/admin/google-client/login",
            put(admin::admin_google_client::put_login)
                .fallback(|| async { method_not_allowed("PUT") }),
        )
        .route(
            "/api/admin/instance",
            get(admin::admin_instance::get)
                .put(admin::admin_instance::put)
                .post(admin::admin_instance::post)
                .fallback(|| async { method_not_allowed("GET, PUT, POST") }),
        )
        .route(
            "/api/admin/apps",
            get(admin::admin_apps::get)
                .put(admin::admin_apps::put)
                .post(admin::admin_apps::post)
                .delete(admin::admin_apps::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, POST, DELETE") }),
        )
        .route(
            "/api/admin/domains",
            get(admin::admin_domains::get)
                .post(admin::admin_domains::post)
                .delete(admin::admin_domains::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/admin/email",
            get(admin::admin_email::get)
                .put(admin::admin_email::put)
                .post(admin::admin_email::post)
                .fallback(|| async { method_not_allowed("GET, PUT, POST") }),
        )
        .route(
            "/api/admin/encryption",
            get(admin::admin_encryption::get)
                .post(admin::admin_encryption::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/admin/guardrails",
            get(admin::admin_guardrails::get)
                .put(admin::admin_guardrails::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/invites",
            get(admin::admin_invites::get)
                .post(admin::admin_invites::post)
                .delete(admin::admin_invites::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/admin/judge",
            get(admin::admin_judge::get)
                .put(admin::admin_judge::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        // The fitness plane — the probe/eval/adversarial battery over the
        // gateway's models, its run engine, and its archive.
        .route(
            "/api/admin/model-fitness",
            get(admin::admin_model_fitness::get)
                .post(admin::admin_model_fitness::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/admin/outreach",
            get(admin::admin_outreach::get)
                .put(admin::admin_outreach::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/platform-agents",
            get(admin::admin_platform_agents::get)
                .put(admin::admin_platform_agents::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/search",
            get(admin::admin_search::get)
                .put(admin::admin_search::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/secrets",
            get(admin::admin_secrets::get)
                .delete(admin::admin_secrets::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        .route(
            "/api/admin/settings",
            get(admin::admin_settings::get)
                .put(admin::admin_settings::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/storage",
            get(admin::admin_storage::get)
                .put(admin::admin_storage::put)
                .post(admin::admin_storage::post)
                .fallback(|| async { method_not_allowed("GET, PUT, POST") }),
        )
        .route(
            "/api/admin/users",
            get(admin::admin_users::get)
                .put(admin::admin_users::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/workspace-secrets",
            get(admin::admin_workspace_secrets::get)
                .post(admin::admin_workspace_secrets::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/admin/permissions",
            get(admin::admin_permissions::get)
                .put(admin::admin_permissions::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/model-roles",
            get(admin::admin_model_roles::get)
                .put(admin::admin_model_roles::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        // The retrieval console — the route that kicks the rag-backfill and
        // rag-reindex runs and reads their projections.
        .route(
            "/api/admin/rag",
            get(admin::admin_rag::get)
                .put(admin::admin_rag::put)
                .post(admin::admin_rag::post)
                .fallback(|| async { method_not_allowed("GET, POST, PUT") }),
        )
        // The rag family proper: the collection registry (list/create, then
        // one collection's bindings/delete) and the search the MCP
        // search_knowledge tool rides. Crossed together — the registry is
        // what the search resolves principals against.
        .route(
            "/api/rag/collections",
            get(knowledge::rag_collections::get)
                .post(knowledge::rag_collections::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        // The knowledgebase plane: folders (spaces), their doc trees, the
        // docs themselves (with comments/backlinks/move/live-presence
        // sub-routes), full-text search, and the two no-auth public slug
        // reads. One family under /api/kb — the whole tree crossed together.
        .route(
            "/api/kb/spaces",
            get(knowledge::kb_spaces::get)
                .post(knowledge::kb_spaces::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/kb/spaces/{id}",
            get(knowledge::kb_spaces_id::get)
                .put(knowledge::kb_spaces_id::put)
                .delete(knowledge::kb_spaces_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/kb/spaces/{id}/docs",
            get(knowledge::kb_spaces_id_docs::get)
                .post(knowledge::kb_spaces_id_docs::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/kb/docs/{id}",
            get(knowledge::kb_docs_id::get)
                .put(knowledge::kb_docs_id::put)
                .delete(knowledge::kb_docs_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/kb/docs/{id}/comments",
            get(knowledge::kb_docs_id_comments::get)
                .post(knowledge::kb_docs_id_comments::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/kb/docs/{id}/backlinks",
            get(knowledge::kb_docs_id_backlinks::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/kb/docs/{id}/move",
            post(knowledge::kb_docs_id_move::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/kb/docs/{id}/live",
            get(knowledge::kb_docs_id_live::get)
                .put(knowledge::kb_docs_id_live::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/kb/comments/{id}",
            patch(knowledge::kb_comments_id::patch)
                .delete(knowledge::kb_comments_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/kb/search",
            get(knowledge::kb_search::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/kb/public/space/{slug}",
            get(knowledge::kb_public_space::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/kb/public/{slug}",
            get(knowledge::kb_public::get).fallback(|| async { method_not_allowed("GET") }),
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
            get(files::artifacts::get)
                .post(files::artifacts::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/artifacts/for",
            get(files::artifacts_for::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/artifacts/public/{slug}",
            get(files::artifacts_public_slug::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/artifacts/public/{slug}/download",
            get(files::artifacts_public_slug_download::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/artifacts/{id}",
            get(files::artifacts_id::get)
                .put(files::artifacts_id::put)
                .delete(files::artifacts_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/artifacts/{id}/links",
            post(files::artifacts_id_links::post)
                .delete(files::artifacts_id_links::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/artifacts/{id}/export/google",
            post(files::artifacts_id_export_google::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/artifact-folders",
            get(files::artifact_folders::get)
                .post(files::artifact_folders::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/artifact-folders/{id}",
            get(files::artifact_folders_id::get)
                .put(files::artifact_folders_id::put)
                .delete(files::artifact_folders_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/uploads",
            post(files::uploads::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/uploads/{id}",
            get(files::uploads_id::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/rag/collections/{id}",
            axum::routing::delete(knowledge::rag_collections_id::delete)
                .put(knowledge::rag_collections_id::put)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/rag/search",
            post(knowledge::rag_search::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The fleet family. The hire lifecycle (create + the hire queue),
        // the read plane (overview, containers, defs, endpoints, crons), the
        // admin verbs (render, reconcile, federate), the per-agent control
        // surface (lifecycle, crons, secrets), and the fleet-plane pair
        // agents call themselves (register + heartbeat, registered above).
        .route(
            "/api/fleet/create",
            post(fleet::fleet_create::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/hires",
            get(fleet::fleet_hires::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet",
            get(fleet::fleet::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet/containers",
            get(fleet::fleet_containers::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet/render",
            post(fleet::fleet_render::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/reconcile",
            post(fleet::fleet_reconcile::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/federate",
            post(fleet::fleet_federate::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/defs",
            get(fleet::fleet_defs::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet/endpoints",
            get(fleet::fleet_endpoints::get)
                .post(fleet::fleet_endpoints::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/fleet/endpoints/{id}",
            axum::routing::put(fleet::fleet_endpoints_id::put)
                .delete(fleet::fleet_endpoints_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/fleet/endpoints/{id}/available",
            get(fleet::fleet_endpoints_id_available::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet/crons",
            get(fleet::fleet_crons::get)
                .post(fleet::fleet_crons::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/fleet/agents/{id}/crons",
            get(fleet::fleet_agents_id_crons::get)
                .post(fleet::fleet_agents_id_crons::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/fleet/agents/{id}/crons/{jobId}",
            axum::routing::delete(fleet::fleet_agents_id_crons_jobid::delete)
                .put(fleet::fleet_agents_id_crons_jobid::put)
                .post(fleet::fleet_agents_id_crons_jobid::post)
                .fallback(|| async { method_not_allowed("POST, PUT, DELETE") }),
        )
        .route(
            "/api/fleet/agents/{id}/secrets",
            get(fleet::fleet_agents_id_secrets::get)
                .put(fleet::fleet_agents_id_secrets::put)
                .delete(fleet::fleet_agents_id_secrets::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/fleet/agents/{id}/control",
            post(fleet::fleet_agents_id_control::post)
                .fallback(|| async { method_not_allowed("POST") }),
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
            get(workbench::workbench::get)
                .put(workbench::workbench::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/flow",
            get(workbench::workbench_flow::get)
                .put(workbench::workbench_flow::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/github",
            get(workbench::workbench_github::get)
                .put(workbench::workbench_github::put)
                .delete(workbench::workbench_github::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/workbench/harnesses",
            get(workbench::workbench_harnesses::get)
                .put(workbench::workbench_harnesses::put)
                .delete(workbench::workbench_harnesses::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/workbench/jobs",
            get(workbench::workbench_jobs::get)
                .put(workbench::workbench_jobs::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/repo-requests",
            get(workbench::workbench_repo_requests::get)
                .put(workbench::workbench_repo_requests::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/repos/{agentId}",
            get(workbench::workbench_repos_agent_id::get)
                .put(workbench::workbench_repos_agent_id::put)
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
            get(mcp::mcp::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/servers",
            get(mcp::mcp_servers::get)
                .post(mcp::mcp_servers::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/mcp/servers/{id}",
            put(mcp::mcp_servers_id::put)
                .delete(mcp::mcp_servers_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/me/mcp",
            get(account::me_mcp::get)
                .put(account::me_mcp::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/fleet/defs/{id}/mcp",
            post(fleet::fleet_defs_id_mcp::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/mcp/library",
            get(mcp::mcp_library::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/icon",
            get(mcp::mcp_icon::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/test",
            post(mcp::mcp_test::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/mcp/oauth/start",
            get(mcp::mcp_oauth_start::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/oauth/callback",
            get(mcp::mcp_oauth_callback::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/llm/v1/chat/completions",
            post(llm::llm_chat::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The SSE attach points (realtime.ts's streams): a watch stream's
        // legitimate lifetime is the client's, not a handler's.
        .route(
            "/api/runs/{id}/events",
            get(agents::runs_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/me/events",
            get(account::me_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // A channel's live events — the multiplayer-chat SSE stream.
        .route(
            "/api/channels/{id}/events",
            get(comms::channels_id_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The MCP gateway — both verbs. A tools/call may legitimately hold
        // for the upstream's own 120s timeout, and the GET is a streamable-
        // HTTP notification stream whose lifetime is the client's.
        .route(
            "/api/mcp/gw/{server}",
            post(mcp::mcp_gw_server::post)
                .get(mcp::mcp_gw_server::get)
                .fallback(|| async { method_not_allowed("POST, GET") }),
        )
        // The Inbox panel's command run — named SSE events for one assistant
        // turn. The turn's lifetime is the model's, and the Inbox lock rides
        // inside the stream task.
        .route(
            "/api/inbox/focus/command",
            post(inbox::inbox_focus_command::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        // ── The remaining singles (R23) ────────────────────────────────────
        // The instance identity beacon, the derived-alerts read, and the
        // Studio's Suggested queue with its status verb.
        .route(
            "/api/well-known/talaria-instance",
            get(system::well_known_talaria_instance::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/alerts",
            get(activity::alerts::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/gaps",
            get(agents::gaps::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/gaps/{id}",
            axum::routing::put(agents::gaps_id::put)
                .fallback(|| async { method_not_allowed("PUT") }),
        )
        // Home/Today, and the public /join invite lookup (dual-counter rate
        // limited, same shape as login).
        .route(
            "/api/home",
            get(activity::home::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/join",
            get(account::join::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The agent-memory surface (admin or the assistant's owner), the
        // live web-search door both agents and sessions reach, and the
        // org's template library with its one-template verbs.
        .route(
            "/api/memory/{id}",
            get(knowledge::memory_id::get)
                .put(knowledge::memory_id::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/search",
            post(knowledge::search::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/templates",
            get(knowledge::templates::get)
                .post(knowledge::templates::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/templates/{id}",
            axum::routing::put(knowledge::templates_id::put)
                .delete(knowledge::templates_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        // The org's skill library: the owner index with edit flags, and the
        // one-skill verbs (rename/copy/move ride POST).
        .route(
            "/api/skills",
            get(agents::skills::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/skills/{owner}/{name}",
            get(agents::skills_owner_name::get)
                .put(agents::skills_owner_name::put)
                .post(agents::skills_owner_name::post)
                .delete(agents::skills_owner_name::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, POST, DELETE") }),
        )
        // The agent surface: media reads/writes scoped by model, the two
        // honesty-loop reports (gap, problem), the plain-language
        // message-user door, and the self-introspection probe.
        .route(
            "/api/agent-media/{model}",
            get(files::agent_media_model::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/agent-media/{model}/save",
            post(files::agent_media_model_save::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/agent/whoami",
            get(agents::agent_whoami::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/agent/gap",
            post(agents::agent_gap::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/agent/problem",
            post(agents::agent_problem::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/agent/message-user",
            post(agents::agent_message_user::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        // The Muse, the image describer, the personal assistant, and the
        // local-inference observability plane.
        .route(
            "/api/muse",
            post(agents::muse::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/vision/describe",
            post(agents::vision_describe::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/me/assistant",
            get(account::me_assistant::get)
                .post(account::me_assistant::post)
                .patch(account::me_assistant::patch)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH") }),
        )
        .route(
            "/api/inference",
            get(models::inference::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // Multiplayer plans: the living document and the member roster.
        .route(
            "/api/plans/{id}/doc",
            get(plans::plans_id_doc::get)
                .post(plans::plans_id_doc::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/plans/{id}/members",
            get(plans::plans_id_members::get)
                .post(plans::plans_id_members::post)
                .put(plans::plans_id_members::put)
                .delete(plans::plans_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        // The fleet defs detail trio: identity PATCH, the versioned edit,
        // and the version history.
        .route(
            "/api/fleet/defs/{id}",
            axum::routing::patch(fleet::fleet_defs_id::patch)
                .fallback(|| async { method_not_allowed("PATCH") }),
        )
        .route(
            "/api/fleet/defs/{id}/edit",
            post(fleet::fleet_defs_id_edit::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/defs/{id}/versions",
            get(fleet::fleet_defs_id_versions::get)
                .post(fleet::fleet_defs_id_versions::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .fallback(api_not_found)
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .with_state(state)
}
