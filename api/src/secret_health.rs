// The one answer to "what secrets does this instance hold, and can it still
// read them?" — a port of ui/src/server/secret-health.ts. A VIEW over the
// stores that already own each value, never a second store of its own.
//
// TWO RULES THIS FILE KEEPS (TS's, verbatim):
//   1. It never returns a plaintext secret. Not masked, not truncated — the
//      shape has no field for one, so there is nowhere for one to leak.
//   2. It never throws on an unhealthy instance. Every read here is a status
//      read, and a status read that fails when the instance is broken is
//      useless exactly when it is needed. The typed config getters are avoided
//      on purpose (several call open() on read); the app_settings rows are
//      read directly instead.

use crate::config::RootSource;
use crate::secretbox::SecretBox;
use sqlx::PgPool;
use std::collections::BTreeMap;

/// Per-user rows: an admin sees that a user's connection exists, whose it is,
/// when it was set, and whether it still decrypts — enough to recover the
/// instance. NOT the granted scopes, the account it points at, or when it was
/// last used (TS's USER_SCOPED_METADATA = false).
const USER_SCOPED_METADATA: bool = false;

// ── Wire shape ───────────────────────────────────────────────────────────────
// Hand-built serde_json rather than typed structs: field ORDER is the wire
// order (TS object literals), and `owner`/`href`/`setAt`/… are OMITTED when
// absent, not null.

fn row(fields: serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    serde_json::Value::Object(fields)
}

/// iso() — a timestamp column as node's toISOString(), or JSON null. The
/// epoch-ms read + epoch_ms_to_iso pair is the house's byte-exact route (the
/// ms is trunc'd, so the .SSS fraction always has three digits).
fn opt_iso(ms: Option<i64>) -> serde_json::Value {
    match ms {
        Some(ms) => crate::agent_auth::epoch_ms_to_iso(ms).into(),
        None => serde_json::Value::Null,
    }
}

/// stateOf — one row's state from its ciphertext. `env_name` covers the
/// provider-key case where the value is a variable name rather than a sealed
/// value.
fn state_of(sb: &SecretBox, cipher: Option<&str>, env_name: Option<&str>) -> &'static str {
    // JS truthiness: an empty-string cipher is falsy, so the env check runs.
    let cipher = cipher.filter(|c| !c.is_empty());
    match cipher {
        Some(c) => {
            if sb.token_readable(c) {
                "ok"
            } else {
                "unreadable"
            }
        }
        None => {
            if env_name.is_some() && !env_name.unwrap_or("").is_empty() {
                "env"
            } else {
                "missing"
            }
        }
    }
}

/// One row's state where an empty string means missing (the workspace entry
/// case: `resolveHandles` empties the ciphertext once the last use is gone,
/// deliberately — a SPENT ONE-SHOT IS NOT BROKEN).
fn state_of_entry(sb: &SecretBox, cipher: &str) -> &'static str {
    if cipher.is_empty() {
        "missing"
    } else if sb.token_readable(cipher) {
        "ok"
    } else {
        "unreadable"
    }
}

