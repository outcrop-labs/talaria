// /api/secrets — port of ui/src/routes/api/secrets.ts.
//
// WORKING SECRETS — the ones a PERSON needs back. Not admin, and that is the
// entire reason this exists: somebody wiring up a staging integration has a
// key their two teammates also need this week, and if the answer is "ask an
// admin", the real answer becomes a Slack thread. Placement is
// artifact-shaped (folders, sharing, a title) but storage is not — the value
// never enters the artifact pipeline, which indexes, exports and serves
// bodies. `revealable = true` is what makes this a different noun from the
// admin vault, and nothing ever flips it after creation.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    array_msg, array_too_big_msg, array_too_small_msg, as_object, nullable_uuid_member,
    nullish_max_string_member, object_msg, optional_string_array_member,
    optional_uuid_array_member, optional_uuid_member, parse, string_member, string_value_member,
    too_big_msg, utf16_substr, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::workspace_secrets::{
    CreateSecret, SecretEntryInput, create_secret_doc, delete_secret_doc, get_secret_doc,
    list_secrets_for_user, move_secret_to_folder,
};

/// One entry of the create body. `key`'s regex sits BEFORE the max in the
/// schema chain, so a long key of bad characters answers the regex sentence
/// and only a long key of good ones answers the bound.
pub(crate) struct EntryBody {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) value: String,
}

pub(crate) fn parse_entry(el: &Value) -> Result<EntryBody, String> {
    let obj = el
        .as_object()
        .ok_or_else(|| object_msg(zod_type_name(el)))?;
    let key = string_value_member(obj, "key")?;
    if !entry_key_ok(&key) {
        return Err("lowercase letters, digits, - and _".into());
    }
    if crate::body::utf16_len(&key) > 40 {
        return Err(too_big_msg(40));
    }
    Ok(EntryBody {
        key,
        label: string_member(obj, "label", 1, 60)?,
        value: string_member(obj, "value", 1, 20_000)?,
    })
}

/// `^[a-z0-9][a-z0-9_-]*$` — first character may not be - or _.
pub(crate) fn entry_key_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

struct CreateBody {
    title: String,
    entries: Vec<EntryBody>,
    note: Option<String>,
    folder_id: Option<String>,
    readers: Option<Vec<String>>,
    grant_to: Option<Vec<String>>,
    allowed_hosts: Option<Vec<String>>,
    expires_at: Option<String>,
}

fn parse_create(obj: &serde_json::Map<String, Value>) -> Result<CreateBody, String> {
    let title = string_member(obj, "title", 1, 80)?;
    // Elements before length: zod parses each item, then runs the bounds.
    let v = obj.get("entries").ok_or_else(|| array_msg("undefined"))?;
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut entries = Vec::with_capacity(arr.len());
    for el in arr {
        entries.push(parse_entry(el)?);
    }
    if arr.is_empty() {
        return Err(array_too_small_msg(1));
    }
    if arr.len() > 20 {
        return Err(array_too_big_msg(20));
    }
    Ok(CreateBody {
        title,
        entries,
        note: nullish_max_string_member(obj, "note", 400)?,
        folder_id: optional_uuid_member(obj, "folderId")?,
        readers: optional_uuid_array_member(obj, "readers", 50)?,
        grant_to: optional_string_array_member(obj, "grantTo", 0, 120, 50)?,
        allowed_hosts: optional_string_array_member(obj, "allowedHosts", 0, 253, 30)?,
        expires_at: nullish_max_string_member(obj, "expiresAt", 40)?,
    })
}

