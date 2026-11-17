// /api/workbench/permission-requests. The GitHub permissions agents asked
// for (`request_git_permission`; talaria-git-hosts' requests). GET → every
// open request (pending, or approved and waiting on GitHub), each with what
// GitHub's side holds, every repo the agent holds on that host (an approval
// applies to all of them) and its ticket's column; PUT {id, action:
// approve|deny, note?} → approve merges the permission into the agent's set
// on the host, `granted` when the installation holds it and
// `awaiting_github` when it does not (a holdings refresh flips it); deny
// closes it. Either way the ticket gets a line and the agent's managers a
// notice.
//
// ADMIN, by session only (an agent's or a personal key is a 401), and for a
// request raised on a ticket, an admin who may read that board's work: the
// census's content half (talaria-approvals' `git_permission_authority`, the
// one answer both use). Any other admin is shown that a request waits and
// nothing of what it says.
//
// Nothing here moves a ticket or wakes an agent. The page's "post my note in
// the ticket thread" is the decider's own message through the chat door, and
// its Restart is the board's own human move: both are the person's calls
// from the page, never this route's.

use std::collections::HashMap;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_approvals::{
    git_permission_authority, may_decide_content, resolve_approval, resolve_disclosures,
};
use talaria_body::{enum_member, optional_max_string_member, parse, uuid_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::requests::{
    self, DecideError, Decided, Decider, PermissionRequest, github_side,
};
use talaria_notify::{
    ActorKind, NotificationInput, NotifyActor, NotifyDeps, NotifyMeta, add_notification_with,
};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

/// Who a decision's line and notice are from.
pub(crate) enum By<'a> {
    /// An admin: their audit actor, their user id, and their name for the
    /// notice.
    Admin {
        actor: &'a str,
        user_id: &'a str,
        name: &'a str,
    },
    /// The holdings refresh, when GitHub came to hold one an admin approved,
    /// or an admin took the permission out of the agent's set first.
    Github,
}

/// The ticket line a decision writes. Pure, so its wording is pinned. The
/// note is the decider's, and the ticket's readers are the board's, which is
/// whom the note was written for.
pub(crate) fn decision_line(req: &PermissionRequest, was: &str, note: Option<&str>) -> String {
    let agent = req.agent_label();
    let host = req.host_label();
    let asked = req.asked();
    let line = match (req.status.as_str(), was) {
        ("granted", "awaiting_github") => format!(
            "GitHub now holds the permission {asked} for {agent} on {host}: the next git credential carries it"
        ),
        ("granted", _) => format!(
            "approved the GitHub permission {asked} for {agent} on {host} — it applies to every repo {agent} holds there, and the next git credential carries it"
        ),
        ("awaiting_github", _) => format!(
            "approved the GitHub permission {asked} for {agent} on {host} — waiting on GitHub: the App's installation has not accepted it, and no token carries it until an owner of the account does"
        ),
        ("denied", "awaiting_github") => format!(
            "closed {agent}'s request for the GitHub permission {asked} on {host}, which was waiting on GitHub"
        ),
        ("denied", _) => {
            format!("denied the GitHub permission {asked} that {agent} asked for on {host}")
        }
        _ => format!(
            "withdrew {agent}'s request for the GitHub permission {asked} on {host}: it was taken out of the agent's permissions before GitHub held it"
        ),
    };
    match note.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("{line}: {n}"),
        None => line,
    }
}

/// The managers' notice: who, what, where and the outcome — never the
/// agent's words or the decider's note, which quote the board's work and the
/// managers may not be on that board.
pub(crate) fn notice_title(req: &PermissionRequest) -> String {
    let agent = req.agent_label();
    let host = req.host_label();
    let asked = req.asked();
    match req.status.as_str() {
        "granted" => format!("{agent} now carries {asked} on {host}"),
        "awaiting_github" => {
            format!("{agent}'s {asked} on {host} was approved — waiting on GitHub")
        }
        "denied" => format!("{agent}'s request for {asked} on {host} was denied"),
        _ => format!("{agent}'s request for {asked} on {host} was withdrawn"),
    }
}

