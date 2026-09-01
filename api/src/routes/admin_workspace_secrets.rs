// /api/admin/workspace-secrets — port of ui/src/routes/api/admin.workspace-secrets.ts.
//
// WORKSPACE SECRETS — the credentials agents may USE without ever reading one.
//
// NOT `/api/admin/secrets`, WHICH IS A DIFFERENT THING. That route is the
// instance's own secret INVENTORY: provider keys, agent credentials, whether
// each still decrypts. This one holds credentials the workspace hands to agents.
// Two nouns, one word — worth the longer path, because an operator who conflates
// them will eventually revoke the wrong one.
//
// THE ONE RULE THIS FILE EXISTS TO HOLD: a value goes IN and never comes OUT.
// There is no GET that returns one, no echo on create, and no "reveal" verb —
// not as a permission, not for an admin, not once. An endpoint that can return a
// credential is an endpoint that will eventually return one to the wrong caller,
// and the whole arrangement downstream (`secret-vault.ts`, `resolveHandles`)
// rests on the value existing in exactly two places: the sealed column, and the
// outbound request that spends it.
//
// So the GET is deliberately dull — names, titles, entry KEYS, labels, grants,
// lifetimes. Everything a human needs to decide who may use what, and nothing
// that would help anybody use it themselves.
//
// ROTATION IS A CREATE, not an update: writing a new value over an old one under
// the same name leaves no moment where an operator can see which agents were
// using which, so replacing a credential is deleting the doc and making it
// again. That is a deliberate friction on the one operation worth being slow.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    NumKind, array_msg, array_too_big_msg, array_too_small_msg, as_object, boolean_member,
    nullable_number_member, nullish_max_string_member, optional_enum_member,
    optional_string_array_member, parse, present_nullable_uuid_member, string_member,
    string_value_member, too_big_msg, utf16_len, uuid_member, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::routes::secrets::{entry_key_ok, parse_entry};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use crate::workspace_secrets::{
    CreateSecret, FolderWho, SecretEntryInput, create_secret_doc, create_secret_folder,
    delete_secret_doc, delete_secret_folder, grant_secret, handles_held_by, list_secret_docs,
    list_secret_folders, move_secret_to_folder, revoke_secret, share_secret_folder,
};

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // `?agent=` answers the narrower question an agent's own page asks: what can
    // THIS one spend. Not derivable from the full listing — `SecretDoc.grants`
    // carries direct grants only, and a credential reaching the agent through a
    // shared folder would be missing from it.
    let agent = crate::google_oauth::query_pairs(uri.query())
        .get("agent")
        .filter(|a| !a.is_empty())
        .cloned();
    if let Some(agent) = agent {
        return match handles_held_by(&state.pg, &agent).await {
            Ok(held) => Json(json!({ "held": held })).into_response(),
            Err(e) => {
                tracing::error!("[admin/workspace-secrets] held read failed: {e}");
                thrown_internal_error()
            }
        };
    }
    // WORKSPACE folders — owner-less, so they belong to the org rather than to
    // whichever admin happened to make one and can outlive that account.
    let secrets = match list_secret_docs(&state.pg).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[admin/workspace-secrets] list failed: {e}");
            return thrown_internal_error();
        }
    };
    let folders = match list_secret_folders(&state.pg, &user.id, true).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[admin/workspace-secrets] folders read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "secrets": secrets, "folders": folders })).into_response()
}

/// The post-mutation listings every action ends with.
async fn listing(pg: &sqlx::PgPool) -> Response {
    match list_secret_docs(pg).await {
        Ok(s) => Json(json!({ "secrets": s })).into_response(),
        Err(e) => internal(e, "list"),
    }
}

async fn folders_listing(pg: &sqlx::PgPool, user_id: &str) -> Response {
    match list_secret_folders(pg, user_id, true).await {
        Ok(f) => Json(json!({ "folders": f })).into_response(),
        Err(e) => internal(e, "folders"),
    }
}