pub async fn secret_health(pg: &PgPool, sb: &SecretBox, root: &crate::config::SecretRoot) -> serde_json::Value {
    let mut rows: Vec<serde_json::Value> = Vec::new();

    // ── Models ──────────────────────────────────────────────────────────────
    // Every endpoint, not only the sealed ones: an endpoint with no key at all
    // is a real gap ("why won't this model answer?") and belongs in the list.
    type EndpointRow = (String, String, Option<String>, Option<String>, Option<i64>, Option<i64>);
    let endpoints: Result<Vec<EndpointRow>, _> =
        sqlx::query_as(
            "select id::text, name, api_key_cipher, api_key_env, \
             (trunc(extract(epoch from created_at) * 1000))::bigint, \
             (trunc(extract(epoch from updated_at) * 1000))::bigint \
             from llm_endpoints order by name asc",
        )
        .fetch_all(pg)
        .await;
    if let Ok(eps) = endpoints {
        for (id, name, cipher, env_name, created, updated) in eps {
            let mut f = serde_json::Map::new();
            f.insert("id".into(), format!("llm:{id}").into());
            f.insert("group".into(), "models".into());
            f.insert("label".into(), name.clone().into());
            f.insert(
                "unlocks".into(),
                format!("Chat, Plan, Research and every agent turn routed to {name}").into(),
            );
            f.insert("surface".into(), "Models".into());
            f.insert("href".into(), "/models".into());
            f.insert("state".into(), state_of(sb, cipher.as_deref(), env_name.as_deref()).into());
            f.insert("scope".into(), "instance".into());
            // setAt: iso(updatedAt) ?? iso(createdAt) — both null when both are.
            f.insert("setAt".into(), opt_iso(updated.or(created)));
            f.insert("clearable".into(), cipher.as_deref().is_some_and(|c| !c.is_empty()).into());
            rows.push(row(f));
        }
    }

    // ── Workspace credentials (the handle store) ────────────────────────────
    // ONE ROW PER ENTRY, not per doc: a bundle can be half-readable if it was
    // written across a rotation, and a doc-level row would report the whole
    // thing on the state of whichever entry happened to sort first.
    type WorkspaceRow = (String, String, bool, Option<String>, Option<i64>, String, String, String, i32);
    let workspace: Result<Vec<WorkspaceRow>, _> =
        sqlx::query_as(
            "select s.name, s.title, s.revealable, s.owner_user_id::text, \
             (trunc(extract(epoch from s.updated_at) * 1000))::bigint, \
             e.key, e.label, e.value_cipher, \
             (select count(*)::int from workspace_secret_grants g where g.secret_id = s.id) \
             from workspace_secrets s join workspace_secret_entries e on e.secret_id = s.id \
             order by s.title asc, e.key asc",
        )
        .fetch_all(pg)
        .await;
    if let Ok(ws) = workspace {
        for (name, title, revealable, owner, updated, key, label, cipher, grants) in ws {
            let mut f = serde_json::Map::new();
            f.insert("id".into(), format!("workspace-secret:{name}:{key}").into());
            f.insert("group".into(), "agents".into());
            f.insert("label".into(), format!("{title} · {label}").into());
            f.insert(
                "unlocks".into(),
                if revealable {
                    format!(
                        "A working secret. {}the people it is shared with can read it",
                        if grants > 0 {
                            format!("{grants} agent{} can spend it; ", if grants == 1 { "" } else { "s" })
                        } else {
                            String::new()
                        }
                    )
                } else {
                    format!(
                        "{} — and nobody can read it",
                        if grants > 0 {
                            format!("{grants} agent{} can SPEND it", if grants == 1 { "" } else { "s" })
                        } else {
                            "Granted to no agent, so nobody can spend it".to_string()
                        }
                    )
                }
                .into(),
            );
            f.insert("surface".into(), if revealable { "Files → Secrets" } else { "Admin → Secrets" }.into());
            f.insert("href".into(), if revealable { "/artifacts/secrets" } else { "/admin/secrets" }.into());
            f.insert("state".into(), state_of_entry(sb, &cipher).into());
            f.insert("scope".into(), if revealable { "user" } else { "instance" }.into());
            if revealable
                && let Some(o) = owner.as_deref()
            {
                f.insert("owner".into(), o.into());
            }
            f.insert("setAt".into(), opt_iso(updated));
            // NOT CLEARABLE FROM HERE: deleting one entry out of a bundle
            // leaves a credential that half-works.
            f.insert("clearable".into(), false.into());
            rows.push(row(f));
        }
    }

    // ── Agents ──────────────────────────────────────────────────────────────
    type AgentSecretRow = (String, String, String, Option<i64>, String);
    let agent_secrets: Result<Vec<AgentSecretRow>, _> = sqlx::query_as(
        "select s.agent_id::text, s.name, s.value_enc, \
         (trunc(extract(epoch from s.updated_at) * 1000))::bigint, d.display_name \
         from agent_secrets s join agent_defs d on d.id = s.agent_id \
         order by d.display_name asc, s.name asc",
    )
    .fetch_all(pg)
    .await;
    if let Ok(ss) = agent_secrets {
        for (agent_id, name, cipher, updated, agent_name) in ss {
            let mut f = serde_json::Map::new();
            f.insert("id".into(), format!("agent-secret:{agent_id}:{name}").into());
            f.insert("group".into(), "agents".into());
            f.insert("label".into(), name.clone().into());
            f.insert(
                "unlocks".into(),
                format!(
                    "Whatever {agent_name} uses {name} for. Materialised into the container as a PLAINTEXT environment variable, so {agent_name} can read it — prefer a credential handle where the value only has to be spent, not seen"
                )
                .into(),
            );
            f.insert("surface".into(), "Agents".into());
            f.insert("href".into(), "/agents".into());
            f.insert("state".into(), state_of(sb, Some(&cipher), None).into());
            f.insert("scope".into(), "agent".into());
            f.insert("owner".into(), agent_name.clone().into());
            f.insert("setAt".into(), opt_iso(updated));
            f.insert("clearable".into(), true.into());
            rows.push(row(f));
        }
    }

    type AgentKeyRow = (String, String, Option<i64>, Option<i64>, String);
    let agent_keys: Result<Vec<AgentKeyRow>, _> = sqlx::query_as(
        "select k.agent_id::text, k.key_enc, \
         (trunc(extract(epoch from k.created_at) * 1000))::bigint, \
         (trunc(extract(epoch from k.last_used_at) * 1000))::bigint, d.display_name \
         from agent_keys k join agent_defs d on d.id = k.agent_id \
         order by d.display_name asc",
    )
    .fetch_all(pg)
    .await;
    if let Ok(ks) = agent_keys {
        for (agent_id, cipher, created, last_used, agent_name) in ks {
            let mut f = serde_json::Map::new();
            f.insert("id".into(), format!("agent-key:{agent_id}").into());
            f.insert("group".into(), "agents".into());
            f.insert("label".into(), format!("{agent_name} credential").into());
            f.insert(
                "unlocks".into(),
                format!(
                    "Re-rendering {agent_name}'s container config. The agent authenticates against a hash, so it keeps working; the fleet just cannot reissue its key without a re-render"
                )
                .into(),
            );
            f.insert("surface".into(), "Agents".into());
            f.insert("href".into(), "/agents".into());
            f.insert("state".into(), state_of(sb, Some(&cipher), None).into());
            f.insert("scope".into(), "agent".into());
            f.insert("owner".into(), agent_name.into());
            f.insert("setAt".into(), opt_iso(created));
            f.insert("lastUsedAt".into(), opt_iso(last_used));
            f.insert("clearable".into(), true.into());
            rows.push(row(f));
        }
    }

    // ── Integrations ────────────────────────────────────────────────────────
    type GoogleUserRow = (String, Option<String>, Option<i64>, String);
    let google_users: Result<Vec<GoogleUserRow>, _> = sqlx::query_as(
        "select g.user_id::text, g.refresh_token_enc, \
         (trunc(extract(epoch from g.updated_at) * 1000))::bigint, u.email \
         from google_connections g join users u on u.id = g.user_id \
         order by u.email asc",
    )
    .fetch_all(pg)
    .await;
    if let Ok(cs) = google_users {
        for (user_id, cipher, updated, email) in cs {
            let mut f = serde_json::Map::new();
            f.insert("id".into(), format!("google-user:{user_id}").into());
            f.insert("group".into(), "integrations".into());
            f.insert("label".into(), "Google connection".into());
            f.insert(
                "unlocks".into(),
                "Drive and Docs for this person, and their assistant acting as them".into(),
            );
            f.insert("surface".into(), "Settings".into());
            f.insert("state".into(), state_of(sb, cipher.as_deref(), None).into());
            f.insert("scope".into(), "user".into());
            f.insert("owner".into(), email.into());
            f.insert("setAt".into(), opt_iso(updated));
            f.insert("clearable".into(), true.into());
            rows.push(row(f));
        }
    }

    type GoogleOrgRow = (Option<String>, Option<String>, Option<i64>, Option<i64>);
    let google_org: Result<Vec<GoogleOrgRow>, _> =
        sqlx::query_as(
            "select refresh_token_enc, email, \
             (trunc(extract(epoch from updated_at) * 1000))::bigint, \
             (trunc(extract(epoch from access_expires_at) * 1000))::bigint \
             from google_org_connection where id = 1",
        )
        .fetch_all(pg)
        .await;
    let org_row = google_org.unwrap_or_default().first().cloned();
    {
        let (cipher, email, updated, expires) = match org_row {
            Some(t) => t,
            None => (None, None, None, None),
        };
        let mut f = serde_json::Map::new();
        f.insert("id".into(), "google-org".into());
        f.insert("group".into(), "integrations".into());
        f.insert("label".into(), "Org Google connection".into());
        f.insert(
            "unlocks".into(),
            "Drive and Docs for every fleet agent without a human owner".into(),
        );
        f.insert("surface".into(), "Admin → Organization".into());
        f.insert("href".into(), "/admin".into());
        f.insert("state".into(), state_of(sb, cipher.as_deref(), None).into());
        f.insert("scope".into(), "instance".into());
        if let Some(e) = email.as_deref().filter(|e| !e.is_empty()) {
            f.insert("owner".into(), e.into());
        }
        f.insert("setAt".into(), opt_iso(updated));
        f.insert("expiresAt".into(), opt_iso(expires));
        f.insert(
            "clearable".into(),
            cipher.as_deref().is_some_and(|c| !c.is_empty()).into(),
        );
        rows.push(row(f));
    }

    type McpTokenRow = (String, String, String, Option<i64>, String, Option<String>);
    let mcp_tokens: Result<Vec<McpTokenRow>, _> =
        sqlx::query_as(
            "select t.server_id::text, t.subject, t.tokens_enc, \
             (trunc(extract(epoch from t.updated_at) * 1000))::bigint, \
             s.label, u.email \
             from mcp_oauth_tokens t \
             join mcp_servers s on s.id = t.server_id \
             left join users u on u.id::text = t.subject \
             order by s.label asc, t.subject asc",
        )
        .fetch_all(pg)
        .await;
    if let Ok(ts) = mcp_tokens {
        for (server_id, subject, cipher, updated, label, email) in ts {
            let org = subject == "org";
            let mut f = serde_json::Map::new();
            f.insert("id".into(), format!("mcp-oauth:{server_id}:{subject}").into());
            f.insert("group".into(), "integrations".into());
            f.insert("label".into(), format!("{label} — connected account").into());
            f.insert(
                "unlocks".into(),
                if org {
                    format!("{label} for everyone who uses it")
                } else {
                    format!("{label} for this person")
                }
                .into(),
            );
            f.insert("surface".into(), "MCP".into());
            f.insert("href".into(), "/mcp".into());
            f.insert("state".into(), state_of(sb, Some(&cipher), None).into());
            f.insert("scope".into(), if org { "instance" } else { "user" }.into());
            if !org {
                f.insert("owner".into(), email.unwrap_or(subject).into());
            }
            f.insert("setAt".into(), opt_iso(updated));
            f.insert("clearable".into(), true.into());
            rows.push(row(f));
        }
    }

    type McpHeaderRow = (String, String, String, Option<i64>, String, String);
    let mcp_headers: Result<Vec<McpHeaderRow>, _> =
        sqlx::query_as(
            "select c.server_id::text, c.user_id::text, c.headers_enc, \
             (trunc(extract(epoch from c.updated_at) * 1000))::bigint, \
             s.label, u.email \
             from mcp_user_credentials c \
             join mcp_servers s on s.id = c.server_id \
             join users u on u.id = c.user_id \
             order by s.label asc, u.email asc",
        )
        .fetch_all(pg)
        .await;
    if let Ok(hs) = mcp_headers {
        for (server_id, user_id, cipher, updated, label, email) in hs {
            let mut f = serde_json::Map::new();
            f.insert("id".into(), format!("mcp-headers:{server_id}:{user_id}").into());
            f.insert("group".into(), "integrations".into());
            f.insert("label".into(), format!("{label} — credentials").into());
            f.insert("unlocks".into(), format!("{label} for this person").into());
            f.insert("surface".into(), "MCP".into());
            f.insert("href".into(), "/mcp".into());
            f.insert("state".into(), state_of(sb, Some(&cipher), None).into());
            f.insert("scope".into(), "user".into());
            f.insert("owner".into(), email.into());
            f.insert("setAt".into(), opt_iso(updated));
            f.insert("clearable".into(), true.into());
            rows.push(row(f));
        }
    }

    // ── Platform (app_settings) ─────────────────────────────────────────────
    // Read raw: the typed getters for these keys call open() on read — one of
    // them swallows the failure into an empty string, which is precisely the
    // silent unreadability this inventory exists to surface.
    let setting_rows: Result<Vec<(String, serde_json::Value, Option<i64>)>, _> = sqlx::query_as(
        "select key, value, (trunc(extract(epoch from updated_at) * 1000))::bigint \
         from app_settings where key = any($1)",
    )
    .bind(vec![
        "email_config".to_string(),
        "storage_config".to_string(),
        "github_config".to_string(),
        "rag_rerank_config".to_string(),
    ])
    .fetch_all(pg)
    .await;
    let settings: BTreeMap<String, (serde_json::Value, Option<i64>)> = setting_rows
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v, at)| (k, (v, at)))
        .collect();
    let dig = |k: Option<&(serde_json::Value, Option<i64>)>, path: &[&str]| -> Option<String> {
        let mut cur = k?.0.get(path[0])?;
        for p in &path[1..] {
            cur = cur.get(p)?;
        }
        cur.as_str().filter(|s| !s.is_empty()).map(String::from)
    };

    let email = settings.get("email_config");
    let email_provider = dig(email, &["provider"]);
    let email_cipher = if email_provider.as_deref() == Some("resend") {
        dig(email, &["resend", "apiKeyEnc"])
    } else {
        dig(email, &["smtp", "passEnc"])
    };
    {
        let mut f = serde_json::Map::new();
        f.insert(
            "id".into(),
            format!(
                "setting:email_config:{}",
                if email_provider.as_deref() == Some("resend") {
                    "resend.apiKeyEnc"
                } else {
                    "smtp.passEnc"
                }
            )
            .into(),
        );
        f.insert("group".into(), "platform".into());
        f.insert(
            "label".into(),
            if email_provider.as_deref() == Some("resend") {
                "Resend API key"
            } else {
                "SMTP password"
            }
            .into(),
        );
        f.insert(
            "unlocks".into(),
            "Invites, password resets, and any notification delivered by email".into(),
        );
        f.insert("surface".into(), "Admin → Organization".into());
        f.insert("href".into(), "/admin".into());
        f.insert("state".into(), state_of(sb, email_cipher.as_deref(), None).into());
        f.insert("scope".into(), "instance".into());
        f.insert("setAt".into(), opt_iso(email.and_then(|(_, at)| *at)));
        f.insert("clearable".into(), email.is_some().into());
        rows.push(row(f));
    }

    let storage = settings.get("storage_config");
    let storage_mode = dig(storage, &["mode"]);
    // Only external buckets hold a secret worth inventorying — 'local' writes
    // to disk and 'internal' uses the bundled MinIO's env-held credentials.
    if let Some(mode) = storage_mode.as_deref()
        && mode != "local"
        && mode != "internal"
    {
        for (path, label) in [
            ("secretAccessKey", "Object storage secret key"),
            ("replica.secretAccessKey", "Object storage replica secret key"),
        ] {
            let cipher = match path {
                "secretAccessKey" => dig(storage, &["secretAccessKey"]),
                _ => dig(storage, &["replica", "secretAccessKey"]),
            };
            if path.contains("replica") && cipher.is_none() {
                continue; // the replica row exists only when its secret does
            }
            let mut f = serde_json::Map::new();
            f.insert("id".into(), format!("setting:storage_config:{path}").into());
            f.insert("group".into(), "platform".into());
            f.insert("label".into(), label.into());
            f.insert(
                "unlocks".into(),
                if path.contains("replica") {
                    "Mirroring uploads to the replica bucket"
                } else {
                    "Uploads, attachments and artifact files"
                }
                .into(),
            );
            f.insert("surface".into(), "Admin → Storage".into());
            f.insert("href".into(), "/admin/storage".into());
            f.insert("state".into(), state_of(sb, cipher.as_deref(), None).into());
            f.insert("scope".into(), "instance".into());
            f.insert("setAt".into(), opt_iso(storage.and_then(|(_, at)| *at)));
            f.insert("clearable".into(), true.into());
            rows.push(row(f));
        }
    }

    let github = settings.get("github_config");
    let github_mode = dig(github, &["mode"]);
    if let Some(mode) = github_mode.as_deref() {
        let is_app = mode == "app";
        let cipher = if is_app {
            dig(github, &["app", "privateKeyEnc"])
        } else {
            dig(github, &["pat", "tokenEnc"])
        };
        let mut f = serde_json::Map::new();
        f.insert(
            "id".into(),
            format!("setting:github_config:{}", if is_app { "app.privateKeyEnc" } else { "pat.tokenEnc" }).into(),
        );
        f.insert("group".into(), "platform".into());
        f.insert(
            "label".into(),
            if is_app { "GitHub App private key" } else { "GitHub access token" }.into(),
        );
        f.insert(
            "unlocks".into(),
            "Workbench: cloning repos, pushing branches, opening pull requests".into(),
        );
        f.insert("surface".into(), "Admin → Organization".into());
        f.insert("href".into(), "/admin".into());
        f.insert("state".into(), state_of(sb, cipher.as_deref(), None).into());
        f.insert("scope".into(), "instance".into());
        f.insert("setAt".into(), opt_iso(github.and_then(|(_, at)| *at)));
        f.insert("clearable".into(), true.into());
        rows.push(row(f));
    }

    let rerank = settings.get("rag_rerank_config");
    if let Some(cipher) = dig(rerank, &["keySealed"]) {
        let mut f = serde_json::Map::new();
        f.insert("id".into(), "setting:rag_rerank_config:keySealed".into());
        f.insert("group".into(), "platform".into());
        f.insert("label".into(), "Reranker API key".into());
        f.insert(
            "unlocks".into(),
            "Reranking retrieval results — search still works without it, less well".into(),
        );
        f.insert("surface".into(), "Admin → Retrieval".into());
        f.insert("href".into(), "/admin/retrieval".into());
        f.insert("state".into(), state_of(sb, Some(&cipher), None).into());
        f.insert("scope".into(), "instance".into());
        f.insert("setAt".into(), opt_iso(rerank.and_then(|(_, at)| *at)));
        f.insert("clearable".into(), true.into());
        rows.push(row(f));
    }

    if !USER_SCOPED_METADATA {
        for r in rows.iter_mut() {
            if r.get("scope") == Some(&serde_json::json!("user"))
                && let serde_json::Value::Object(o) = r
            {
                o.remove("lastUsedAt");
                o.remove("expiresAt");
            }
        }
    }

    // ── Root ────────────────────────────────────────────────────────────────
    let stored_versions: i64 = sqlx::query_scalar("select count(*) from secret_keys")
        .fetch_one(pg)
        .await
        .unwrap_or(0);
    let (via, name) = match root.source() {
        RootSource::SecretKey => ("env", "TALARIA_SECRET_KEY"),
        RootSource::SecretKeyFile => ("file", root.name()),
        RootSource::AuthSecretFallback => ("fallback", "AUTH_SECRET"),
    };
    let failure = sb.failure().map(String::from);
    let loaded = sb.loaded_versions();
    let root_state = if failure.is_some() || (stored_versions > 0 && loaded.is_empty()) {
        "unreadable"
    } else if via == "absent" {
        "absent"
    } else if via == "fallback" {
        "fallback"
    } else {
        "ok"
    };
    let root = serde_json::json!({
        "via": via,
        "name": name,
        "state": root_state,
        "failure": failure,
        "activeVersion": sb.active_key_version(),
        "loadedVersions": loaded,
        "storedVersions": stored_versions,
    });

    let mut counts = serde_json::Map::new();
    for key in ["ok", "unreadable", "missing", "env"] {
        counts.insert(
            key.into(),
            rows.iter()
                .filter(|r| r.get("state") == Some(&serde_json::json!(key)))
                .count()
                .into(),
        );
    }

    serde_json::json!({ "root": root, "rows": rows, "counts": counts })
}