/// Everything a decision owes people, after its write committed: the ticket
/// line, the approval's notifications read once it is no longer open, and a
/// notice to each of the agent's managers but the decider. Each step is
/// logged on failure, never undone: the decision is the row.
pub(crate) async fn tell_decided(
    pg: &sqlx::PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    req: &PermissionRequest,
    was: &str,
    by: By<'_>,
    note: Option<&str>,
) {
    let (actor, name, decider) = match &by {
        By::Admin {
            actor,
            user_id,
            name,
        } => (*actor, *name, Some(*user_id)),
        By::Github => ("talaria", "GitHub", None),
    };
    if let Some(task_id) = &req.task_id
        && let Err(e) = talaria_tasks::log_activity(
            pg,
            task_id,
            actor,
            "workbench",
            &decision_line(req, was, note),
        )
        .await
    {
        tracing::warn!(
            "[workbench/permission-requests] the line on {task_id} for {} failed: {e}",
            req.id
        );
    }
    let notify = NotifyDeps::publishing(pg.clone(), redis);
    if !req.is_open() {
        resolve_approval(&notify, &req.approval_key()).await;
    }
    let managers = match talaria_agent_managers::list_managers(pg, &req.agent_id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                "[workbench/permission-requests] {}'s managers could not be read: {e}",
                req.agent_label()
            );
            return;
        }
    };
    let title = notice_title(req);
    let body = format!(
        "Asked for where it worked on {}. Decided by {name}.",
        req.repo
    );
    let meta = NotifyMeta {
        actor: Some(match &by {
            By::Admin { user_id, name, .. } => NotifyActor {
                kind: ActorKind::User,
                id: (*user_id).to_string(),
                label: (*name).to_string(),
            },
            By::Github => NotifyActor {
                kind: ActorKind::System,
                id: "git-hosts".into(),
                label: "GitHub".into(),
            },
        }),
        ..Default::default()
    };
    // The decider knows: a manager who decided is not told their own news.
    for m in managers
        .into_iter()
        .filter(|m| Some(m.user_id.as_str()) != decider)
    {
        if let Err(e) = add_notification_with(
            &notify,
            &m.user_id,
            &NotificationInput {
                kind: "git_permission_decided",
                title: &title,
                body: Some(&body),
                href: Some("/fleet"),
            },
            &meta,
        )
        .await
        {
            tracing::warn!(
                "[workbench/permission-requests] the notice to {} failed: {e}",
                m.user_id
            );
        }
    }
}

/// Point talaria-git-hosts' settled-request sink at this process: a request
/// the holdings refresh settles (GitHub came to hold it, or it was taken out
/// of the agent's set first) gets its line, its notice and its approval read,
/// as an admin's decision does. Called once at boot.
pub fn wire_settled(state: &AppState) {
    let state = state.clone();
    let _ = requests::SETTLED.set(std::sync::Arc::new(
        move |settled: Vec<PermissionRequest>| {
            let state = state.clone();
            Box::pin(async move {
                let redis = state.redis().await.ok();
                for req in settled {
                    tell_decided(
                        &state.pg,
                        redis.clone(),
                        &req,
                        "awaiting_github",
                        By::Github,
                        None,
                    )
                    .await;
                }
            })
        },
    ));
}

/// The ticket a request was raised on, as the page shows it: its column, and
/// where a Restart would move it (the board's first working column, else its
/// pickup queue) when it is Blocked.
async fn ticket_wire(
    pg: &sqlx::PgPool,
    req: &PermissionRequest,
    boards: &mut HashMap<String, Vec<talaria_statuses::BoardStatus>>,
) -> Value {
    let (Some(task_id), Some(board_id)) = (&req.task_id, &req.board_id) else {
        return Value::Null;
    };
    if !boards.contains_key(board_id) {
        let list = talaria_statuses::list_statuses(pg, board_id)
            .await
            .unwrap_or_default();
        boards.insert(board_id.clone(), list);
    }
    let list = boards.get(board_id).map(Vec::as_slice).unwrap_or_default();
    let status = req.ticket_status.clone().unwrap_or_default();
    let label = list
        .iter()
        .find(|s| s.key == status)
        .map(|s| s.label.clone())
        .unwrap_or_else(|| status.clone());
    let blocked = status == "blocked";
    let restart = if blocked {
        let meta = talaria_statuses::meta_of(list);
        meta.active_key.or(meta.assigned_key)
    } else {
        None
    };
    json!({
        "id": task_id,
        "ref": req.ticket_ref,
        "title": req.ticket_title,
        "boardId": board_id,
        "status": status,
        "statusLabel": label,
        "blocked": blocked,
        "restartStatus": restart,
    })
}

