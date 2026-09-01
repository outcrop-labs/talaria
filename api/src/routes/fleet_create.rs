// /api/fleet/create — port of ui/src/routes/api/fleet.create.ts.
// POST → start HIRING a new agent. The work — create the def, write v1 and
// any starter skills, render the fleet, boot the container, wait out the
// healthcheck — is a durable `agent-hire` run, not this request: a boot runs
// to minutes on a cold pull, and a POST is a promise to stay on the line the
// modal cannot keep. The answer is the hire row; the roster shows the phases
// and the finished agent. Admin.

use crate::body::{
    array_msg, array_too_big_msg, as_object, object_msg, optional_boolean_member,
    optional_max_string_member, optional_uuid_member, parse, string_member, too_big_msg, utf16_len,
    zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::realtime::RealtimeDeps;
use crate::runs::defs::agent_hire::{AgentHireInput, SkillSeed, agent_hire_run};
use crate::runs::run::{EnqueueOptions, enqueue};
use crate::session::require_perm;
use crate::state::AppState;
use crate::work_dispatch::dispatch_deps;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// A `z.string().max(n).nullish()` member: absent and present-null are both
/// None (the route maps either to null in the run input anyway), a present
/// string has NO floor — the empty string is legal and the trim below turns
/// it into null.
fn nullish_max_string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => optional_max_string_member(obj, key, max),
    }
}

/// The skill-name check, zod 4's default message and all — `Invalid string:
/// must match pattern /…/` (probed against the live schema, not composed).
const SKILL_NAME_PATTERN: &str = "^[a-z0-9][a-z0-9._-]*$";

fn skill_member(t: &serde_json::Map<String, Value>) -> Result<SkillSeed, String> {
    let name = string_of(t, "name")?;
    // The regex is the name's ONLY bound (no length) — and a zod check fires
    // in declaration order, so the type gate runs before the pattern.
    static NAME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new("^[a-z0-9][a-z0-9._-]*$").expect("the skill-name pattern compiles")
    });
    if !NAME.is_match(&name) {
        return Err(format!(
            "Invalid string: must match pattern /{SKILL_NAME_PATTERN}/"
        ));
    }
    let content = string_of(t, "content")?;
    if utf16_len(&content) > 100_000 {
        return Err(too_big_msg(100_000));
    }
    Ok(SkillSeed { name, content })
}

/// A required member of a nested zod object: absent is the type error on
/// undefined, present must be a string (no bounds of its own).
fn string_of(t: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    match t.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(v) => Err(format!(
            "Invalid input: expected string, received {}",
            zod_type_name(v)
        )),
        None => Err("Invalid input: expected string, received undefined".into()),
    }
}

/// TS's `x?.trim() || null` — trim, and an empty-after-trim is no value.
fn trimmed(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

struct HireBody {
    slug: String,
    department: String,
    display_name: String,
    /// Raw (≤80, nullish) — the trim-or-null runs when the run input is built.
    role: Option<String>,
    template_id: Option<String>,
    /// Raw (≤200k, optional) — same trim-or-null.
    soul: Option<String>,
    skills: Vec<SkillSeed>,
    start: bool,
}

fn validate_body(obj: &serde_json::Map<String, Value>) -> Result<HireBody, String> {
    let slug = string_member(obj, "slug", 2, 30)?;
    let department = string_member(obj, "department", 2, 40)?;
    let display_name = string_member(obj, "displayName", 1, 60)?;
    let role = nullish_max_string_member(obj, "role", 80)?;
    let template_id = optional_uuid_member(obj, "templateId")?;
    let soul = optional_max_string_member(obj, "soul", 200_000)?;
    let skills = match obj.get("skills") {
        None => Vec::new(),
        Some(v) => {
            let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
            if arr.len() > 5 {
                return Err(array_too_big_msg(5));
            }
            let mut out = Vec::with_capacity(arr.len());
            for el in arr {
                let t = el
                    .as_object()
                    .ok_or_else(|| object_msg(zod_type_name(el)))?;
                out.push(skill_member(t)?);
            }
            out
        }
    };
    let start = optional_boolean_member(obj, "start")?.unwrap_or(true);
    Ok(HireBody {
        slug,
        department,
        display_name,
        role,
        template_id,
        soul,
        skills,
        start,
    })
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
    let body = match validate_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // The one check that stays synchronous: a handle somebody can fix in the
    // open modal. Everything slower or rarer (template missing, bad config)
    // belongs to the run, where its sentence is visible on the roster.
    let taken: Option<(i32,)> = match sqlx::query_as("select 1 from agent_defs where slug = $1")
        .bind(&body.slug)
        .fetch_optional(&state.pg)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[fleet] taken-slug check failed: {e}");
            return thrown_internal_error();
        }
    };
    if taken.is_some() {
        return house_error(
            StatusCode::CONFLICT,
            &format!("an agent with the handle \"{}\" already exists", body.slug),
        );
    }

    // Registration only, exactly like the TS route's def import: a process
    // that enqueues a hire can also be the process a reclaim sweep asks to
    // resume one. actorOf: email, else name, else the id.
    let def = agent_hire_run();
    let actor = crate::session::actor_of(&user);
    let input = AgentHireInput {
        slug: body.slug.clone(),
        department: body.department,
        display_name: body.display_name,
        // `role?.trim() || null` — an empty-after-trim role is no role.
        role: trimmed(body.role.as_deref()),
        template_id: body.template_id,
        soul: trimmed(body.soul.as_deref()),
        skills: body.skills,
        start: body.start,
        actor,
    };

    // The enqueue needs a live Redis for the lease and the publish — the run
    // row IS the feature here, so a start without it fails the start rather
    // than half-happening (same posture as the plan-draft twin).
    let Some(redis) = state.redis().await.ok() else {
        return house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the hire could not be enqueued: redis is unavailable",
        );
    };
    let realtime = RealtimeDeps::publish_only(Some(redis.clone()));
    let deps = dispatch_deps(state.pg.clone(), redis, realtime);
    let input = match serde_json::to_value(input) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[fleet] hire input serialize failed: {e}");
            return thrown_internal_error();
        }
    };
    let row = match enqueue(
        def,
        input,
        EnqueueOptions {
            id: Some(uuid::Uuid::new_v4().to_string()),
            owner_user_id: Some(user.id.clone()),
            subject_type: Some("agent-hire".into()),
            subject_id: Some(body.slug),
            phase: Some("queued".into()),
            // The coexistence bridge: no drive while TS owns the sweep — the
            // row is written and published and the TS sweep drives it; inline
            // drive (TS's own behavior) once this process does.
            start: Some(crate::scheduler::rust_owns_schedule()),
        },
        &deps,
    )
    .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::error!("[fleet] hire enqueue failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({
        "ok": true,
        "hire": { "id": row.id, "state": row.state, "phase": row.phase }
    }))
    .into_response()
}