// ── Clearing ─────────────────────────────────────────────────────────────────
// The in-app half of `talaria reset secrets`, scoped to one row. Clearing is
// a DELETE of ciphertext and nothing else: the endpoint, the agent, the
// person and the server all survive, missing a credential they can be given
// again.

#[derive(Debug, thiserror::Error)]
pub enum ClearError {
    /// The id parsed to no store, or a setting path with an empty segment.
    /// The route answers TS's 404 `unknown secret` for exactly this.
    #[error("unknown secret id")]
    Unknown,
    /// A database failure — the route logs it and answers the generic 500;
    /// these paths sit next to key material, so nothing raw is echoed.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The store arms' one shape: literal SQL, string binds, "did a row change".
async fn run_clear(pg: &PgPool, sql: &'static str, binds: &[&str]) -> Result<bool, sqlx::Error> {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(b);
    }
    Ok(q.execute(pg).await?.rows_affected() > 0)
}

/// Clear exactly one row's ciphertext. Ok(false) when the id was well-formed
/// but matched nothing — the caller reports a no-op, because the desired end
/// state has been reached either way.
pub async fn clear_secret(pg: &PgPool, secret_id: &str) -> Result<bool, ClearError> {
    let mut parts = secret_id.split(':');
    let Some(store) = parts.next() else { return Err(ClearError::Unknown) };
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        return Err(ClearError::Unknown);
    }
    let n = |i: usize| rest.get(i).copied().unwrap_or("");

    match store {
        // `is not null` is what makes the return value mean "something
        // changed" — an UPDATE counts rows it matched, not rows it altered.
        "llm" => run_clear(
            pg,
            "update llm_endpoints set api_key_cipher = null, updated_at = now() \
             where id = $1::uuid and api_key_cipher is not null",
            &[n(0)],
        )
        .await
        .map_err(ClearError::from),
        // The name can itself contain no colon (agent secrets are env-var
        // names), but rejoin anyway so a future looser name cannot truncate.
        "agent-secret" => {
            let name = rest[1..].join(":");
            sqlx::query("delete from agent_secrets where agent_id = $1::uuid and name = $2")
                .bind(n(0))
                .bind(&name)
                .execute(pg)
                .await
                .map(|r| r.rows_affected() > 0)
                .map_err(ClearError::from)
        }
        // Deletes the hash along with the sealed copy, so the agent's current
        // key stops authenticating: an unreadable key_enc means the fleet can
        // never reissue this credential, and leaving the hash behind would
        // keep a key alive that nothing can reproduce.
        "agent-key" => run_clear(
            pg,
            "delete from agent_keys where agent_id = $1::uuid",
            &[n(0)],
            )
            .await
            .map_err(ClearError::from),
        "google-user" => run_clear(
            pg,
            "delete from google_connections where user_id = $1::uuid",
            &[n(0)],
            )
            .await
            .map_err(ClearError::from),
        "google-org" => run_clear(pg, "delete from google_org_connection where id = 1", &[])
            .await
            .map_err(ClearError::from),
        "mcp-oauth" => run_clear(
            pg,
            "delete from mcp_oauth_tokens where server_id = $1::uuid and subject = $2",
            &[n(0), n(1)],
            )
            .await
            .map_err(ClearError::from),
        "mcp-headers" => run_clear(
            pg,
            "delete from mcp_user_credentials where server_id = $1::uuid and user_id = $2::uuid",
            &[n(0), n(1)],
            )
            .await
            .map_err(ClearError::from),
        "setting" => clear_setting_leaf(pg, n(0), &rest[1..].join(":")).await,
        _ => Err(ClearError::Unknown),
    }
}