/// One request as a decider reads it.
async fn request_wire(
    pg: &sqlx::PgPool,
    req: &PermissionRequest,
    boards: &mut HashMap<String, Vec<talaria_statuses::BoardStatus>>,
) -> Value {
    let side = github_side(pg, req).await;
    let in_set =
        match talaria_git_hosts::permissions::host_permissions(pg, &req.agent_id, &req.host_id)
            .await
        {
            Ok(set) => talaria_git_hosts::Level::parse(&req.level)
                .map(|l| set.get(&req.permission).is_some_and(|h| h >= l)),
            Err(_) => None,
        };
    let applies_to: Vec<String> = talaria_git_hosts::granted_repos(pg, &req.agent_id)
        .await
        .into_iter()
        .filter(|g| g.host_id == req.host_id)
        .map(|g| g.repo)
        .collect();
    json!({
        "id": req.id,
        "content": true,
        "status": req.status,
        "host": req.host_id,
        "hostName": req.host_label(),
        "repo": req.repo,
        "permission": req.permission,
        "level": req.level,
        "agentId": req.agent_id,
        "agentName": req.agent_label(),
        "agentModel": req.agent_model,
        "why": req.why,
        "evidence": req.evidence,
        "held": side.held,
        "installation": side.installation,
        "installationUrl": side.settings_url,
        "inSet": in_set,
        "appliesTo": applies_to,
        "ticket": ticket_wire(pg, req, boards).await,
        "decidedBy": req.decided_by_name,
        "decisionNote": req.decision_note,
        "createdAt": epoch_ms_to_iso(req.created_ms),
        "updatedAt": epoch_ms_to_iso(req.updated_ms),
    })
}

