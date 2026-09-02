// /api/workbench/harnesses — port of ui/src/routes/api/workbench.harnesses.ts.
// The harness registry. GET → merged definitions with sources (any member —
// grounds the per-agent dropdowns); PUT → register/replace a CUSTOM
// definition (declarative JSON, no code); DELETE ?slug= removes one.
// Builtin/app-shipped entries can be shadowed by slug but never deleted.
//
// The stored definition is the zod OUTPUT object: schema-shape key order,
// absent keys dropped, unknown keys (and unknown nested keys) stripped —
// probed, not assumed. All messages below are probed zod 4 strings.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::body::{
    as_object, enum_member, object_msg, optional_max_string_member, optional_string_array_member,
    parse, record_msg, string_member, string_value_member, too_big_msg, utf16_len, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_perm, require_user};
use crate::state::AppState;
use crate::workbench::harnesses::{
    delete_custom_harness, list_harness_defs, upsert_custom_harness,
};

const SLUG_PATTERN: &str = "^[a-z0-9][a-z0-9-]*$";
const FILENAME_PATTERN: &str = "^[a-z0-9][a-z0-9.-]*$";

fn slug_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn filename_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'.' || *c == b'-')
}

async fn harnesses_json(pg: &sqlx::PgPool) -> Response {
    match list_harness_defs(pg).await {
        Ok(list) => Json(json!({
            "harnesses": list.iter().map(|h| h.wire()).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => {
            tracing::error!("[workbench/harnesses] registry read failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_user(&state, &headers).await {
        return gate;
    }
    harnesses_json(&state.pg).await
}

/// The Definition schema → the stored output object. Field checks run in
/// schema order; each present member is written into `out` in schema order.
fn definition_of(obj: &Map<String, Value>) -> Result<Value, String> {
    let mut out = Map::new();
    // slug: the regex is declared BEFORE the max, so a bad long slug answers
    // the pattern sentence.
    let slug = string_value_member(obj, "slug")?;
    if !slug_ok(&slug) {
        return Err(format!(
            "Invalid string: must match pattern /{SLUG_PATTERN}/"
        ));
    }
    if utf16_len(&slug) > 40 {
        return Err(too_big_msg(40));
    }
    out.insert("slug".into(), json!(slug));
    out.insert("label".into(), json!(string_member(obj, "label", 1, 60)?));
    if let Some(d) = optional_max_string_member(obj, "description", 300)? {
        out.insert("description".into(), json!(d));
    }
    // auth: literal 'gateway' | { provider 1..40, envVar 1..60 }. The union
    // answers "Invalid input" for anything that isn't the literal or a
    // BOTH-keys-present object of strings — an object with one good string
    // and one missing/non-string key falls to the union message. Only
    // in-bounds failures of a structurally-matching object surface as the
    // member's own message (probed).
    let auth = obj.get("auth").ok_or_else(|| "Invalid input".to_string())?;
    match auth {
        Value::String(s) if s == "gateway" => {
            out.insert("auth".into(), json!("gateway"));
        }
        Value::Object(m) => {
            let (Some(p), Some(e)) = (m.get("provider"), m.get("envVar")) else {
                return Err("Invalid input".into());
            };
            let (Value::String(p), Value::String(e)) = (p, e) else {
                return Err("Invalid input".into());
            };
            if utf16_len(p) < 1 {
                return Err("Too small: expected string to have >=1 characters".into());
            }
            if utf16_len(p) > 40 {
                return Err(too_big_msg(40));
            }
            if utf16_len(e) < 1 {
                return Err("Too small: expected string to have >=1 characters".into());
            }
            if utf16_len(e) > 60 {
                return Err(too_big_msg(60));
            }
            out.insert("auth".into(), json!({ "provider": p, "envVar": e }));
        }
        _ => return Err("Invalid input".into()),
    }
    // env: a record whose KEYS are capped too — an over-long key draws zod's
    // bare "Invalid key in record", not the bound sentence.
    if let Some(env) = obj.get("env") {
        let m = env
            .as_object()
            .ok_or_else(|| record_msg(zod_type_name(env)))?;
        let mut stored = Map::new();
        for (k, v) in m {
            let s = v.as_str().ok_or_else(|| {
                format!(
                    "Invalid input: expected string, received {}",
                    zod_type_name(v)
                )
            })?;
            if utf16_len(k) > 60 {
                return Err("Invalid key in record".into());
            }
            if utf16_len(s) > 300 {
                return Err(too_big_msg(300));
            }
            stored.insert(k.clone(), json!(s));
        }
        out.insert("env".into(), Value::Object(stored));
    }
    if let Some(p) = optional_max_string_member(obj, "modelPrefix", 40)? {
        out.insert("modelPrefix".into(), json!(p));
    }
    out.insert(
        "invoke".into(),
        json!(string_member(obj, "invoke", 1, 500)?),
    );
    if let Some(j) = optional_max_string_member(obj, "jsonInvoke", 500)? {
        out.insert("jsonInvoke".into(), json!(j));
    }
    if let Some(v) = obj.get("mcpServe") {
        let m = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
        let mut stored = Map::new();
        stored.insert(
            "command".into(),
            json!(string_member(m, "command", 1, 120)?),
        );
        if let Some(args) = optional_string_array_member(m, "args", 0, 120, 10)? {
            stored.insert("args".into(), json!(args));
        }
        out.insert("mcpServe".into(), Value::Object(stored));
    }
    if let Some(v) = obj.get("mcpConfig") {
        let m = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
        let format = enum_member(m, "format", &["claude-json", "opencode-json"])?;
        let filename = string_value_member(m, "filename")?;
        if !filename_ok(&filename) {
            return Err(format!(
                "Invalid string: must match pattern /{FILENAME_PATTERN}/"
            ));
        }
        if utf16_len(&filename) > 60 {
            return Err(too_big_msg(60));
        }
        out.insert(
            "mcpConfig".into(),
            json!({ "format": format, "filename": filename }),
        );
    }
    out.insert("guide".into(), json!(string_member(obj, "guide", 1, 2000)?));
    if let Some(v) = obj.get("install") {
        let m = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
        let mut stored = Map::new();
        if let Some(npm) = optional_string_array_member(m, "npm", 0, 120, 10)? {
            stored.insert("npm".into(), json!(npm));
        }
        if let Some(commands) = optional_string_array_member(m, "commands", 0, 300, 10)? {
            stored.insert("commands".into(), json!(commands));
        }
        if let Some(notes) = optional_max_string_member(m, "notes", 500)? {
            stored.insert("notes".into(), json!(notes));
        }
        out.insert("install".into(), Value::Object(stored));
    }
    Ok(Value::Object(out))
}

pub async fn put(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let user = match require_perm(&state, &headers, "agents.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let definition = match definition_of(obj) {
        Ok(d) => d,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let slug = definition["slug"].as_str().unwrap_or_default().to_string();
    if let Err(e) = upsert_custom_harness(&state.pg, &slug, &definition, &actor_of(&user)).await {
        tracing::error!("[workbench/harnesses] definition write failed: {e}");
        return thrown_internal_error();
    }
    harnesses_json(&state.pg).await
}

pub async fn delete(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    // searchParams.get('slug') — absent and bare-'?slug' both null.
    let slug = uri
        .query()
        .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("slug=")));
    let Some(slug) = slug else {
        return house_error(StatusCode::BAD_REQUEST, "slug required");
    };
    if let Err(e) = delete_custom_harness(&state.pg, slug).await {
        tracing::error!("[workbench/harnesses] definition delete failed: {e}");
        return thrown_internal_error();
    }
    harnesses_json(&state.pg).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::enum_msg;

    fn base() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("slug".into(), json!("probe"));
        m.insert("label".into(), json!("Probe"));
        m.insert("auth".into(), json!("gateway"));
        m.insert("invoke".into(), json!("run probe"));
        m.insert("guide".into(), json!("how to probe"));
        m
    }

    #[test]
    fn definition_builds_schema_order_and_strips() {
        // Input in REVERSE order with unknown keys at both levels: output is
        // schema order, unknowns gone.
        let mut m = base();
        m.insert("zzz".into(), json!(1));
        m.insert(
            "mcpServe".into(),
            json!({ "command": "c", "args": ["a"], "extra": 1 }),
        );
        let d = definition_of(&m).unwrap();
        let keys: Vec<&str> = d.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec!["slug", "label", "auth", "invoke", "mcpServe", "guide"]
        );
        let serve = d["mcpServe"].as_object().unwrap();
        assert_eq!(
            serve.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["command", "args"]
        );
    }

    #[test]
    fn slug_regex_before_max() {
        let mut m = base();
        m.insert("slug".into(), json!("Bad_Slug"));
        assert_eq!(
            definition_of(&m).unwrap_err(),
            format!("Invalid string: must match pattern /{SLUG_PATTERN}/")
        );
        m.insert("slug".into(), json!("A".repeat(41)));
        assert_eq!(
            definition_of(&m).unwrap_err(),
            format!("Invalid string: must match pattern /{SLUG_PATTERN}/")
        );
        m.insert("slug".into(), json!("a".repeat(41)));
        assert_eq!(definition_of(&m).unwrap_err(), too_big_msg(40));
    }

    #[test]
    fn auth_union_messages() {
        let mut m = base();
        for bad in [
            json!(5),
            json!("pat"),
            json!(null),
            json!(["gateway"]),
            json!({ "provider": "p" }),
            json!({ "provider": 5, "envVar": "e" }),
        ] {
            m.insert("auth".into(), bad);
            assert_eq!(definition_of(&m).unwrap_err(), "Invalid input");
        }
        m.insert("auth".into(), json!({ "provider": "", "envVar": "e" }));
        assert_eq!(
            definition_of(&m).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
        m.insert(
            "auth".into(),
            json!({ "provider": "p".repeat(41), "envVar": "e" }),
        );
        assert_eq!(definition_of(&m).unwrap_err(), too_big_msg(40));
        m.insert(
            "auth".into(),
            json!({ "provider": "p", "envVar": "e".repeat(61) }),
        );
        assert_eq!(definition_of(&m).unwrap_err(), too_big_msg(60));
        m.insert(
            "auth".into(),
            json!({ "provider": "p", "envVar": "e", "zzz": 1 }),
        );
        let d = definition_of(&m).unwrap();
        assert_eq!(d["auth"], json!({ "provider": "p", "envVar": "e" }));
    }

    #[test]
    fn env_record_key_cap_is_its_own_sentence() {
        let mut m = base();
        m.insert("env".into(), json!({ "A": 5 }));
        assert_eq!(
            definition_of(&m).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        // json! can't compute keys — build the record by hand.
        let mut over = Map::new();
        over.insert("k".repeat(61), json!("v"));
        m.insert("env".into(), Value::Object(over));
        assert_eq!(definition_of(&m).unwrap_err(), "Invalid key in record");
        m.insert("env".into(), json!({ "A": "v".repeat(301) }));
        assert_eq!(definition_of(&m).unwrap_err(), too_big_msg(300));
        m.insert("env".into(), json!("nope"));
        assert_eq!(definition_of(&m).unwrap_err(), record_msg("string"));
    }

    #[test]
    fn mcpconfig_checks_format_then_filename_pattern() {
        let mut m = base();
        m.insert(
            "mcpConfig".into(),
            json!({ "format": "nope", "filename": "mcp.json" }),
        );
        assert_eq!(
            definition_of(&m).unwrap_err(),
            enum_msg(&["claude-json", "opencode-json"])
        );
        m.insert(
            "mcpConfig".into(),
            json!({ "format": "claude-json", "filename": "MCP!" }),
        );
        assert_eq!(
            definition_of(&m).unwrap_err(),
            format!("Invalid string: must match pattern /{FILENAME_PATTERN}/")
        );
        // filename missing: format passes, the missing member answers.
        m.insert("mcpConfig".into(), json!({ "format": "claude-json" }));
        assert_eq!(
            definition_of(&m).unwrap_err(),
            "Invalid input: expected string, received undefined"
        );
    }

    #[test]
    fn args_over_ten_items() {
        let mut m = base();
        let args: Vec<Value> = (0..11).map(|_| json!("a")).collect();
        m.insert("mcpServe".into(), json!({ "command": "c", "args": args }));
        assert_eq!(
            definition_of(&m).unwrap_err(),
            crate::body::array_too_big_msg(10)
        );
    }
}
