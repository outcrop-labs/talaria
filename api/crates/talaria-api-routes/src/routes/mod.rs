// Route handlers, and THE router — built here (not in main.rs) so integration
// tests drive the exact same stack the process serves.

// One dir per subsystem (the docs/api group of the same name is the map).
// Handler paths below are group-qualified — the table names the system.

use axum::Router;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use talaria_state::AppState;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

/// app.ts's 405: the body is fixed, and the allow header is the TS route
/// file's handler keys in DECLARATION order (Object.keys of the handlers
/// object) — the order is pinned, not alphabetical.
fn method_not_allowed(allow: &'static str) -> Response {
    let mut res = talaria_error::house_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    res.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static(allow));
    res
}

async fn api_not_found(uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        return talaria_error::house_error(StatusCode::NOT_FOUND, "not found");
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
            get(talaria_routes_workbench::system::health::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/llm/v1/models",
            get(talaria_routes_fleet::llm::llm_models::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/session",
            get(talaria_routes_integrations::account::auth_session::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/logout",
            post(talaria_routes_integrations::account::auth_logout::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/password",
            post(talaria_routes_integrations::account::auth_password::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/providers",
            get(talaria_routes_integrations::account::auth_providers::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/claim",
            post(talaria_routes_integrations::account::auth_claim::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/auth/google",
            get(talaria_routes_integrations::account::auth_google::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/auth/google/callback",
            get(talaria_routes_integrations::account::auth_google_callback::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/users",
            get(talaria_routes_integrations::account::users::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/agents",
            get(talaria_routes_fleet::agents::agents::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The fleet-plane pair agents themselves call (MC-compatible):
        // register with the org key, then heartbeat for assigned work.
        .route(
            "/api/agents/register",
            post(talaria_routes_fleet::agents::agents_register::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/agents/{id}/heartbeat",
            get(talaria_routes_fleet::agents::agents_id_heartbeat::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/apps",
            get(talaria_routes_fleet::apps::apps::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/activity",
            get(talaria_routes_comms::activity::activity::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/models",
            get(talaria_routes_fleet::models::models::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/models/efforts",
            get(talaria_routes_fleet::models::models_efforts::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/cost",
            get(talaria_routes_comms::activity::cost::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/keys",
            get(talaria_routes_fleet::models::keys::get)
                .post(talaria_routes_fleet::models::keys::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards",
            get(talaria_routes_boards::boards::boards::get)
                .post(talaria_routes_boards::boards::boards::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards/{id}",
            axum::routing::patch(talaria_routes_boards::boards::boards_id::patch)
                .delete(talaria_routes_boards::boards::boards_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/boards/{id}/work-sessions",
            get(talaria_routes_boards::boards::boards_id_work_sessions::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/boards/{id}/members",
            get(talaria_routes_boards::boards::boards_id_members::get)
                .post(talaria_routes_boards::boards::boards_id_members::post)
                .delete(talaria_routes_boards::boards::boards_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/boards/{id}/labels",
            get(talaria_routes_boards::boards::boards_id_labels::get)
                .post(talaria_routes_boards::boards::boards_id_labels::post)
                .put(talaria_routes_boards::boards::boards_id_labels::put)
                .delete(talaria_routes_boards::boards::boards_id_labels::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/boards/{id}/statuses",
            get(talaria_routes_boards::boards::boards_id_statuses::get)
                .post(talaria_routes_boards::boards::boards_id_statuses::post)
                .put(talaria_routes_boards::boards::boards_id_statuses::put)
                .delete(talaria_routes_boards::boards::boards_id_statuses::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/boards/{id}/tasks",
            get(talaria_routes_boards::boards::boards_id_tasks::get)
                .post(talaria_routes_boards::boards::boards_id_tasks::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards/{id}/agents",
            get(talaria_routes_boards::boards::boards_id_agents::get)
                .put(talaria_routes_boards::boards::boards_id_agents::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/boards/{id}/agents/self",
            post(talaria_routes_boards::boards::boards_id_agents::post_self)
                .delete(talaria_routes_boards::boards::boards_id_agents::delete_self)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/boards/{id}/agent-requests",
            get(talaria_routes_boards::boards::boards_id_agent_requests::get)
                .post(talaria_routes_boards::boards::boards_id_agent_requests::post)
                .put(talaria_routes_boards::boards::boards_id_agent_requests::put)
                .fallback(|| async { method_not_allowed("GET, POST, PUT") }),
        )
        .route(
            "/api/boards/{id}/templates",
            get(talaria_routes_boards::boards::boards_id_templates::get)
                .put(talaria_routes_boards::boards::boards_id_templates::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/boards/{id}/views",
            get(talaria_routes_boards::boards::boards_id_views::get)
                .post(talaria_routes_boards::boards::boards_id_views::post)
                .put(talaria_routes_boards::boards::boards_id_views::put)
                .delete(talaria_routes_boards::boards::boards_id_views::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/boards/{id}/workchains",
            get(talaria_routes_boards::boards::boards_id_workchains::get)
                .post(talaria_routes_boards::boards::boards_id_workchains::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/boards/{id}/events",
            get(talaria_routes_boards::boards::boards_id_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/tasks/{id}/work-session",
            get(talaria_routes_boards::tasks::tasks_id_work_session::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/tasks/{id}/work-session/stop",
            post(talaria_routes_boards::tasks::tasks_id_work_session_stop::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/tasks/{id}/work-sessions",
            get(talaria_routes_boards::tasks::tasks_id_work_sessions::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/tasks/{id}",
            get(talaria_routes_boards::tasks::tasks_id::get)
                .put(talaria_routes_boards::tasks::tasks_id::put)
                .delete(talaria_routes_boards::tasks::tasks_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/tasks/{id}/comments",
            get(talaria_routes_boards::tasks::tasks_id_comments::get)
                .post(talaria_routes_boards::tasks::tasks_id_comments::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/tasks/{id}/channel",
            post(talaria_routes_boards::tasks::tasks_id_channel::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/tasks/{id}/dependencies",
            axum::routing::post(talaria_routes_boards::tasks::tasks_id_dependencies::post)
                .delete(talaria_routes_boards::tasks::tasks_id_dependencies::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/tasks/{id}/review",
            axum::routing::post(talaria_routes_boards::tasks::tasks_id_review::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/tasks/{id}/usage",
            get(talaria_routes_boards::tasks::tasks_id_usage::get)
                .post(talaria_routes_boards::tasks::tasks_id_usage::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/tasks/{id}/watchers",
            axum::routing::post(talaria_routes_boards::tasks::tasks_id_watchers::post)
                .delete(talaria_routes_boards::tasks::tasks_id_watchers::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/workchains/{id}",
            patch(talaria_routes_boards::workchains::workchains_id::patch)
                .delete(talaria_routes_boards::workchains::workchains_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/workchains/{id}/steps",
            axum::routing::post(talaria_routes_boards::workchains::workchains_id::post_step)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/workchains/{id}/steps/{taskId}",
            axum::routing::delete(talaria_routes_boards::workchains::workchains_id::delete_step)
                .fallback(|| async { method_not_allowed("DELETE") }),
        )
        .route(
            "/api/workchains/{id}/edges",
            axum::routing::post(talaria_routes_boards::workchains::workchains_id::post_edge)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/workchains/{id}/edges/{fromTaskId}/{toTaskId}",
            axum::routing::delete(talaria_routes_boards::workchains::workchains_id::delete_edge)
                .fallback(|| async { method_not_allowed("DELETE") }),
        )
        .route(
            "/api/keys/{id}",
            axum::routing::delete(talaria_routes_fleet::models::keys_id::delete)
                .put(talaria_routes_fleet::models::keys_id::put)
                .fallback(|| async { method_not_allowed("DELETE, PUT") }),
        )
        .route(
            "/api/teams",
            get(talaria_routes_integrations::teams::teams::get)
                .post(talaria_routes_integrations::teams::teams::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/teams/directory",
            get(talaria_routes_integrations::teams::teams_directory::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/teams/{id}",
            get(talaria_routes_integrations::teams::teams_id::get)
                .patch(talaria_routes_integrations::teams::teams_id::patch)
                .delete(talaria_routes_integrations::teams::teams_id::delete)
                .fallback(|| async { method_not_allowed("GET, PATCH, DELETE") }),
        )
        .route(
            "/api/teams/{id}/members",
            get(talaria_routes_integrations::teams::teams_id_members::get)
                .post(talaria_routes_integrations::teams::teams_id_members::post)
                .delete(talaria_routes_integrations::teams::teams_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/teams/{id}/agents",
            get(talaria_routes_integrations::teams::teams_id_agents::get)
                .post(talaria_routes_integrations::teams::teams_id_agents::post)
                .delete(talaria_routes_integrations::teams::teams_id_agents::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/teams/{id}/access",
            get(talaria_routes_integrations::teams::teams_id_access::get)
                .put(talaria_routes_integrations::teams::teams_id_access::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workflows",
            get(talaria_routes_boards::tasks::workflows::get)
                .post(talaria_routes_boards::tasks::workflows::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/notifications",
            get(talaria_routes_comms::activity::notifications::get)
                .put(talaria_routes_comms::activity::notifications::put)
                .patch(talaria_routes_comms::activity::notifications::patch)
                .fallback(|| async { method_not_allowed("GET, PUT, PATCH") }),
        )
        .route(
            "/api/unreads",
            get(talaria_routes_comms::activity::unreads::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The closed-tab plane's door: the VAPID public key a browser needs
        // to subscribe, and the subscribe/unsubscribe pair that files and
        // retires a device browser. Delivery itself is scheduler-plane
        // (src/push.rs), not a route.
        .route(
            "/api/push/key",
            get(talaria_routes_comms::activity::push::key).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/push/subscribe",
            post(talaria_routes_comms::activity::push::subscribe).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/push/unsubscribe",
            post(talaria_routes_comms::activity::push::unsubscribe).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/history",
            get(talaria_routes_comms::activity::history::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The comms plane: channels/relays/DMs and their messages (with edit,
        // delete, reactions, threads, read cursors), membership, fleet-agent
        // membership, and the Relay conclude. The SSE attach point
        // ({id}/events) rides the streaming stack below, like every stream.
        .route(
            "/api/channels",
            get(talaria_routes_comms::comms::channels::get)
                .post(talaria_routes_comms::comms::channels::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/channels/{id}",
            get(talaria_routes_comms::comms::channels_id::get)
                .put(talaria_routes_comms::comms::channels_id::put)
                .delete(talaria_routes_comms::comms::channels_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/channels/{id}/agents",
            post(talaria_routes_comms::comms::channels_id_agents::post)
                .delete(talaria_routes_comms::comms::channels_id_agents::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/channels/{id}/conclude",
            post(talaria_routes_comms::comms::channels_id_conclude::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/channels/{id}/members",
            post(talaria_routes_comms::comms::channels_id_members::post)
                .delete(talaria_routes_comms::comms::channels_id_members::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/channels/{id}/teams",
            post(talaria_routes_comms::comms::channels_id_teams::post)
                .delete(talaria_routes_comms::comms::channels_id_teams::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/channels/{id}/messages",
            get(talaria_routes_comms::comms::channels_id_messages::get)
                .post(talaria_routes_comms::comms::channels_id_messages::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/channels/{id}/messages/{msgId}",
            axum::routing::patch(talaria_routes_comms::comms::channels_id_messages_msgid::patch)
                .delete(talaria_routes_comms::comms::channels_id_messages_msgid::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/channels/{id}/messages/{msgId}/reactions",
            post(talaria_routes_comms::comms::channels_id_messages_msgid_reactions::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/channels/{id}/read",
            post(talaria_routes_comms::comms::channels_id_read::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/channels/{id}/plan",
            get(talaria_routes_comms::comms::channels_id_plan::get)
                .post(talaria_routes_comms::comms::channels_id_plan::post)
                .patch(talaria_routes_comms::comms::channels_id_plan::patch)
                .delete(talaria_routes_comms::comms::channels_id_plan::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH, DELETE") }),
        )
        .route(
            "/api/chat",
            post(talaria_routes_comms::comms::chat::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/chat/chips/resolve",
            post(talaria_routes_comms::comms::chips::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/chat/chips/approvals/{id}",
            post(talaria_routes_comms::comms::chips_approvals_id::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/conversations",
            get(talaria_routes_comms::comms::conversations::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/conversations/{id}",
            get(talaria_routes_comms::comms::conversations_id::get)
                .patch(talaria_routes_comms::comms::conversations_id::patch)
                .delete(talaria_routes_comms::comms::conversations_id::delete)
                .fallback(|| async { method_not_allowed("GET, PATCH, DELETE") }),
        )
        .route(
            "/api/conversations/{id}/read",
            post(talaria_routes_comms::comms::conversations_id_read::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/dms",
            post(talaria_routes_comms::comms::dms::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The Inbox focus family (queue, summary, state, actions, the
        // segmented conversations). The SSE command route lives in the
        // streaming stack below.
        .route(
            "/api/inbox/focus",
            get(talaria_routes_comms::inbox::inbox_focus::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/inbox/focus/summary",
            get(talaria_routes_comms::inbox::inbox_focus_summary::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/inbox/focus/state",
            put(talaria_routes_comms::inbox::inbox_focus_state::put).fallback(|| async { method_not_allowed("PUT") }),
        )
        .route(
            "/api/inbox/focus/actions",
            post(talaria_routes_comms::inbox::inbox_focus_actions::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/inbox/focus/conversations",
            get(talaria_routes_comms::inbox::inbox_focus_conversations::get)
                .post(talaria_routes_comms::inbox::inbox_focus_conversations::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/inbox/focus/conversations/{id}",
            get(talaria_routes_comms::inbox::inbox_focus_conversations_id::get)
                .delete(talaria_routes_comms::inbox::inbox_focus_conversations_id::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        // The brief family — the assistant's morning document, its read
        // cursor, the owner's verdict on a line, and the delegation trio.
        .route(
            "/api/brief",
            get(talaria_routes_comms::brief::brief::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/brief/delegate",
            get(talaria_routes_comms::brief::brief_delegate::get)
                .post(talaria_routes_comms::brief::brief_delegate::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/brief/item",
            post(talaria_routes_comms::brief::brief_item::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/brief/read",
            post(talaria_routes_comms::brief::brief_read::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/brief/reply",
            post(talaria_routes_comms::brief::brief_reply::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The secrets family — the sealed vault's six surfaces: the working
        // secrets a person saves and reads back, their folders, the one
        // reveal verb, sharing, the one-shot relay, and git's own way in.
        .route(
            "/api/secrets",
            get(talaria_routes_integrations::secrets::secrets::get)
                .post(talaria_routes_integrations::secrets::secrets::post)
                .patch(talaria_routes_integrations::secrets::secrets::patch)
                .delete(talaria_routes_integrations::secrets::secrets::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH, DELETE") }),
        )
        .route(
            "/api/secrets/folders",
            get(talaria_routes_integrations::secrets::secrets_folders::get)
                .post(talaria_routes_integrations::secrets::secrets_folders::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/secrets/git-credential",
            post(talaria_routes_integrations::secrets::secrets_git_credential::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/git-push-check",
            post(talaria_routes_integrations::secrets::secrets_git_push_check::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/relay",
            post(talaria_routes_integrations::secrets::secrets_relay::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/reveal",
            post(talaria_routes_integrations::secrets::secrets_reveal::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/secrets/share",
            post(talaria_routes_integrations::secrets::secrets_share::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The integrations/google family — the personal AND org Google planes:
        // connect/callback pairs, the org targets + provisioning + health, the
        // pending-action approval queue, and the per-surface reads/mutations
        // (calendar, drive, gmail) in both flavors — as the signed-in user,
        // and as the agent acting for its owner or the org.
        .route(
            "/api/integrations/google",
            get(talaria_routes_integrations::integrations::integrations_google::get)
                .delete(talaria_routes_integrations::integrations::integrations_google::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        .route(
            "/api/integrations/google/connect",
            get(talaria_routes_integrations::integrations::integrations_google_connect::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/callback",
            get(talaria_routes_integrations::integrations::integrations_google_callback::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org",
            get(talaria_routes_integrations::integrations::integrations_google_org::get)
                .put(talaria_routes_integrations::integrations::integrations_google_org::put)
                .delete(talaria_routes_integrations::integrations::integrations_google_org::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/integrations/google/org/connect",
            get(talaria_routes_integrations::integrations::integrations_google_org_connect::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org/callback",
            get(talaria_routes_integrations::integrations::integrations_google_org_callback::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org/health",
            get(talaria_routes_integrations::integrations::integrations_google_org_health::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/org/provision",
            get(talaria_routes_integrations::integrations::integrations_google_org_provision::get)
                .post(talaria_routes_integrations::integrations::integrations_google_org_provision::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/pending",
            get(talaria_routes_integrations::integrations::integrations_google_pending::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/pending/{id}",
            post(talaria_routes_integrations::integrations::integrations_google_pending_id::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/calendar/events",
            get(talaria_routes_integrations::integrations::integrations_google_calendar_events::get)
                .post(talaria_routes_integrations::integrations::integrations_google_calendar_events::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/drive/drives",
            get(talaria_routes_integrations::integrations::integrations_google_drive_drives::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/drive/browse",
            get(talaria_routes_integrations::integrations::integrations_google_drive_browse::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/drive/rename",
            post(talaria_routes_integrations::integrations::integrations_google_drive_manage::rename)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/drive/move",
            post(talaria_routes_integrations::integrations::integrations_google_drive_manage::drive_move)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/drive/trash",
            post(talaria_routes_integrations::integrations::integrations_google_drive_manage::trash)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/drive/create-folder",
            post(talaria_routes_integrations::integrations::integrations_google_drive_manage::create_folder)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/drive/import",
            post(talaria_routes_integrations::integrations::integrations_google_drive_import::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/gmail/messages",
            get(talaria_routes_integrations::integrations::integrations_google_gmail_messages::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/gmail/send",
            post(talaria_routes_integrations::integrations::integrations_google_gmail_send::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/agent/calendar",
            get(talaria_routes_integrations::integrations::integrations_google_agent_calendar::get)
                .post(talaria_routes_integrations::integrations::integrations_google_agent_calendar::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/agent/drive",
            get(talaria_routes_integrations::integrations::integrations_google_agent_drive::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/agent/gmail",
            get(talaria_routes_integrations::integrations::integrations_google_agent_gmail::get)
                .post(talaria_routes_integrations::integrations::integrations_google_agent_gmail::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/agent/pending",
            get(talaria_routes_integrations::integrations::integrations_google_agent_pending::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/integrations/google/agent/pending/{id}",
            get(talaria_routes_integrations::integrations::integrations_google_agent_pending::get_one)
                .fallback(|| async { method_not_allowed("GET") }),
        )

        .route(
            "/api/integrations/google/agent/gmail/labels",
            get(talaria_routes_integrations::integrations::integrations_google_agent_gmail_labels::get)
                .post(talaria_routes_integrations::integrations::integrations_google_agent_gmail_labels::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/integrations/google/agent/gmail/organize",
            post(talaria_routes_integrations::integrations::integrations_google_agent_gmail_organize::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/integrations/google/agent/gmail/{id}",
            get(talaria_routes_integrations::integrations::integrations_google_agent_gmail_id::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/plans/{id}/draft",
            get(talaria_routes_boards::plans::plans_id_draft::get)
                .post(talaria_routes_boards::plans::plans_id_draft::post)
                .patch(talaria_routes_boards::plans::plans_id_draft::patch)
                .delete(talaria_routes_boards::plans::plans_id_draft::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH, DELETE") }),
        )
        .route(
            "/api/research",
            get(talaria_routes_workbench::research::research::get)
                .post(talaria_routes_workbench::research::research::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/research/{id}",
            get(talaria_routes_workbench::research::research_id::get)
                .patch(talaria_routes_workbench::research::research_id::patch)
                .delete(talaria_routes_workbench::research::research_id::delete)
                .fallback(|| async { method_not_allowed("GET, PATCH, DELETE") }),
        )
        .route(
            "/api/research/{id}/members",
            get(talaria_routes_workbench::research::research_id_members::get)
                .post(talaria_routes_workbench::research::research_id_members::post)
                .delete(talaria_routes_workbench::research::research_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/research/{id}/teams",
            post(talaria_routes_workbench::research::research_id_teams::post)
                .delete(talaria_routes_workbench::research::research_id_teams::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/research/{id}/conversation",
            post(talaria_routes_workbench::research::research_id_conversation::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/research/{id}/decide",
            post(talaria_routes_workbench::research::research_id_decide::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/me",
            get(talaria_routes_integrations::account::me::get)
                .put(talaria_routes_integrations::account::me::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workflows/{id}",
            put(talaria_routes_boards::tasks::workflows_id::put)
                .delete(talaria_routes_boards::tasks::workflows_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/agent-role-templates",
            get(talaria_routes_fleet::agents::agent_role_templates::get)
                .put(talaria_routes_fleet::agents::agent_role_templates::put)
                .delete(talaria_routes_fleet::agents::agent_role_templates::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/admin/password-accounts",
            get(talaria_routes_admin::admin::admin_password_accounts::get)
                .post(talaria_routes_admin::admin::admin_password_accounts::post)
                .put(talaria_routes_admin::admin::admin_password_accounts::put)
                .delete(talaria_routes_admin::admin::admin_password_accounts::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/admin/google-client",
            get(talaria_routes_admin::admin::admin_google_client::get)
                .put(talaria_routes_admin::admin::admin_google_client::put)
                .delete(talaria_routes_admin::admin::admin_google_client::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/admin/google-client/login",
            put(talaria_routes_admin::admin::admin_google_client::put_login)
                .fallback(|| async { method_not_allowed("PUT") }),
        )
        .route(
            "/api/admin/instance",
            get(talaria_routes_admin::admin::admin_instance::get)
                .put(talaria_routes_admin::admin::admin_instance::put)
                .post(talaria_routes_admin::admin::admin_instance::post)
                .fallback(|| async { method_not_allowed("GET, PUT, POST") }),
        )
        .route(
            "/api/admin/apps",
            get(talaria_routes_admin::admin::admin_apps::get)
                .put(talaria_routes_admin::admin::admin_apps::put)
                .post(talaria_routes_admin::admin::admin_apps::post)
                .delete(talaria_routes_admin::admin::admin_apps::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, POST, DELETE") }),
        )
        .route(
            "/api/admin/domains",
            get(talaria_routes_admin::admin::admin_domains::get)
                .post(talaria_routes_admin::admin::admin_domains::post)
                .delete(talaria_routes_admin::admin::admin_domains::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/admin/email",
            get(talaria_routes_admin::admin::admin_email::get)
                .put(talaria_routes_admin::admin::admin_email::put)
                .post(talaria_routes_admin::admin::admin_email::post)
                .fallback(|| async { method_not_allowed("GET, PUT, POST") }),
        )
        .route(
            "/api/admin/encryption",
            get(talaria_routes_admin::admin::admin_encryption::get)
                .post(talaria_routes_admin::admin::admin_encryption::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/admin/guardrails",
            get(talaria_routes_admin::admin::admin_guardrails::get)
                .put(talaria_routes_admin::admin::admin_guardrails::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/invites",
            get(talaria_routes_admin::admin::admin_invites::get)
                .post(talaria_routes_admin::admin::admin_invites::post)
                .delete(talaria_routes_admin::admin::admin_invites::delete)
                .fallback(|| async { method_not_allowed("GET, POST, DELETE") }),
        )
        .route(
            "/api/admin/judge",
            get(talaria_routes_admin::admin::admin_judge::get)
                .put(talaria_routes_admin::admin::admin_judge::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/updates",
            get(talaria_routes_admin::admin::admin_updates::get)
                .post(talaria_routes_admin::admin::admin_updates::post)
                .put(talaria_routes_admin::admin::admin_updates::put)
                .fallback(|| async { method_not_allowed("GET, POST, PUT") }),
        )
        // The fitness plane — the probe/eval/adversarial battery over the
        // gateway's models, its run engine, and its archive.
        .route(
            "/api/admin/model-fitness",
            get(talaria_routes_admin::admin::admin_model_fitness::get)
                .post(talaria_routes_admin::admin::admin_model_fitness::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/admin/outreach",
            get(talaria_routes_admin::admin::admin_outreach::get)
                .put(talaria_routes_admin::admin::admin_outreach::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/platform-agents",
            get(talaria_routes_admin::admin::admin_platform_agents::get)
                .put(talaria_routes_admin::admin::admin_platform_agents::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/search",
            get(talaria_routes_admin::admin::admin_search::get)
                .put(talaria_routes_admin::admin::admin_search::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/secrets",
            get(talaria_routes_admin::admin::admin_secrets::get)
                .delete(talaria_routes_admin::admin::admin_secrets::delete)
                .fallback(|| async { method_not_allowed("GET, DELETE") }),
        )
        .route(
            "/api/admin/settings",
            get(talaria_routes_admin::admin::admin_settings::get)
                .put(talaria_routes_admin::admin::admin_settings::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/storage",
            get(talaria_routes_admin::admin::admin_storage::get)
                .put(talaria_routes_admin::admin::admin_storage::put)
                .post(talaria_routes_admin::admin::admin_storage::post)
                .fallback(|| async { method_not_allowed("GET, PUT, POST") }),
        )
        .route(
            "/api/admin/users",
            get(talaria_routes_admin::admin::admin_users::get)
                .put(talaria_routes_admin::admin::admin_users::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/workspace-secrets",
            get(talaria_routes_admin::admin::admin_workspace_secrets::get)
                .post(talaria_routes_admin::admin::admin_workspace_secrets::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/admin/permissions",
            get(talaria_routes_admin::admin::admin_permissions::get)
                .put(talaria_routes_admin::admin::admin_permissions::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/admin/model-roles",
            get(talaria_routes_admin::admin::admin_model_roles::get)
                .put(talaria_routes_admin::admin::admin_model_roles::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        // The retrieval console — the route that kicks the rag-backfill and
        // rag-reindex runs and reads their projections.
        .route(
            "/api/admin/rag",
            get(talaria_routes_admin::admin::admin_rag::get)
                .put(talaria_routes_admin::admin::admin_rag::put)
                .post(talaria_routes_admin::admin::admin_rag::post)
                .fallback(|| async { method_not_allowed("GET, POST, PUT") }),
        )
        // The rag family proper: the collection registry (list/create, then
        // one collection's bindings/delete) and the search the MCP
        // search_knowledge tool rides. Crossed together — the registry is
        // what the search resolves principals against.
        .route(
            "/api/rag/collections",
            get(talaria_routes_knowledge::knowledge::rag_collections::get)
                .post(talaria_routes_knowledge::knowledge::rag_collections::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        // The knowledgebase plane: folders (spaces), their doc trees, the
        // docs themselves (with comments/backlinks/move/live-presence
        // sub-routes), full-text search, and the two no-auth public slug
        // reads. One family under /api/kb — the whole tree crossed together.
        .route(
            "/api/kb/spaces",
            get(talaria_routes_knowledge::knowledge::kb_spaces::get)
                .post(talaria_routes_knowledge::knowledge::kb_spaces::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/kb/spaces/{id}",
            get(talaria_routes_knowledge::knowledge::kb_spaces_id::get)
                .put(talaria_routes_knowledge::knowledge::kb_spaces_id::put)
                .delete(talaria_routes_knowledge::knowledge::kb_spaces_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/kb/spaces/{id}/docs",
            get(talaria_routes_knowledge::knowledge::kb_spaces_id_docs::get)
                .post(talaria_routes_knowledge::knowledge::kb_spaces_id_docs::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/kb/docs/{id}",
            get(talaria_routes_knowledge::knowledge::kb_docs_id::get)
                .put(talaria_routes_knowledge::knowledge::kb_docs_id::put)
                .delete(talaria_routes_knowledge::knowledge::kb_docs_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/kb/docs/{id}/comments",
            get(talaria_routes_knowledge::knowledge::kb_docs_id_comments::get)
                .post(talaria_routes_knowledge::knowledge::kb_docs_id_comments::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/kb/docs/{id}/backlinks",
            get(talaria_routes_knowledge::knowledge::kb_docs_id_backlinks::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/kb/docs/{id}/move",
            post(talaria_routes_knowledge::knowledge::kb_docs_id_move::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/kb/docs/{id}/live",
            get(talaria_routes_knowledge::knowledge::kb_docs_id_live::get)
                .put(talaria_routes_knowledge::knowledge::kb_docs_id_live::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/kb/comments/{id}",
            patch(talaria_routes_knowledge::knowledge::kb_comments_id::patch)
                .delete(talaria_routes_knowledge::knowledge::kb_comments_id::delete)
                .fallback(|| async { method_not_allowed("PATCH, DELETE") }),
        )
        .route(
            "/api/kb/search",
            get(talaria_routes_knowledge::knowledge::kb_search::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/kb/public/space/{slug}",
            get(talaria_routes_knowledge::knowledge::kb_public_space::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/kb/public/{slug}",
            get(talaria_routes_knowledge::knowledge::kb_public::get).fallback(|| async { method_not_allowed("GET") }),
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
            get(talaria_routes_knowledge::files::artifacts::get)
                .post(talaria_routes_knowledge::files::artifacts::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/artifacts/for",
            get(talaria_routes_knowledge::files::artifacts_for::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/artifacts/public/{slug}",
            get(talaria_routes_knowledge::files::artifacts_public_slug::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/artifacts/public/{slug}/download",
            get(talaria_routes_knowledge::files::artifacts_public_slug_download::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/artifacts/{id}",
            get(talaria_routes_knowledge::files::artifacts_id::get)
                .put(talaria_routes_knowledge::files::artifacts_id::put)
                .delete(talaria_routes_knowledge::files::artifacts_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/artifacts/{id}/links",
            post(talaria_routes_knowledge::files::artifacts_id_links::post)
                .delete(talaria_routes_knowledge::files::artifacts_id_links::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        .route(
            "/api/artifacts/{id}/duplicate",
            post(talaria_routes_knowledge::files::artifacts_id_duplicate::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/artifacts/{id}/export/google",
            post(talaria_routes_knowledge::files::artifacts_id_export_google::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/artifact-folders",
            get(talaria_routes_knowledge::files::artifact_folders::get)
                .post(talaria_routes_knowledge::files::artifact_folders::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/artifact-folders/{id}/duplicate",
            post(talaria_routes_knowledge::files::artifact_folders_id_duplicate::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/artifact-folders/{id}",
            get(talaria_routes_knowledge::files::artifact_folders_id::get)
                .put(talaria_routes_knowledge::files::artifact_folders_id::put)
                .delete(talaria_routes_knowledge::files::artifact_folders_id::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/uploads",
            // The route's own body cap: without this the extractor's hidden
            // 2 MB default owns the stream and every larger upload dies as a
            // Malformed 400 before read_upload_form can refuse it properly.
            post(talaria_routes_knowledge::files::uploads::post)
                .fallback(|| async { method_not_allowed("POST") })
                .layer(axum::extract::DefaultBodyLimit::max(
                    talaria_uploads::ROUTE_BODY_LIMIT,
                )),
        )
        .route(
            "/api/uploads/{id}",
            get(talaria_routes_knowledge::files::uploads_id::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/rag/collections/{id}",
            axum::routing::delete(talaria_routes_knowledge::knowledge::rag_collections_id::delete)
                .put(talaria_routes_knowledge::knowledge::rag_collections_id::put)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/rag/search",
            post(talaria_routes_knowledge::knowledge::rag_search::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The fleet family. The hire lifecycle (create + the hire queue),
        // the read plane (overview, containers, defs, endpoints, crons), the
        // admin verbs (render, reconcile, federate), the per-agent control
        // surface (lifecycle, crons, secrets), and the fleet-plane pair
        // agents call themselves (register + heartbeat, registered above).
        .route(
            "/api/fleet/create",
            post(talaria_routes_fleet::fleet::fleet_create::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/hires",
            get(talaria_routes_fleet::fleet::fleet_hires::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet",
            get(talaria_routes_fleet::fleet::fleet::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet/containers",
            get(talaria_routes_fleet::fleet::fleet_containers::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet/render",
            post(talaria_routes_fleet::fleet::fleet_render::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/reconcile",
            post(talaria_routes_fleet::fleet::fleet_reconcile::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/federate",
            post(talaria_routes_fleet::fleet::fleet_federate::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/defs",
            get(talaria_routes_fleet::fleet::fleet_defs::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet/resources",
            get(talaria_routes_fleet::fleet::fleet_resources::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet/endpoints",
            get(talaria_routes_fleet::fleet::fleet_endpoints::get)
                .post(talaria_routes_fleet::fleet::fleet_endpoints::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/fleet/endpoints/{id}",
            axum::routing::put(talaria_routes_fleet::fleet::fleet_endpoints_id::put)
                .delete(talaria_routes_fleet::fleet::fleet_endpoints_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/fleet/endpoints/{id}/available",
            get(talaria_routes_fleet::fleet::fleet_endpoints_id_available::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/fleet/crons",
            get(talaria_routes_fleet::fleet::fleet_crons::get)
                .post(talaria_routes_fleet::fleet::fleet_crons::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/fleet/agents/{id}/crons",
            get(talaria_routes_fleet::fleet::fleet_agents_id_crons::get)
                .post(talaria_routes_fleet::fleet::fleet_agents_id_crons::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/fleet/agents/{id}/crons/{jobId}",
            axum::routing::delete(talaria_routes_fleet::fleet::fleet_agents_id_crons_jobid::delete)
                .put(talaria_routes_fleet::fleet::fleet_agents_id_crons_jobid::put)
                .post(talaria_routes_fleet::fleet::fleet_agents_id_crons_jobid::post)
                .fallback(|| async { method_not_allowed("POST, PUT, DELETE") }),
        )
        .route(
            "/api/fleet/agents/{id}/secrets",
            get(talaria_routes_fleet::fleet::fleet_agents_id_secrets::get)
                .put(talaria_routes_fleet::fleet::fleet_agents_id_secrets::put)
                .delete(talaria_routes_fleet::fleet::fleet_agents_id_secrets::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/fleet/agents/{id}/control",
            post(talaria_routes_fleet::fleet::fleet_agents_id_control::post)
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
            get(talaria_routes_workbench::workbench::workbench::get)
                .put(talaria_routes_workbench::workbench::workbench::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/flow",
            get(talaria_routes_workbench::workbench::workbench_flow::get)
                .put(talaria_routes_workbench::workbench::workbench_flow::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/github",
            get(talaria_routes_workbench::workbench::workbench_github::get)
                .put(talaria_routes_workbench::workbench::workbench_github::put)
                .delete(talaria_routes_workbench::workbench::workbench_github::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/workbench/harnesses",
            get(talaria_routes_workbench::workbench::workbench_harnesses::get)
                .put(talaria_routes_workbench::workbench::workbench_harnesses::put)
                .delete(talaria_routes_workbench::workbench::workbench_harnesses::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, DELETE") }),
        )
        .route(
            "/api/workbench/jobs",
            get(talaria_routes_workbench::workbench::workbench_jobs::get)
                .put(talaria_routes_workbench::workbench::workbench_jobs::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/repo-requests",
            get(talaria_routes_workbench::workbench::workbench_repo_requests::get)
                .put(talaria_routes_workbench::workbench::workbench_repo_requests::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/repos/{agentId}",
            get(talaria_routes_workbench::workbench::workbench_repos_agent_id::get)
                .put(talaria_routes_workbench::workbench::workbench_repos_agent_id::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/workbench/env/{*repo}",
            get(talaria_routes_workbench::workbench::workbench_env_repo::get)
                .patch(talaria_routes_workbench::workbench::workbench_env_repo::patch)
                .fallback(|| async { method_not_allowed("GET, PATCH") }),
        )
        // The MCP family — the registry plane: the roster read (agent wire +
        // the fleet's own config), server CRUD with oauth sniffing, the
        // per-user connect surface, the fleet version-edit hook, the
        // marketplace library/icon pair, the admin probe, and the OAuth
        // start/callback pair (the callback is unauthenticated by design —
        // identity is bound into the single-use state row).
        .route(
            "/api/mcp",
            get(talaria_routes_fleet::mcp::mcp::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/servers",
            get(talaria_routes_fleet::mcp::mcp_servers::get)
                .post(talaria_routes_fleet::mcp::mcp_servers::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/mcp/servers/{id}",
            put(talaria_routes_fleet::mcp::mcp_servers_id::put)
                .delete(talaria_routes_fleet::mcp::mcp_servers_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        .route(
            "/api/me/mcp",
            get(talaria_routes_integrations::account::me_mcp::get)
                .put(talaria_routes_integrations::account::me_mcp::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/fleet/defs/{id}/mcp",
            post(talaria_routes_fleet::fleet::fleet_defs_id_mcp::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/mcp/library",
            get(talaria_routes_fleet::mcp::mcp_library::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/icon",
            get(talaria_routes_fleet::mcp::mcp_icon::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/test",
            post(talaria_routes_fleet::mcp::mcp_test::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/mcp/oauth/start",
            get(talaria_routes_fleet::mcp::mcp_oauth_start::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/mcp/oauth/callback",
            get(talaria_routes_fleet::mcp::mcp_oauth_callback::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/llm/v1/chat/completions",
            post(talaria_routes_fleet::llm::llm_chat::post).fallback(|| async { method_not_allowed("POST") }),
        )
        // The SSE attach points (realtime.ts's streams): a watch stream's
        // legitimate lifetime is the client's, not a handler's.
        .route(
            "/api/runs/{id}/events",
            get(talaria_routes_fleet::agents::runs_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/runs/{id}/watch",
            get(talaria_routes_fleet::agents::runs_watch::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/agents/tool-events",
            post(talaria_routes_fleet::agents::tool_events::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/me/events",
            get(talaria_routes_integrations::account::me_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // A channel's live events — the multiplayer-chat SSE stream.
        .route(
            "/api/channels/{id}/events",
            get(talaria_routes_comms::comms::channels_id_events::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The MCP gateway — both verbs. A tools/call may legitimately hold
        // for the upstream's own 120s timeout, and the GET is a streamable-
        // HTTP notification stream whose lifetime is the client's.
        .route(
            "/api/mcp/gw/{server}",
            post(talaria_routes_fleet::mcp::mcp_gw_server::post)
                .get(talaria_routes_fleet::mcp::mcp_gw_server::get)
                .fallback(|| async { method_not_allowed("POST, GET") }),
        )
        // The Inbox panel's command run — named SSE events for one assistant
        // turn. The turn's lifetime is the model's, and the Inbox lock rides
        // inside the stream task.
        .route(
            "/api/inbox/focus/command",
            post(talaria_routes_comms::inbox::inbox_focus_command::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        // ── The remaining singles (R23) ────────────────────────────────────
        // The instance identity beacon, the derived-alerts read, and the
        // Studio's Suggested queue with its status verb.
        .route(
            "/api/well-known/talaria-instance",
            get(talaria_routes_workbench::system::well_known_talaria_instance::get)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/alerts",
            get(talaria_routes_comms::activity::alerts::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/gaps",
            get(talaria_routes_fleet::agents::gaps::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/gaps/{id}",
            axum::routing::put(talaria_routes_fleet::agents::gaps_id::put)
                .fallback(|| async { method_not_allowed("PUT") }),
        )
        // Home/Today, and the public /join invite lookup (dual-counter rate
        // limited, same shape as login).
        .route(
            "/api/home",
            get(talaria_routes_comms::activity::home::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/join",
            get(talaria_routes_integrations::account::join::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // The agent-memory surface (admin or the assistant's owner), the
        // live web-search door both agents and sessions reach, and the
        // org's template library with its one-template verbs.
        .route(
            "/api/memory/{id}",
            get(talaria_routes_knowledge::knowledge::memory_id::get)
                .put(talaria_routes_knowledge::knowledge::memory_id::put)
                .fallback(|| async { method_not_allowed("GET, PUT") }),
        )
        .route(
            "/api/search",
            post(talaria_routes_knowledge::knowledge::search::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/templates",
            get(talaria_routes_knowledge::knowledge::templates::get)
                .post(talaria_routes_knowledge::knowledge::templates::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/templates/{id}",
            axum::routing::put(talaria_routes_knowledge::knowledge::templates_id::put)
                .delete(talaria_routes_knowledge::knowledge::templates_id::delete)
                .fallback(|| async { method_not_allowed("PUT, DELETE") }),
        )
        // The org's skill library: the owner index with edit flags, and the
        // one-skill verbs (rename/copy/move ride POST).
        .route(
            "/api/skills",
            get(talaria_routes_fleet::agents::skills::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/skills/{owner}/{name}",
            get(talaria_routes_fleet::agents::skills_owner_name::get)
                .put(talaria_routes_fleet::agents::skills_owner_name::put)
                .post(talaria_routes_fleet::agents::skills_owner_name::post)
                .delete(talaria_routes_fleet::agents::skills_owner_name::delete)
                .fallback(|| async { method_not_allowed("GET, PUT, POST, DELETE") }),
        )
        // The skills marketplace (Hermes Atlas's ranked catalog): the list,
        // one repo's discovered skills, and the per-owner install. Static
        // "marketplace" segments outrank the {owner}/{name} captures above.
        .route(
            "/api/skills/marketplace",
            get(talaria_routes_fleet::agents::skills_marketplace::get_list)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/skills/marketplace/detail",
            get(talaria_routes_fleet::agents::skills_marketplace::get_detail)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/skills/marketplace/install",
            post(talaria_routes_fleet::agents::skills_marketplace::post_install)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        // The agent surface: media reads/writes scoped by model, the two
        // honesty-loop reports (gap, problem), the plain-language
        // message-user door, and the self-introspection probe.
        .route(
            "/api/agent-media/{model}",
            get(talaria_routes_knowledge::files::agent_media_model::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/agent-media/{model}/save",
            post(talaria_routes_knowledge::files::agent_media_model_save::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/agent/whoami",
            get(talaria_routes_fleet::agents::agent_whoami::get).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/api/agent/chips",
            post(talaria_routes_comms::comms::chips_expose::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/agent/gap",
            post(talaria_routes_fleet::agents::agent_gap::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/agent/problem",
            post(talaria_routes_fleet::agents::agent_problem::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/agent/message-user",
            post(talaria_routes_fleet::agents::agent_message_user::post)
                .fallback(|| async { method_not_allowed("POST") }),
        )
        // The Muse, the image describer, the personal assistant, and the
        // local-inference observability plane.
        .route(
            "/api/muse",
            post(talaria_routes_fleet::agents::muse::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/vision/describe",
            post(talaria_routes_fleet::agents::vision_describe::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/me/assistant",
            get(talaria_routes_integrations::account::me_assistant::get)
                .post(talaria_routes_integrations::account::me_assistant::post)
                .patch(talaria_routes_integrations::account::me_assistant::patch)
                .fallback(|| async { method_not_allowed("GET, POST, PATCH") }),
        )
        .route(
            "/api/inference",
            get(talaria_routes_fleet::models::inference::get).fallback(|| async { method_not_allowed("GET") }),
        )
        // Multiplayer plans: the living document and the member roster.
        .route(
            "/api/plans/{id}/doc",
            get(talaria_routes_boards::plans::plans_id_doc::get)
                .post(talaria_routes_boards::plans::plans_id_doc::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .route(
            "/api/plans/{id}/members",
            get(talaria_routes_boards::plans::plans_id_members::get)
                .post(talaria_routes_boards::plans::plans_id_members::post)
                .put(talaria_routes_boards::plans::plans_id_members::put)
                .delete(talaria_routes_boards::plans::plans_id_members::delete)
                .fallback(|| async { method_not_allowed("GET, POST, PUT, DELETE") }),
        )
        .route(
            "/api/plans/{id}/teams",
            post(talaria_routes_boards::plans::plans_id_teams::post)
                .delete(talaria_routes_boards::plans::plans_id_teams::delete)
                .fallback(|| async { method_not_allowed("POST, DELETE") }),
        )
        // The fleet defs detail trio: identity PATCH, the versioned edit,
        // and the version history.
        .route(
            "/api/fleet/defs/{id}",
            axum::routing::patch(talaria_routes_fleet::fleet::fleet_defs_id::patch)
                .fallback(|| async { method_not_allowed("PATCH") }),
        )
        .route(
            "/api/fleet/defs/{id}/edit",
            post(talaria_routes_fleet::fleet::fleet_defs_id_edit::post).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/api/fleet/defs/{id}/versions",
            get(talaria_routes_fleet::fleet::fleet_defs_id_versions::get)
                .post(talaria_routes_fleet::fleet::fleet_defs_id_versions::post)
                .fallback(|| async { method_not_allowed("GET, POST") }),
        )
        .fallback(api_not_found)
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .with_state(state)
}
