// /api/secrets/git-push-check.
//
// THE PRE-PUSH GATE — the platform's own answer to "whose call is main".
// A git host's own branch protection enforces the posture where it can;
// this route is what the rendered pre-push hook asks, and it works for every
// granted repo on every registered host, including the private ones a free
// plan cannot protect. The hook fires inside the container BEFORE any bytes
// leave for the remote, with the refs git is about to push; this route
// answers allow/deny against the agent's own repo rules, in the same shape
// as the credential route beside it: agent-authenticated, never
// session-authenticated, the decision logged for the operator.
//
// A DENY NEVER LEAVES THE CONTAINER AS DATA: the hook turns it into git's
// own "pre-push declined" with the sentence on stderr, so the agent reads
// the rule where it reads every other git fact.
//
// THE SAME DOOR AS THE CREDENTIAL'S: a proven agent whose Developer switch is
// on (`talaria_agent_auth::agent_is_developer`), then its grant on the host
// the push goes to.
//
// An allowed push is recorded against the run that owns the branch's job, so a
// person can read what a run did outside its workspace. A push to a branch a
// rewind retired is declined: the attempt's own branch is where its work goes.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::{agent_is_developer, require_agent};
use talaria_body::{optional_max_string_member, parse, string_array_member, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::{self as hosts, GitHost};
use talaria_session::secretbox_or_500;
use talaria_state::AppState;

use super::secrets_git_credential::NOT_A_DEVELOPER;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return Ok(gate),
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let repo = match string_member(obj, "repo", 3, 400) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The refs git handed the hook: full ref names, ≤200 chars each, ≤50 of
    // them (one push cannot honestly carry more).
    let refs = match string_array_member(obj, "refs", 1, 200, 0, 50) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The remote's host (`host[:port]`), which the rendered hook sends. A
    // hook rendered before hosts were named sends none, and the agent's
    // grants decide — unambiguous unless the repo is granted on two hosts.
    let authority = match optional_max_string_member(obj, "host", 253) {
        Ok(h) => h.map(|h| h.trim().to_lowercase()).filter(|h| !h.is_empty()),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // No proven identity (the legacy org-wide key) or no grant: the same
    // answer as the credential route, for the same reason — a caller we
    // cannot scope is one we cannot answer for.
    let Some(agent_id) = caller.id.clone() else {
        tracing::warn!("[secrets] a legacy-key caller asked to push to {repo} — refused");
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            "no credential for that host",
        ));
    };
    // The Developer switch, by the same check as the credential door beside
    // this route: an agent that may not have a git credential may not push,
    // and the hook shows it why.
    match agent_is_developer(&state.pg, &caller.model).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                "[secrets] {} asked to push to {repo} but is not a Developer Agent — refused",
                caller.model
            );
            return Ok((
                StatusCode::FORBIDDEN,
                [(axum::http::header::CACHE_CONTROL, "no-store")],
                Json(json!({ "error": NOT_A_DEVELOPER })),
            )
                .into_response());
        }
        Err(e) => return Ok(internal("[secrets] push check developer read failed", e)),
    }
    let host_id = match &authority {
        Some(a) => match hosts::host_row_at(&state.pg, a).await {
            Ok(row) => row.map(|r| r.id),
            Err(e) => return Ok(internal("[secrets] push check host read failed", e)),
        },
        None => hosts::resolve_grant(&state.pg, &agent_id, &repo, None)
            .await
            .ok()
            .map(|g| g.host_id),
    };
    let rule = match &host_id {
        Some(h) => hosts::repo_rule(&state.pg, &agent_id, h, &repo).await,
        None => None,
    };
    let Some(rule) = rule else {
        tracing::warn!(
            "[secrets] {} has no grant covering {repo} — push refused",
            caller.model
        );
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            "no credential for that host",
        ));
    };

    // A null base_branch resolves to the repo's live default — one read of
    // its host per push, on the same login the push itself would spend.
    // Unresolvable means unanswerable: deny, with the operator line.
    let default_branch = match &rule.base_branch {
        Some(b) => b.clone(),
        None => {
            let sb = secretbox_or_500(&state, "[secrets] push check secretbox").await?;
            let resolved = match hosts::host(&state.pg, &sb, &rule.host_id).await {
                Ok(host) => host.default_branch(&repo).await.ok(),
                Err(_) => None,
            };
            match resolved {
                Some(b) => b,
                None => {
                    tracing::warn!(
                        "[secrets] could not resolve the default branch of {repo} — push refused"
                    );
                    return Ok(house_error(
                        StatusCode::BAD_GATEWAY,
                        "could not resolve the repo's default branch — try again",
                    ));
                }
            }
        }
    };

    for ref_name in &refs {
        if let Err(sentence) = hosts::push_allowed(&rule, &default_branch, ref_name) {
            tracing::warn!(
                "[secrets] {} push to {repo}:{ref_name} declined — {sentence}",
                caller.model
            );
            return Ok((
                StatusCode::FORBIDDEN,
                [(axum::http::header::CACHE_CONTROL, "no-store")],
                Json(json!({ "error": sentence })),
            )
                .into_response());
        }
    }
    // Like every read above, a ledger that cannot answer declines the push: an
    // unread attribution would let a retired branch through unrecorded.
    match rewind_verdict(&state.pg, &agent_id, &rule.host_id, &repo, &refs).await {
        Ok(None) => {}
        Ok(Some(sentence)) => {
            tracing::warn!(
                "[secrets] {} push to {repo} declined — {sentence}",
                caller.model
            );
            return Ok((
                StatusCode::FORBIDDEN,
                [(axum::http::header::CACHE_CONTROL, "no-store")],
                Json(json!({ "error": sentence })),
            )
                .into_response());
        }
        Err(e) => return Ok(internal("[secrets] push check attribution failed", e)),
    }
    tracing::warn!(
        "[secrets] {} push to {repo} ({} ref(s)) allowed by repo rules",
        caller.model,
        refs.len()
    );
    Ok((
        StatusCode::OK,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(json!({ "ok": true })),
    )
        .into_response())
}

