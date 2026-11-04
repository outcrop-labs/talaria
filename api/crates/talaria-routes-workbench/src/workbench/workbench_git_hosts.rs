// /api/workbench/git-hosts. The git hosts the operator chose
// (talaria-git-hosts): GET → every registered host (redacted — whether a
// secret is set, never its value) with what still uses it, and the kinds an
// operator can add; POST → add one. Deliberately requireAdmin (not
// agents.manage): a host holds ORG CREDENTIALS (a GitHub App private key or
// PAT, or names the workspace secret that holds a Forgejo token) — a
// grantable permission shouldn't reach them.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    enum_member, object_msg, optional_enum_member, optional_max_string_member,
    optional_string_array_member, parse, present_nullable_max_string_member, string_member,
    zod_type_name,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::HostKind;
use talaria_git_hosts::registry::{self, NewHost};
use talaria_github::{GithubConfigPatch, PatchField};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

/// A nullable-optional enum — the tri-state: absent is "don't touch"
/// (None), present-null is "clear it" (Some(None)), a present value must be
/// one of the options.
pub(crate) fn present_nullable_enum_member(
    obj: &Map<String, Value>,
    key: &str,
    options: &[&str],
) -> Result<Option<Option<String>>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => optional_enum_member(obj, key, options).map(Some),
    }
}

/// A GitHub host's connection fields, as they arrive: `mode`, `pat.token`,
/// `app.{appId, installationIds, privateKey}` — owned, so the engine's
/// borrowed patch can be made from them.
#[derive(Default)]
pub(crate) struct GithubBody {
    mode: Option<Option<String>>,
    pat_token: Option<Option<String>>,
    app_id: Option<String>,
    installation_ids: Option<Vec<String>>,
    private_key: Option<Option<String>>,
}

fn field(v: &Option<Option<String>>) -> PatchField<&str> {
    match v {
        None => PatchField::Unset,
        Some(None) => PatchField::Set(None),
        Some(Some(s)) => PatchField::Set(Some(s.as_str())),
    }
}

impl GithubBody {
    /// The engine's patch. `ids` keeps the borrowed installation ids alive.
    pub(crate) fn patch<'a>(&'a self, ids: &'a Option<Vec<&'a str>>) -> GithubConfigPatch<'a> {
        GithubConfigPatch {
            mode: field(&self.mode),
            pat_token: field(&self.pat_token),
            app_id: self.app_id.as_deref(),
            installation_ids: ids.as_deref(),
            private_key: field(&self.private_key),
        }
    }

    pub(crate) fn ids(&self) -> Option<Vec<&str>> {
        self.installation_ids
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect())
    }
}

/// The `github` member of a host body. `pat` and `app` are optional
/// OBJECTS — present-but-wrong type answers the object message.
pub(crate) fn github_body(v: &Value) -> Result<GithubBody, String> {
    let Some(obj) = v.as_object() else {
        return Err(object_msg(zod_type_name(v)));
    };
    let mut out = GithubBody {
        mode: present_nullable_enum_member(obj, "mode", &["app", "pat"])?,
        ..Default::default()
    };
    if let Some(v) = obj.get("pat") {
        let Some(m) = v.as_object() else {
            return Err(object_msg(zod_type_name(v)));
        };
        out.pat_token = present_nullable_max_string_member(m, "token", 400)?;
    }
    if let Some(v) = obj.get("app") {
        let Some(m) = v.as_object() else {
            return Err(object_msg(zod_type_name(v)));
        };
        out.app_id = optional_max_string_member(m, "appId", 40)?;
        out.installation_ids = optional_string_array_member(m, "installationIds", 0, 40, 20)?;
        out.private_key = present_nullable_max_string_member(m, "privateKey", 20_000)?;
    }
    Ok(out)
}