/// The engine errors that reach the caller: TS wraps ONLY the create in a
/// .catch (its message is a dup-name sentence the operator needs); every other
/// action throws to the 500 boundary.
fn internal(e: String, what: &str) -> Response {
    tracing::error!("[admin/workspace-secrets] {what} failed: {e}");
    thrown_internal_error()
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let actor = actor_of(&user);
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Post = z.union of nine objects, each pinned by an `action` literal —
    // dispatch on the literal, then validate that branch's fields in schema
    // order with zod's own messages. No `action` at all fails every branch
    // of the union: the blanket 400, NEVER the delete arm.
    let action = obj.get("action").and_then(Value::as_str);

    match action {
        // ── CREATE (rotation is a create — see the file header) ────────────
        Some("create") => {
            // name: z.string().regex(..., 'lowercase letters, digits, - and
            // _').max(40) — the regex sits before the max, like the entry key.
            let name = match string_value_member(obj, "name") {
                Ok(n) => n,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            if !entry_key_ok(&name) {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    "lowercase letters, digits, - and _",
                );
            }
            if utf16_len(&name) > 40 {
                return house_error(StatusCode::BAD_REQUEST, &too_big_msg(40));
            }
            let title = match string_member(obj, "title", 1, 80) {
                Ok(t) => t,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            // entries: elements parse before the array bounds run.
            let arr = match obj.get("entries") {
                None => return house_error(StatusCode::BAD_REQUEST, &array_msg("undefined")),
                Some(v) => match v.as_array() {
                    Some(a) => a,
                    None => {
                        return house_error(StatusCode::BAD_REQUEST, &array_msg(zod_type_name(v)));
                    }
                },
            };
            let mut entries = Vec::with_capacity(arr.len());
            for el in arr {
                match parse_entry(el) {
                    Ok(e) => entries.push(e),
                    Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
                }
            }
            if arr.is_empty() {
                return house_error(StatusCode::BAD_REQUEST, &array_too_small_msg(1));
            }
            if arr.len() > 20 {
                return house_error(StatusCode::BAD_REQUEST, &array_too_big_msg(20));
            }
            let kind = match optional_enum_member(obj, "kind", &["vault", "relay"]) {
                Ok(k) => k.map(|k| match k.as_str() {
                    "vault" => "vault",
                    _ => "relay",
                }),
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let note = match nullish_max_string_member(obj, "note", 400) {
                Ok(n) => n,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let expires_at = match nullish_max_string_member(obj, "expiresAt", 40) {
                Ok(e) => e,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            // z.number().int().min(1).max(1000).nullish()
            let uses = match nullable_number_member(obj, "uses", NumKind::Int, 1.0, 1000.0) {
                Ok(u) => u.map(|f| f as i64),
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let grant_to = match optional_string_array_member(obj, "grantTo", 0, 120, 50) {
                Ok(g) => g,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            // Hosts this credential may be spent against. Empty/absent =
            // unrestricted, which is what every secret predating the check has.
            let allowed_hosts = match optional_string_array_member(obj, "allowedHosts", 0, 253, 30)
            {
                Ok(h) => h,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };

            // A failed secretbox is the create failing: the value cannot be
            // sealed. The ADMIN create surfaces the engine's own message
            // (unlike /api/secrets, which hides it) — TS .catch's 400.
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => {
                    tracing::error!("[admin/workspace-secrets] secretbox unavailable: {e}");
                    return thrown_internal_error();
                }
            };
            let doc = create_secret_doc(
                &state.pg,
                &sb,
                &CreateSecret {
                    name,
                    title,
                    entries: entries
                        .into_iter()
                        .map(|e| SecretEntryInput {
                            key: e.key,
                            label: e.label,
                            value: e.value,
                        })
                        .collect(),
                    kind,
                    note,
                    created_by: Some(actor.clone()),
                    expires_at,
                    uses,
                    grant_to,
                    allowed_hosts,
                    // The admin route never sets it: an agent credential
                    // stays unreadable forever.
                    revealable: false,
                    owner_user_id: None,
                    folder_id: None,
                    readers: None,
                },
            )
            .await;
            let doc = match doc {
                Ok(d) => d,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            // THE AUDIT LINE CARRIES KINDS, NEVER VALUES — the same rule the
            // store itself follows. `doc.entries` is keys and labels by
            // construction.
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "secrets.create",
                    target_type: "secret",
                    target_id: Some(&doc.name),
                    target_label: None,
                    before: None,
                    after: Some(json!({
                        "kind": doc.kind,
                        "entries": doc.entries,
                        "grants": doc.grants,
                        "uses": doc.uses_remaining,
                        "allowedHosts": doc.allowed_hosts,
                    })),
                },
            )
            .await;
            Json(json!({ "secret": doc })).into_response()
        }

        // ── GRANT / REVOKE ──────────────────────────────────────────────────
        Some("grant") | Some("revoke") => {
            let name = match string_member(obj, "name", 0, 40) {
                Ok(n) => n,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let agent_model = match string_member(obj, "agentModel", 1, 120) {
                Ok(a) => a,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let result = if action == Some("grant") {
                grant_secret(&state.pg, &name, &agent_model, Some(&actor)).await
            } else {
                revoke_secret(&state.pg, &name, &agent_model).await
            };
            if let Err(e) = result {
                return internal(e, "grant/revoke");
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: if action == Some("grant") {
                        "secrets.grant"
                    } else {
                        "secrets.revoke"
                    },
                    target_type: "secret",
                    target_id: Some(&name),
                    target_label: None,
                    before: None,
                    after: Some(json!({ "agentModel": agent_model })),
                },
            )
            .await;
            listing(&state.pg).await
        }

        // ── DELETE ──────────────────────────────────────────────────────────
        Some("delete") => {
            let name = match string_member(obj, "name", 0, 40) {
                Ok(n) => n,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            if let Err(e) = delete_secret_doc(&state.pg, &name).await {
                return internal(e, "delete");
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "secrets.delete",
                    target_type: "secret",
                    target_id: Some(&name),
                    target_label: None,
                    before: None,
                    after: None,
                },
            )
            .await;
            listing(&state.pg).await
        }

        // ── FOLDERS, for grouping credentials and granting a whole set to an
        // agent at once — the same argument that made folder sharing worth
        // building for people. ──────────────────────────────────────────────
        Some("folder-create") => {
            let name = match string_member(obj, "name", 1, 60) {
                Ok(n) => n,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let f = match create_secret_folder(&state.pg, &name, None).await {
                Ok(f) => f,
                Err(e) => return internal(e, "folder create"),
            };
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "secrets.folder.create",
                    target_type: "secret-folder",
                    target_id: Some(&f.id),
                    target_label: Some(&f.name),
                    before: None,
                    after: None,
                },
            )
            .await;
            folders_listing(&state.pg, &user.id).await
        }
        Some("folder-delete") => {
            // The credentials survive — `on delete set null` returns them to
            // the top level. Deleting four working keys because somebody
            // tidied a label would be an unforgivable way to lose them.
            let id = match uuid_member(obj, "id") {
                Ok(i) => i,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let deleted = match delete_secret_folder(&state.pg, &id, &user.id, true).await {
                Ok(d) => d,
                Err(e) => return internal(e, "folder delete"),
            };
            if !deleted {
                return house_error(StatusCode::NOT_FOUND, "no such folder");
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "secrets.folder.delete",
                    target_type: "secret-folder",
                    target_id: Some(&id),
                    target_label: None,
                    before: None,
                    after: None,
                },
            )
            .await;
            folders_listing(&state.pg, &user.id).await
        }
        Some("folder-grant") => {
            // GRANTS THE WHOLE FOLDER, now and later. A credential added to it
            // next week is covered without anybody re-granting — which is the
            // step everybody forgets, and forgetting it looks like the agent
            // silently lacking a key nobody can explain.
            let id = match uuid_member(obj, "id") {
                Ok(i) => i,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let agent_model = match string_member(obj, "agentModel", 1, 120) {
                Ok(a) => a,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let on = match boolean_member(obj, "on") {
                Ok(b) => b,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let shared = match share_secret_folder(
                &state.pg,
                &id,
                &FolderWho {
                    user_id: None,
                    agent_model: Some(agent_model.clone()),
                },
                on,
                &user.id,
                true,
            )
            .await
            {
                Ok(s) => s,
                Err(e) => return internal(e, "folder grant"),
            };
            if !shared {
                return house_error(StatusCode::NOT_FOUND, "no such folder");
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: if on {
                        "secrets.folder.grant"
                    } else {
                        "secrets.folder.revoke"
                    },
                    target_type: "secret-folder",
                    target_id: Some(&id),
                    target_label: None,
                    before: None,
                    after: Some(json!({ "agentModel": agent_model })),
                },
            )
            .await;
            folders_listing(&state.pg, &user.id).await
        }
        Some("file") => {
            let name = match string_member(obj, "name", 0, 40) {
                Ok(n) => n,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            // folderId is REQUIRED but may be null (Uuid.nullable()) — absent
            // fails the whole branch, and zod's union answers its blanket
            // (probed: neither the uuid's nor the string's own words).
            let folder_id = match present_nullable_uuid_member(obj, "folderId") {
                Ok(Some(f)) => f,
                Ok(None) => return house_error(StatusCode::BAD_REQUEST, "Invalid input"),
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let moved =
                match move_secret_to_folder(&state.pg, &name, folder_id.as_deref(), &user.id, true)
                    .await
                {
                    Ok(m) => m,
                    Err(e) => return internal(e, "file"),
                };
            if !moved {
                return house_error(StatusCode::BAD_REQUEST, "could not file that");
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "secrets.move",
                    target_type: "secret",
                    target_id: Some(&name),
                    target_label: None,
                    before: None,
                    after: Some(json!({ "folderId": folder_id })),
                },
            )
            .await;
            listing(&state.pg).await
        }
        // An unknown action, or none at all, fails every branch of the union.
        Some(_) | None => house_error(StatusCode::BAD_REQUEST, "Invalid input"),
    }
}