#[derive(sqlx::FromRow)]
struct BranchJob {
    id: String,
    run_id: Option<String>,
    status: String,
    branch: String,
    superseded_by: Option<String>,
}

async fn deciding_job_per_branch(
    pg: &sqlx::PgPool,
    agent_id: &str,
    host_id: &str,
    repo: &str,
    branches: &[&str],
) -> Result<Vec<BranchJob>, sqlx::Error> {
    sqlx::query_as(
        "select distinct on (branch) id::text as id, run_id::text as run_id, status, branch, \
                superseded_by::text as superseded_by \
         from workbench_jobs \
         where agent_id = $1::uuid and host_id = $2 and lower(repo) = lower($3) \
           and branch = any($4) \
         order by branch, \
                  status in ('started', 'queued', 'awaiting_approval', 'pr_open') desc, \
                  created_at desc",
    )
    .bind(agent_id)
    .bind(host_id)
    .bind(repo)
    .bind(branches)
    .fetch_all(pg)
    .await
}

/// The rewind ledger's word on a push the repo rules allowed. `Some` is the
/// sentence declining a branch whose job a rewind retired. `None` lets the push
/// through, recorded against the run of each job that owns one of its branches.
/// A read or a record that fails is an `Err`, never a `None`.
async fn rewind_verdict(
    pg: &sqlx::PgPool,
    agent_id: &str,
    host_id: &str,
    repo: &str,
    refs: &[String],
) -> Result<Option<String>, sqlx::Error> {
    let branches: Vec<&str> = refs
        .iter()
        .filter_map(|r| r.strip_prefix("refs/heads/"))
        .collect();
    let jobs = deciding_job_per_branch(pg, agent_id, host_id, repo, &branches).await?;
    if let Some(retired) = jobs.iter().find(|j| j.status == "superseded") {
        let current = retired
            .superseded_by
            .as_deref()
            .map_or(String::new(), |id| {
                format!(" — this work continues in job {id}")
            });
        return Ok(Some(format!(
            "{} belongs to job {}, which a rewind superseded{current}",
            retired.branch, retired.id,
        )));
    }
    record_pushes(pg, &jobs).await?;
    Ok(None)
}

