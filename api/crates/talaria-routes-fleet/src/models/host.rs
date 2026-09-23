// doc: The machine this api can see: CPU, load, memory, swap, disk per
// doc: mount, and the processes using them. Admins and Observability
// doc: grantees. A container without the host mounts says so instead of
// doc: reporting its own cgroup.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use talaria_host_metrics::HostSnapshot;
use talaria_session::require_view;
use talaria_state::AppState;

#[derive(Serialize)]
struct HostBody {
    host: HostSnapshot,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_view(&state, &headers, "/observability").await?;
    let host = talaria_host_metrics::snapshot().await;
    Ok(Json(HostBody { host }).into_response())
}
