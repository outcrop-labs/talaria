// /api/workflows — port of ui/src/routes/api/workflows.ts. GET lists every
// workflow for any signed-in member (they ground what agents will be told —
// deliberately unscoped); POST is agents.manage. The body schema is shared
// with workflows_id exactly the way TS shares it ($id imports Body from this
// file): validate_workflow_body is that export, post=false is Body.partial()
// .extend({enabled}).

use crate::body::{
    array_msg, array_too_big_msg, as_object, boolean_member, object_msg,
    optional_max_string_member, optional_string_array_member, optional_uuid_array_member, parse,
    string_member, trimmed_string_member, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_perm, require_user};
use crate::state::AppState;
use crate::workflows::{create_workflow, list_workflows};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

pub(crate) struct MatchRules {
    labels: Option<Vec<String>>,
    boards: Option<Vec<String>>,
    keywords: Option<Vec<String>>,
}

pub(crate) struct Toolkit {
    server: String,
    tools: Option<Vec<String>>,
}

/// The validated body — every field Option<"present"> so the same run serves
/// POST (name required) and the PUT patch (everything optional).
pub(crate) struct WorkflowBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub rules: Option<MatchRules>,
    pub skills: Option<Vec<String>>,
    pub toolkits: Option<Vec<Toolkit>>,
    pub enabled: Option<bool>,
}

/// workflows.ts's Body, checks in zod's schema order: name, description,
/// match (labels → boards → keywords), skills, toolkits, enabled. Unknown
/// keys are stripped at every level — zod objects are not strict.
pub(crate) fn validate_workflow_body(
    obj: &serde_json::Map<String, Value>,
    post: bool,
) -> Result<WorkflowBody, String> {
    let name = match obj.get("name") {
        None if !post => None,
        _ => Some(trimmed_string_member(obj, "name", 1, 80)?),
    };
    let description = optional_max_string_member(obj, "description", 500)?;
    let rules = match obj.get("match") {
        None => None,
        Some(v) => {
            let m = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
            let labels = optional_string_array_member(m, "labels", 1, 60, 30)?;
            let boards = optional_uuid_array_member(m, "boards", 30)?;
            let keywords = optional_string_array_member(m, "keywords", 1, 80, 30)?;
            Some(MatchRules {
                labels,
                boards,
                keywords,
            })
        }
    };
    let skills = optional_string_array_member(obj, "skills", 1, 80, 20)?;
    let toolkits = match obj.get("toolkits") {
        None => None,
        Some(v) => {
            let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
            let mut out = Vec::with_capacity(arr.len());
            for el in arr {
                let t = el
                    .as_object()
                    .ok_or_else(|| object_msg(zod_type_name(el)))?;
                let server = string_member(t, "server", 1, 80)?;
                let tools = optional_string_array_member(t, "tools", 1, 120, 60)?;
                out.push(Toolkit { server, tools });
            }
            if arr.len() > 20 {
                return Err(array_too_big_msg(20));
            }
            Some(out)
        }
    };
    let enabled = match obj.get("enabled") {
        None => None,
        Some(_) if post => None, // Body has no enabled — stripped, not an error
        Some(_) => Some(boolean_member(obj, "enabled")?),
    };
    Ok(WorkflowBody {
        name,
        description,
        rules,
        skills,
        toolkits,
        enabled,
    })
}

/// The stored jsonb for a PRESENT match: only facets that were PRESENT (zod
/// strips absent optionals — they never become null). None in, None out, so
/// the PUT patch leaves absent fields untouched; POST substitutes {} / [].
pub(crate) fn match_json(r: &Option<MatchRules>) -> Option<Value> {
    let r = r.as_ref()?;
    let mut m = serde_json::Map::new();
    if let Some(v) = &r.labels {
        m.insert("labels".into(), json!(v));
    }
    if let Some(v) = &r.boards {
        m.insert("boards".into(), json!(v));
    }
    if let Some(v) = &r.keywords {
        m.insert("keywords".into(), json!(v));
    }
    Some(Value::Object(m))
}

pub(crate) fn toolkits_json(t: &Option<Vec<Toolkit>>) -> Option<Value> {
    let list = t.as_ref()?;
    Some(Value::Array(
        list.iter()
            .map(|tk| {
                let mut m = serde_json::Map::new();
                m.insert("server".into(), json!(tk.server));
                if let Some(tools) = &tk.tools {
                    m.insert("tools".into(), json!(tools));
                }
                Value::Object(m)
            })
            .collect(),
    ))
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_user(&state, &headers).await {
        return gate;
    }
    let workflows = match list_workflows(&state.pg).await {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("[workflows] list failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "workflows": workflows })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_perm(&state, &headers, "agents.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match validate_workflow_body(obj, true) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = body.name.expect("post requires name");
    let skills = json!(body.skills.unwrap_or_default());
    let rules = match_json(&body.rules).unwrap_or_else(|| json!({}));
    let toolkits = toolkits_json(&body.toolkits).unwrap_or_else(|| json!([]));
    let workflow = match create_workflow(
        &state.pg,
        &name,
        body.description.as_deref().unwrap_or(""),
        &rules,
        &skills,
        &toolkits,
        &actor_of(&user),
    )
    .await
    {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("[workflows] create failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "workflow": workflow })).into_response()
}