/// A slug a person never types. Working secrets are addressed by title, so
/// the name only has to be unique, handle-safe, and not guessable —
/// guessable would let another agent ask for it by name and rely on the
/// grant check as the only defence.
fn slug_for(title: &str) -> String {
    static NON_ALNUM: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new("[^a-z0-9]+").unwrap());
    static EDGE_DASHES: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new("^-+|-+$").unwrap());
    let lowered = title.to_lowercase();
    let dashed = NON_ALNUM.replace_all(&lowered, "-");
    let trimmed = EDGE_DASHES.replace_all(&dashed, "");
    let stem = utf16_substr(&trimmed, 0, 24);
    let stem = if stem.is_empty() { "secret" } else { stem };
    format!("{stem}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // Mine, plus what has been shared with me. Keys and labels; no values — a
    // LISTING never carries one, only an explicit reveal does.
    match list_secrets_for_user(&state.pg, &user.id).await {
        Ok(secrets) => Json(json!({ "secrets": secrets })).into_response(),
        Err(e) => {
            tracing::error!("[secrets] list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match parse_create(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // A failed secretbox is the create failing: the value cannot be sealed,
    // and the caller sees the route's own sentence, not the box's error.
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[secrets] create failed: {e}");
            return house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not save that secret — see server logs",
            );
        }
    };

    let doc = create_secret_doc(
        &state.pg,
        &sb,
        &CreateSecret {
            name: slug_for(&body.title),
            title: body.title.clone(),
            entries: body
                .entries
                .iter()
                .map(|e| SecretEntryInput {
                    key: e.key.clone(),
                    label: e.label.clone(),
                    value: e.value.clone(),
                })
                .collect(),
            kind: None,
            note: body.note.clone(),
            created_by: Some(actor_of(&user)),
            expires_at: body.expires_at.clone(),
            uses: None,
            grant_to: body.grant_to.clone(),
            allowed_hosts: body.allowed_hosts.clone(),
            // THE FLAG THAT MAKES IT A DIFFERENT NOUN. Set only here — the
            // admin route never sets it, so an agent credential stays
            // unreadable forever.
            revealable: true,
            owner_user_id: Some(user.id.clone()),
            folder_id: body.folder_id.clone(),
            readers: body.readers.clone(),
        },
    )
    .await;
    let doc = match doc {
        Ok(d) => d,
        Err(e) => {
            // Never echo the engine's error: it names the values it was
            // handed.
            tracing::error!("[secrets] create failed: {e}");
            return house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not save that secret — see server logs",
            );
        }
    };

    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "secrets.save",
            target_type: "secret",
            target_id: Some(&doc.name),
            target_label: Some(&doc.title),
            before: None,
            after: Some(json!({
                "entries": doc.entries,
                "readers": doc.readers,
                "grants": doc.grants,
                "allowedHosts": doc.allowed_hosts,
            })),
        },
    )
    .await;
    Json(json!({ "secret": doc })).into_response()
}

// Move it into (or out of) a folder. Owner-only, like every other change to
// where a credential lives.
pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // name is max-only; folderId is REQUIRED but may be null.
    let name = match string_member(obj, "name", 0, 80) {
        Ok(n) => n,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let folder_id = match nullable_uuid_member(obj, "folderId") {
        Ok(f) => f,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // The TS route passes no isAdmin — owner-only, even for admins, by design.
    let moved = match move_secret_to_folder(&state.pg, &name, folder_id.as_deref(), &user.id, false)
        .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[secrets] move failed: {e}");
            return thrown_internal_error();
        }
    };
    if !moved {
        return house_error(StatusCode::FORBIDDEN, "not yours to move");
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "secrets.move",
            target_type: "secret",
            target_id: Some(&name),
            target_label: None,
            before: None,
            after: Some(json!({ "folderId": folder_id })),
        },
    )
    .await;
    match get_secret_doc(&state.pg, &name).await {
        Ok(doc) => Json(json!({ "secret": doc })).into_response(),
        Err(e) => {
            tracing::error!("[secrets] move re-read failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 0, 80) {
        Ok(n) => n,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let doc = match get_secret_doc(&state.pg, &name).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[secrets] delete read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(doc) = doc else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    // OWNER ONLY. A reader was let in to USE the credential, not to destroy
    // it for everyone else — and an admin deleting one goes through the
    // admin route, where the act is recorded as administration.
    if !doc.revealable || doc.owner_user_id.as_deref() != Some(user.id.as_str()) {
        return house_error(StatusCode::FORBIDDEN, "not yours to delete");
    }
    if let Err(e) = delete_secret_doc(&state.pg, &name).await {
        tracing::error!("[secrets] delete failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "secrets.delete",
            target_type: "secret",
            target_id: Some(&name),
            target_label: Some(&doc.title),
            before: None,
            after: None,
        },
    )
    .await;
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_for_slugifies_and_suffices() {
        // Lowercase, squeeze, trim, 24-char cap, 8-hex suffix.
        let s = slug_for("Staging — deploy keys!");
        assert!(s.starts_with("staging-deploy-keys-"), "{s}");
        assert_eq!(s.split('-').next_back().unwrap().len(), 8);
        let hex_tail = s.rsplit('-').next().unwrap();
        assert!(hex_tail.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn slug_for_caps_at_twenty_four_utf16_units() {
        let s = slug_for("aaaaaaaaaabbbbbbbbbbccccccccccdddddddddd");
        // 24 UTF-16 units of the stem survive: 10 a's, 10 b's, 4 c's.
        assert!(s.starts_with("aaaaaaaaaabbbbbbbbbbcccc-"), "{s}");
    }

    #[test]
    fn slug_for_falls_back_when_nothing_survives() {
        let s = slug_for("---");
        assert!(s.starts_with("secret-"), "{s}");
        let s = slug_for("");
        assert!(s.starts_with("secret-"), "{s}");
    }

    #[test]
    fn entry_key_shape() {
        assert!(entry_key_ok("a"));
        assert!(entry_key_ok("a-b_c9"));
        assert!(!entry_key_ok("-a"));
        assert!(!entry_key_ok("_a"));
        assert!(!entry_key_ok("A"));
        assert!(!entry_key_ok(""));
        assert!(!entry_key_ok("a.b"));
    }
}