/// The `setting:` arm — null the leaf, not delete the key: several of these
/// configs read the key's presence and a missing key would fall back to a
/// DEFAULT rather than to "unset". Read-modify-write under a row lock, like
/// the TS transaction.
async fn clear_setting_leaf(pg: &PgPool, key: &str, dotted: &str) -> Result<bool, ClearError> {
    if key.is_empty() {
        return Err(ClearError::Unknown);
    }
    let path: Vec<&str> = dotted.split('.').collect();
    if path.is_empty() || path.iter().any(|p| p.is_empty()) {
        return Err(ClearError::Unknown);
    }
    let mut tx = pg.begin().await.map_err(ClearError::from)?;
    let current: Option<(serde_json::Value,)> = sqlx::query_as(
        "select value from app_settings where key = $1 for update",
    )
    .bind(key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(ClearError::from)?;
    let Some((mut value,)) = current else { return Ok(false) };
    // Walk to the parent, then null the leaf — '' for strings (several of
    // these configs read presence), null for everything else.
    let mut node: &mut serde_json::Value = &mut value;
    for p in &path[..path.len() - 1] {
        match node.get_mut(*p) {
            Some(next) => node = next,
            None => return Ok(false),
        }
    }
    let leaf = path[path.len() - 1];
    match node.get(leaf) {
        None | Some(serde_json::Value::Null) => return Ok(false),
        Some(serde_json::Value::String(s)) if s.is_empty() => return Ok(false),
        _ => {}
    }
    let Some(obj) = node.as_object_mut() else { return Ok(false) };
    let replacement = match obj.get(leaf) {
        Some(serde_json::Value::String(_)) => serde_json::Value::String(String::new()),
        _ => serde_json::Value::Null,
    };
    obj.insert(leaf.into(), replacement);
    sqlx::query("update app_settings set value = $1, updated_at = now() where key = $2")
        .bind(&value)
        .bind(key)
        .execute(tx.as_mut())
        .await
        .map_err(ClearError::from)?;
    tx.commit().await.map_err(ClearError::from)?;
    Ok(true)
}

/// Clear every row the probe reports as unreadable. Re-probes first, so what
/// is cleared is what is broken NOW — not what a stale page thought was
/// broken.
pub async fn clear_unreadable(
    pg: &PgPool,
    sb: &SecretBox,
    root: &crate::config::SecretRoot,
) -> (Vec<String>, Vec<String>) {
    let health = secret_health(pg, sb, root).await;
    let mut cleared = Vec::new();
    let mut failed = Vec::new();
    let Some(list) = health.get("rows").and_then(|r| r.as_array()) else {
        return (cleared, failed);
    };
    for r in list {
        if r.get("state") != Some(&serde_json::json!("unreadable"))
            || r.get("clearable") != Some(&serde_json::json!(true))
        {
            continue;
        }
        let (Some(id), Some(label)) = (
            r.get("id").and_then(|v| v.as_str()),
            r.get("label").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        match clear_secret(pg, id).await {
            // One store refusing must not strand the rest — the operator came
            // here to get unstuck, and a partial clear beats an aborted one.
            Ok(true) => cleared.push(label.to_string()),
            Ok(false) => {}
            Err(_) => {
                tracing::error!("[secrets] clear failed {id}");
                failed.push(label.to_string());
            }
        }
    }
    (cleared, failed)
}