pub(crate) fn audit(state: &AppState, actor: String, action: &'static str, id: String) {
    let pg = state.pg.clone();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action,
                target_type: "git_host",
                target_id: Some(&id),
                target_label: None,
                before: None,
                after: None,
            },
        )
        .await;
    });
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let rows = match registry::host_rows(&state.pg).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[workbench/git-hosts] host read failed", e)),
    };
    let mut hosts = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut wire = row.wire();
        match registry::host_use(&state.pg, &row.id).await {
            Ok(used) => wire["inUse"] = json!(used),
            Err(e) => return Ok(internal("[workbench/git-hosts] use read failed", e)),
        }
        hosts.push(wire);
    }
    let kinds: Vec<Value> = HostKind::ALL
        .iter()
        .map(|k| json!({ "id": k.as_str(), "label": k.label() }))
        .collect();
    Ok(Json(json!({ "hosts": hosts, "kinds": kinds })).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let bad = |msg: String| house_error(StatusCode::BAD_REQUEST, &msg);
    let id = match string_member(obj, "id", 1, 40) {
        Ok(v) => v,
        Err(msg) => return Ok(bad(msg)),
    };
    let kind = match enum_member(obj, "kind", &["github", "forgejo"]) {
        Ok(v) => HostKind::parse(&v).unwrap_or(HostKind::Github),
        Err(msg) => return Ok(bad(msg)),
    };
    let name = match string_member(obj, "name", 1, 80) {
        Ok(v) => v,
        Err(msg) => return Ok(bad(msg)),
    };
    let base_url = match string_member(obj, "baseUrl", 1, 300) {
        Ok(v) => v,
        Err(msg) => return Ok(bad(msg)),
    };
    let api_url = match optional_max_string_member(obj, "apiUrl", 300) {
        Ok(v) => v,
        Err(msg) => return Ok(bad(msg)),
    };
    let orgs = match optional_string_array_member(obj, "repoCreationOrgs", 1, 100, 20) {
        Ok(v) => v.unwrap_or_default(),
        Err(msg) => return Ok(bad(msg)),
    };
    let secret = match optional_max_string_member(obj, "secret", 200) {
        Ok(v) => v,
        Err(msg) => return Ok(bad(msg)),
    };
    let github = match obj.get("github") {
        None => None,
        Some(v) => match github_body(v) {
            Ok(b) => Some(b),
            Err(msg) => return Ok(bad(msg)),
        },
    };
    let ids = github.as_ref().and_then(GithubBody::ids);
    let sb = state.secretbox().await.unwrap_or_default();
    let actor = actor_of(&user);
    let created = registry::create_host(
        &state.pg,
        &sb,
        &NewHost {
            id: id.trim(),
            kind,
            name: &name,
            base_url: &base_url,
            api_url: api_url.as_deref(),
            repo_creation_orgs: &orgs,
            secret: secret.as_deref(),
            github: github.as_ref().map(|g| g.patch(&ids)),
            actor: &actor,
        },
    )
    .await;
    match created {
        Ok(row) => {
            audit(&state, actor, "workbench.git_host_create", row.id.clone());
            let st = talaria_git_hosts::GitHost::status(
                &talaria_git_hosts::connect(&state.pg, &sb, &row).await,
            )
            .await;
            Ok((
                StatusCode::CREATED,
                Json(json!({ "host": row.wire_with(&st) })),
            )
                .into_response())
        }
        Err(msg) => Ok(bad(msg)),
    }
}

#[cfg(test)]
mod tests {
    use talaria_body::{array_too_big_msg, enum_msg, too_big_msg};

    use super::*;

    #[test]
    fn enum_member_tri_state() {
        let mut m = Map::new();
        assert!(
            present_nullable_enum_member(&m, "mode", &["app", "pat"])
                .unwrap()
                .is_none()
        );
        m.insert("mode".into(), Value::Null);
        assert_eq!(
            present_nullable_enum_member(&m, "mode", &["app", "pat"]).unwrap(),
            Some(None)
        );
        m.insert("mode".into(), json!("pat"));
        assert_eq!(
            present_nullable_enum_member(&m, "mode", &["app", "pat"]).unwrap(),
            Some(Some("pat".into()))
        );
        m.insert("mode".into(), json!("oauth"));
        assert_eq!(
            present_nullable_enum_member(&m, "mode", &["app", "pat"]).unwrap_err(),
            enum_msg(&["app", "pat"])
        );
    }

    #[test]
    fn a_github_body_reads_every_tri_state_and_caps() {
        let b = github_body(&json!({
            "mode": "app",
            "pat": { "token": null },
            "app": { "appId": "123", "installationIds": ["9"], "privateKey": "" },
        }))
        .unwrap();
        assert_eq!(b.mode, Some(Some("app".into())));
        assert_eq!(b.pat_token, Some(None));
        assert_eq!(b.app_id.as_deref(), Some("123"));
        assert_eq!(b.private_key, Some(Some(String::new())));
        let ids = b.ids();
        let patch = b.patch(&ids);
        assert!(matches!(patch.pat_token, PatchField::Set(None)));
        assert_eq!(patch.installation_ids, Some(&["9"][..]));
        assert!(github_body(&json!("x")).is_err());
        assert!(github_body(&json!({ "pat": "x" })).is_err());
        let long = "x".repeat(401);
        assert_eq!(
            github_body(&json!({ "pat": { "token": long } }))
                .err()
                .unwrap(),
            too_big_msg(400)
        );
        let ids: Vec<Value> = (0..21).map(|i| json!(i.to_string())).collect();
        assert_eq!(
            github_body(&json!({ "app": { "installationIds": ids } }))
                .err()
                .unwrap(),
            array_too_big_msg(20)
        );
    }
}