/// One request as an admin who may not read it sees it: that it waits, on
/// which host, since when — nothing it says.
fn fact_wire(req: &PermissionRequest) -> Value {
    json!({
        "id": req.id,
        "content": false,
        "status": req.status,
        "host": req.host_id,
        "hostName": req.host_label(),
        "createdAt": epoch_ms_to_iso(req.created_ms),
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let open = match requests::open_requests(&state.pg).await {
        Ok(r) => r,
        Err(e) => {
            return Ok(internal(
                "[workbench/permission-requests] queue read failed",
                e,
            ));
        }
    };
    let authorities: Vec<_> = open
        .iter()
        .map(|r| git_permission_authority(r.board_id.clone()))
        .collect();
    // "Could not say" is nobody: a failed resolve shows every request as a
    // fact only, never its words.
    let who = resolve_disclosures(&state.pg, &authorities)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("[workbench/permission-requests] audience read failed: {e}");
            Vec::new()
        });
    let mut boards = HashMap::new();
    let mut out = Vec::with_capacity(open.len());
    for (i, req) in open.iter().enumerate() {
        let may = who.get(i).is_some_and(|d| may_decide_content(d, &user.id));
        out.push(if may {
            request_wire(&state.pg, req, &mut boards).await
        } else {
            fact_wire(req)
        });
    }
    Ok(Json(json!({ "requests": out })).into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let id = match uuid_member(obj, "id") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let action = match enum_member(obj, "action", &["approve", "deny"]) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Why the decider decided: on the ticket, and in list_repos for the
    // agent.
    let note = match optional_max_string_member(obj, "note", 500) {
        Ok(v) => v.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let req = match requests::get_request(&state.pg, &id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "no such request")),
        Err(e) => {
            return Ok(internal(
                "[workbench/permission-requests] request read failed",
                e,
            ));
        }
    };
    // The decider must be one who may read what the agent wrote: the census
    // asks the same question of the same authority.
    let who =
        talaria_approvals::audience_for(&state.pg, &git_permission_authority(req.board_id.clone()))
            .await;
    if !may_decide_content(&who, &user.id) {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "this request was raised on a board you are not a member of — an admin who is decides it",
        ));
    }
    let actor = actor_of(&user);
    let by = Decider {
        actor: &actor,
        user_id: &user.id,
    };
    let decided = match action.as_str() {
        "approve" => requests::approve(&state.pg, &id, &by, note.as_deref()).await,
        _ => requests::deny(&state.pg, &id, &by, note.as_deref()).await,
    };
    let Decided {
        request,
        permissions,
        repos,
    } = match decided {
        Ok(d) => d,
        Err(DecideError::NotOpen) => {
            return Ok(house_error(
                StatusCode::CONFLICT,
                "this request is already decided — read the queue again",
            ));
        }
        Err(DecideError::GrantGone) => {
            return Ok(house_error(
                StatusCode::CONFLICT,
                &format!(
                    "{} no longer holds a repo on {}, so the permission would apply to nothing — grant it a repo there first",
                    req.agent_label(),
                    req.host_label()
                ),
            ));
        }
        Err(DecideError::Failed(e)) => {
            return Ok(internal(
                "[workbench/permission-requests] decision write failed",
                e,
            ));
        }
    };
    let name = user.name.clone().unwrap_or_else(|| actor.clone());
    tell_decided(
        &state.pg,
        state.redis().await.ok(),
        &request,
        &req.status,
        By::Admin {
            actor: &actor,
            user_id: &user.id,
            name: &name,
        },
        note.as_deref(),
    )
    .await;
    Ok(Json(json!({
        "ok": true,
        "status": request.status,
        "permissions": permissions.map(|p| p.to_json()),
        "appliesTo": repos,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(status: &str) -> PermissionRequest {
        PermissionRequest {
            id: "r1".into(),
            agent_id: "a1".into(),
            agent_model: "dev-billy".into(),
            agent_name: Some("Billy".into()),
            host_id: "github".into(),
            host_name: Some("GitHub".into()),
            repo: "acme/site".into(),
            permission: "workflows".into(),
            level: "write".into(),
            why: "CI must change".into(),
            evidence: json!([]),
            task_id: Some("t1".into()),
            ticket_ref: Some("WEB-1".into()),
            ticket_title: Some("Fix CI".into()),
            board_id: Some("b1".into()),
            ticket_status: Some("blocked".into()),
            status: status.into(),
            decided_by: None,
            decided_by_name: None,
            decision_note: None,
            created_ms: 0,
            updated_ms: 0,
            decided_ms: None,
        }
    }

    #[test]
    fn a_decision_line_names_the_agent_the_permission_the_host_and_the_note() {
        assert_eq!(
            decision_line(&req("granted"), "pending", Some("for the deploy")),
            "approved the GitHub permission workflows write for Billy on GitHub — it applies to every repo Billy holds there, and the next git credential carries it: for the deploy"
        );
        assert!(
            decision_line(&req("awaiting_github"), "pending", None).contains("waiting on GitHub")
        );
        assert_eq!(
            decision_line(&req("denied"), "pending", Some("  ")),
            "denied the GitHub permission workflows write that Billy asked for on GitHub"
        );
        assert!(
            decision_line(&req("denied"), "awaiting_github", None)
                .starts_with("closed Billy's request")
        );
        assert!(
            decision_line(&req("granted"), "awaiting_github", None)
                .starts_with("GitHub now holds the permission workflows write for Billy")
        );
        assert!(decision_line(&req("withdrawn"), "awaiting_github", None).starts_with("withdrew"));
    }

    #[test]
    fn the_managers_notice_never_carries_the_agents_words_or_the_note() {
        for status in ["granted", "awaiting_github", "denied", "withdrawn"] {
            let title = notice_title(&req(status));
            assert!(
                title.contains("Billy") && title.contains("workflows write"),
                "{title}"
            );
            assert!(!title.contains("CI must change"), "{title}");
        }
        assert_eq!(
            notice_title(&req("granted")),
            "Billy now carries workflows write on GitHub"
        );
    }

    #[test]
    fn a_fact_says_that_a_request_waits_and_nothing_it_says() {
        let fact = fact_wire(&req("pending"));
        assert_eq!(fact["content"], false);
        for hidden in [
            "why",
            "repo",
            "permission",
            "agentName",
            "ticket",
            "evidence",
        ] {
            assert!(fact.get(hidden).is_none(), "{hidden} in {fact}");
        }
    }

    // ── Live-DB: the route end to end ────────────────────────────────────────
    //
    // #[ignore]d (house rule: no service-dependent tests in the default run;
    // ci.yml's api job sweeps ignored lib tests in its live-DB steps).
    //
    //   DATABASE_URL=postgres://… REDIS_URL=… TALARIA_SECRET_KEY=… \
    //     cargo test -p talaria-routes-workbench permission_requests -- --ignored

    mod live {
        use super::super::*;
        use crate::workbench::live_support::{
            agent, answer, cleanup, github_host, person, state, tag,
        };
        use axum::http::header;

        async fn put_as(state: &AppState, headers: &HeaderMap, body: Value) -> (StatusCode, Value) {
            answer(
                put(
                    State(state.clone()),
                    headers.clone(),
                    Bytes::from(body.to_string()),
                )
                .await,
            )
            .await
        }

        async fn get_as(state: &AppState, headers: &HeaderMap) -> (StatusCode, Value) {
            answer(get(State(state.clone()), headers.clone()).await).await
        }

        /// A board `owner` owns, and a Blocked ticket on it. Returns (board,
        /// ticket).
        async fn ticket(state: &AppState, owner: &str, tag: &str) -> (String, String) {
            let board: String = sqlx::query_scalar(
                "insert into boards (name, owner_id) values ($1, $2::uuid) returning id::text",
            )
            .bind(format!("perm-live-{tag}"))
            .bind(owner)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            sqlx::query(
                "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner') \
                 on conflict do nothing",
            )
            .bind(&board)
            .bind(owner)
            .execute(&state.pg)
            .await
            .unwrap();
            let task: String = sqlx::query_scalar(
                "insert into tasks (board_id, title, status) values ($1::uuid, 'Fix CI', 'blocked') \
                 returning id::text",
            )
            .bind(&board)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            (board, task)
        }

        /// The agent asks for `permissions` on acme/widgets, from `task`.
        async fn ask(
            state: &AppState,
            agent: &str,
            model: &str,
            task: &str,
            permissions: &[(&str, &str)],
        ) -> requests::Filing {
            let permissions: Vec<(String, String)> = permissions
                .iter()
                .map(|(k, l)| (k.to_string(), l.to_string()))
                .collect();
            requests::file_git_permission_request(
                &state.pg,
                &requests::Ask {
                    agent_id: agent,
                    agent_model: model,
                    repo: "acme/widgets",
                    host: None,
                    permissions: &permissions,
                    why: "the deploy workflow must change",
                    task_id: Some(task),
                },
            )
            .await
            .expect("filed")
        }

        fn mine<'a>(body: &'a Value, id: &str) -> &'a Value {
            body["requests"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("request {id} in {body}"))
        }

        #[tokio::test]
        #[ignore = "needs a live database and Redis (DATABASE_URL, REDIS_URL)"]
        async fn an_admin_on_the_board_decides_and_everyone_else_is_told_less() {
            let state = state().await;
            let t = tag();
            let (decider, as_decider) = person(&state, &t, "decider", "admin").await;
            let (stranger, as_stranger) = person(&state, &t, "stranger", "admin").await;
            let (manager, as_manager) = person(&state, &t, "manager", "member").await;
            let a = agent(&state, &t, &manager).await;
            let model: String =
                sqlx::query_scalar("select model from agent_defs where id = $1::uuid")
                    .bind(&a)
                    .fetch_one(&state.pg)
                    .await
                    .unwrap();
            // A host that is never asked anything: approval reads the record.
            let h = github_host(&state, &t, "http://127.0.0.1:9", "95101").await;
            talaria_git_hosts::add_grant(&state.pg, &a, &h, "acme/widgets")
                .await
                .unwrap();
            let (board, task) = ticket(&state, &decider, &t).await;
            let filing = ask(&state, &a, &model, &task, &[("workflows", "write")]).await;
            let requests::Raised::Filed { id, status } = &filing.permissions[0].raised else {
                panic!("{filing:?}")
            };
            assert_eq!(status, "pending");
            let id = id.clone();

            // An agent's key is not a session: 401. A member: 403.
            let mut as_agent = HeaderMap::new();
            as_agent.insert(
                header::AUTHORIZATION,
                "Bearer tk_agent_not_a_session".parse().unwrap(),
            );
            let (status, _) = get_as(&state, &as_agent).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            let (status, _) =
                put_as(&state, &as_agent, json!({ "id": id, "action": "approve" })).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            let (status, _) = put_as(
                &state,
                &as_manager,
                json!({ "id": id, "action": "approve" }),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);

            // An admin off the board sees that it waits, and may not decide.
            let (status, body) = get_as(&state, &as_stranger).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let fact = mine(&body, &id);
            assert_eq!(fact["content"], false);
            assert!(
                fact.get("why").is_none() && fact.get("permission").is_none(),
                "{fact}"
            );
            let (status, body) = put_as(
                &state,
                &as_stranger,
                json!({ "id": id, "action": "approve" }),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

            // The admin on the board reads all of it.
            let (_, body) = get_as(&state, &as_decider).await;
            let full = mine(&body, &id);
            assert_eq!(full["content"], true);
            assert_eq!(full["why"], "the deploy workflow must change");
            assert_eq!(full["permission"], "workflows");
            assert_eq!(
                full["held"], true,
                "the record says the installation holds it"
            );
            assert_eq!(full["inSet"], false);
            assert_eq!(full["appliesTo"], json!(["acme/widgets"]));
            assert_eq!(full["ticket"]["id"], task.as_str());
            assert_eq!(full["ticket"]["blocked"], true);
            assert_eq!(full["ticket"]["restartStatus"], "in_progress");

            // Approve: the set gains it from the default, the request is
            // granted (the installation holds it), the ticket says so.
            let (status, body) = put_as(
                &state,
                &as_decider,
                json!({ "id": id, "action": "approve", "note": "for the deploy" }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["status"], "granted");
            assert_eq!(
                body["permissions"],
                json!({ "contents": "write", "metadata": "read", "pull_requests": "write", "workflows": "write" })
            );
            assert_eq!(body["appliesTo"], json!(["acme/widgets"]));
            let line: String = sqlx::query_scalar(
                "select description from task_activity where task_id = $1::uuid \
                 order by created_at desc limit 1",
            )
            .bind(&task)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            assert!(
                line.contains("approved the GitHub permission workflows write"),
                "{line}"
            );
            assert!(line.ends_with(": for the deploy"), "{line}");
            // The agent's manager hears the outcome, without the words.
            let notice: String = sqlx::query_scalar(
                "select title from notifications where user_id = $1::uuid \
                 and kind = 'git_permission_decided' order by created_at desc limit 1",
            )
            .bind(&manager)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            assert!(notice.contains("now carries workflows write"), "{notice}");
            // Decided once.
            let (status, _) =
                put_as(&state, &as_decider, json!({ "id": id, "action": "deny" })).await;
            assert_eq!(status, StatusCode::CONFLICT);

            // Deny another: its note is the agent's to read.
            let filing = ask(&state, &a, &model, &task, &[("secrets", "write")]).await;
            let requests::Raised::Filed { id: other, .. } = &filing.permissions[0].raised else {
                panic!("{filing:?}")
            };
            let (status, body) = put_as(
                &state,
                &as_decider,
                json!({ "id": other, "action": "deny", "note": "use the deploy key" }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["status"], "denied");
            let (status, note): (String, Option<String>) = sqlx::query_as(
                "select status, decision_note from workbench_permission_requests where id = $1::uuid",
            )
            .bind(other)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            assert_eq!(
                (status.as_str(), note.as_deref()),
                ("denied", Some("use the deploy key"))
            );

            // The agent's grants on the host went before the approval: 409,
            // and nothing written — not the request, not the set.
            let filing = ask(&state, &a, &model, &task, &[("actions", "write")]).await;
            let requests::Raised::Filed { id: late, .. } = &filing.permissions[0].raised else {
                panic!("{filing:?}")
            };
            sqlx::query("delete from workbench_repos where agent_id = $1::uuid")
                .bind(&a)
                .execute(&state.pg)
                .await
                .unwrap();
            let (status, body) = put_as(
                &state,
                &as_decider,
                json!({ "id": late, "action": "approve" }),
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            assert!(
                body["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("no longer holds a repo")),
                "{body}"
            );
            let still: String = sqlx::query_scalar(
                "select status from workbench_permission_requests where id = $1::uuid",
            )
            .bind(late)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            assert_eq!(still, "pending");
            let set = talaria_git_hosts::permissions::host_permissions(&state.pg, &a, &h)
                .await
                .unwrap();
            assert!(!set.contains("actions"), "{set}");

            sqlx::query("delete from boards where id = $1::uuid")
                .bind(&board)
                .execute(&state.pg)
                .await
                .unwrap();
            cleanup(&state, &[&a], &[&decider, &stranger, &manager], &[&h]).await;
        }
    }
}