/// One transaction: a push with several branches is recorded whole or not at all.
async fn record_pushes(pg: &sqlx::PgPool, jobs: &[BranchJob]) -> Result<(), sqlx::Error> {
    if jobs.iter().all(|j| j.run_id.is_none()) {
        return Ok(());
    }
    let mut tx = pg.begin().await?;
    for job in jobs {
        let Some(run_id) = &job.run_id else {
            continue;
        };
        sqlx::query(
            "insert into run_pushes (run_id, job_id, branch) values ($1::uuid, $2::uuid, $3)",
        )
        .bind(run_id)
        .bind(&job.id)
        .bind(&job.branch)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "push-check-test";
    const BRANCH: &str = "talaria/push-check";

    async fn job(pg: &sqlx::PgPool, agent: &str, status: &str, age_mins: i32) -> String {
        sqlx::query_scalar(
            "insert into workbench_jobs \
               (agent_id, agent_model, host_id, repo, branch, status, created_at) \
             values ($1::uuid, $2, 'github', 'acme/widgets', $3, $4, \
                     now() - make_interval(mins => $5)) \
             returning id::text",
        )
        .bind(agent)
        .bind(MODEL)
        .bind(BRANCH)
        .bind(status)
        .bind(age_mins)
        .fetch_one(pg)
        .await
        .unwrap()
    }

    async fn deciding(pg: &sqlx::PgPool, agent: &str) -> String {
        let jobs = deciding_job_per_branch(pg, agent, "github", "acme/widgets", &[BRANCH])
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);
        jobs[0].id.clone()
    }

    async fn reset(pg: &sqlx::PgPool) {
        sqlx::query("delete from agent_defs where model = $1")
            .bind(MODEL)
            .execute(pg)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "needs a live database (DATABASE_URL)"]
    async fn the_live_job_decides_a_branch_whatever_its_age() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pg = sqlx::PgPool::connect(&url).await.unwrap();
        reset(&pg).await;
        let agent: String = sqlx::query_scalar(
            "insert into agent_defs (slug, department, model, display_name) \
             values ($1, $1, $1, 'Push') returning id::text",
        )
        .bind(MODEL)
        .fetch_one(&pg)
        .await
        .unwrap();

        let resumed = job(&pg, &agent, "started", 60).await;
        let retired = job(&pg, &agent, "superseded", 30).await;
        assert_eq!(deciding(&pg, &agent).await, resumed);

        sqlx::query("update workbench_jobs set status = 'abandoned' where id = $1::uuid")
            .bind(&resumed)
            .execute(&pg)
            .await
            .unwrap();
        assert_eq!(
            deciding(&pg, &agent).await,
            retired,
            "with nothing live, the newest job still speaks"
        );
        reset(&pg).await;
    }

    /// The route answers this `Err` with a 500, which the hook turns into a
    /// declined push. No database here: the pool points at a closed port.
    #[tokio::test]
    async fn a_ledger_that_cannot_be_read_never_allows_a_push() {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(300))
            .connect_lazy("postgres://nobody@127.0.0.1:9/none")
            .unwrap();
        let refs = vec![format!("refs/heads/{BRANCH}")];
        let agent = "6113b5bc-6935-46e1-a67c-60bd58daf4f0";
        let verdict = rewind_verdict(&pg, agent, "github", "acme/widgets", &refs).await;
        assert!(verdict.is_err(), "{verdict:?}");
    }

    const RECORD_MODEL: &str = "push-record-test";

    async fn sweep_record(pg: &sqlx::PgPool) {
        sqlx::query("delete from runs where input->>'agentModel' = $1")
            .bind(RECORD_MODEL)
            .execute(pg)
            .await
            .unwrap();
        sqlx::query("delete from agent_defs where model = $1")
            .bind(RECORD_MODEL)
            .execute(pg)
            .await
            .unwrap();
    }

    async fn pushes_of(pg: &sqlx::PgPool, run: &str) -> Vec<String> {
        sqlx::query_scalar(
            "select branch from run_pushes where run_id = $1::uuid order by created_at",
        )
        .bind(run)
        .fetch_all(pg)
        .await
        .unwrap()
    }

    #[tokio::test]
    #[ignore = "needs a live database (DATABASE_URL)"]
    async fn an_allowed_push_lands_on_its_run_and_a_retired_branch_is_declined() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pg = sqlx::PgPool::connect(&url).await.unwrap();
        sweep_record(&pg).await;
        let agent: String = sqlx::query_scalar(
            "insert into agent_defs (slug, department, model, display_name) \
             values ($1, $1, $1, 'Push record') returning id::text",
        )
        .bind(RECORD_MODEL)
        .fetch_one(&pg)
        .await
        .unwrap();
        let run: String = sqlx::query_scalar(
            "insert into runs (kind, state, input) \
             values ('work-session', 'running', jsonb_build_object('agentModel', $1::text)) \
             returning id::text",
        )
        .bind(RECORD_MODEL)
        .fetch_one(&pg)
        .await
        .unwrap();
        let owned: String = sqlx::query_scalar(
            "insert into workbench_jobs \
               (agent_id, agent_model, host_id, repo, branch, status, run_id) \
             values ($1::uuid, $2, 'github', 'acme/widgets', $3, 'started', $4::uuid) \
             returning id::text",
        )
        .bind(&agent)
        .bind(RECORD_MODEL)
        .bind(BRANCH)
        .bind(&run)
        .fetch_one(&pg)
        .await
        .unwrap();
        let refs = vec![format!("refs/heads/{BRANCH}"), "refs/tags/v1".to_string()];

        let verdict = rewind_verdict(&pg, &agent, "github", "acme/widgets", &refs).await;
        assert_eq!(verdict.unwrap(), None);
        assert_eq!(pushes_of(&pg, &run).await, vec![BRANCH.to_string()]);

        sqlx::query("update workbench_jobs set status = 'superseded' where id = $1::uuid")
            .bind(&owned)
            .execute(&pg)
            .await
            .unwrap();
        let declined = rewind_verdict(&pg, &agent, "github", "acme/widgets", &refs)
            .await
            .unwrap()
            .expect("a retired branch is declined");
        assert!(
            declined.contains(&format!("job {owned}, which a rewind superseded")),
            "{declined}"
        );
        assert_eq!(
            pushes_of(&pg, &run).await,
            vec![BRANCH.to_string()],
            "a declined push records nothing"
        );
        sweep_record(&pg).await;
    }
}
