// An update waiting for an admin: the `app_update` approval.
//
// The maintenance pass raises it when a plugin's branch moves past the
// commit installed (a merged PR), one per plugin, following the branch while
// it waits. An admin decides it in Manage → Apps: approval runs talaria-apps'
// `update_app` — fetch, build and probe beside the live app, swap, roll back
// if the new version fails — and the outcome is a line on the plugin's
// ticket. A plugin on `update_policy = 'auto'` never waits here: the pass
// applies the update itself and records the outcome as an already-decided
// request.

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_apps::{AppHost, PlatformCredentials};
use talaria_tasks::log_activity;

use crate::short_sha;
use crate::upkeep::apply_update;

/// One waiting update, as the admin queue shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRequestRow {
    pub id: String,
    pub app_slug: String,
    pub repo: String,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub from_sha: Option<String>,
    pub target_sha: String,
    /// The merged PR numbers that moved the branch, as the pass saw them.
    pub prs: Value,
    pub maintainer: Option<String>,
    pub board_name: Option<String>,
    pub created_at: String,
}

type UpdateRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    Value,
    Option<String>,
    Option<String>,
    i64,
);

/// The pending queue, oldest first.
pub async fn pending_updates(pg: &PgPool) -> Result<Vec<UpdateRequestRow>, sqlx::Error> {
    let rows: Vec<UpdateRow> = sqlx::query_as(
        "select r.id::text, r.app_slug, s.repo, r.git_ref, s.sha, r.target_sha, r.prs, \
                coalesce(d.display_name, d.model), b.name, \
                (trunc(extract(epoch from r.created_at) * 1000))::bigint \
         from app_update_requests r \
         join plugins p on p.app_slug = r.app_slug \
         join app_sources s on s.slug = r.app_slug \
         left join agent_defs d on d.id = p.maintainer_agent_id \
         left join boards b on b.id = p.board_id \
         where r.status = 'pending' order by r.created_at",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| UpdateRequestRow {
            id: r.0,
            app_slug: r.1,
            repo: r.2,
            git_ref: r.3,
            from_sha: r.4,
            target_sha: r.5,
            prs: r.6,
            maintainer: r.7,
            board_name: r.8,
            created_at: epoch_ms_to_iso(r.9),
        })
        .collect())
}

/// What the admin decided.
pub enum UpdateDecision {
    Approve,
    Reject,
}

/// What a decision did.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDecided {
    /// applied | failed | rejected
    pub status: &'static str,
    pub line: String,
    pub slug: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub up_to_date: bool,
    pub rolled_back: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug)]
pub enum UpdateDecideError {
    NotFound,
    Db(String),
}

/// Decide update request `id`.
pub async fn decide_update(
    pg: &PgPool,
    creds: &PlatformCredentials<'_>,
    host: &AppHost,
    id: &str,
    decision: UpdateDecision,
    note: Option<&str>,
    actor: &str,
) -> Result<UpdateDecided, UpdateDecideError> {
    let claimed: Option<(String, String)> = sqlx::query_as(
        "update app_update_requests set status = 'updating', updated_at = now() \
         where id = $1::uuid and status = 'pending' returning app_slug, target_sha",
    )
    .bind(id)
    .fetch_optional(pg)
    .await
    .map_err(|e| UpdateDecideError::Db(format!("request claim failed: {e}")))?;
    let Some((slug, target)) = claimed else {
        return Err(UpdateDecideError::NotFound);
    };
    let finish = |status: &'static str, line: String| async move {
        sqlx::query(
            "update app_update_requests set status = $2, decided_by = $3, decision_note = $4, \
                result = $5, updated_at = now() where id = $1::uuid",
        )
        .bind(id)
        .bind(status)
        .bind(actor)
        .bind(note)
        .bind(&line)
        .execute(pg)
        .await
        .map_err(|e| UpdateDecideError::Db(format!("decision write failed: {e}")))?;
        Ok::<String, UpdateDecideError>(line)
    };
    let plugin = crate::plugin(pg, &slug)
        .await
        .map_err(|e| UpdateDecideError::Db(format!("plugin read failed: {e}")))?;
    let Some(plugin) = plugin else {
        // The app was uninstalled under the request: nothing to update.
        let line = format!("plugin \"{slug}\" is no longer installed");
        finish("rejected", line.clone()).await?;
        return Ok(UpdateDecided {
            status: "rejected",
            line,
            slug,
            from: None,
            to: None,
            up_to_date: false,
            rolled_back: false,
            error: None,
        });
    };
    if let UpdateDecision::Reject = decision {
        let line = format!(
            "rejected the update of plugin \"{slug}\" to {}{}",
            short_sha(Some(&target)),
            note.map(|n| format!(" — {n}")).unwrap_or_default()
        );
        let line = finish("rejected", line).await?;
        if let Some(task_id) = plugin.task_id.as_deref() {
            let _ = log_activity(pg, task_id, actor, "workbench", &line).await;
        }
        return Ok(UpdateDecided {
            status: "rejected",
            line,
            slug,
            from: plugin.installed_commit.clone(),
            to: Some(target),
            up_to_date: false,
            rolled_back: false,
            error: None,
        });
    }
    match apply_update(pg, creds, host, &plugin, actor, "approved update").await {
        Ok((outcome, line)) => {
            let status = if outcome.ok { "applied" } else { "failed" };
            let line = finish(status, line).await?;
            Ok(UpdateDecided {
                status,
                line,
                slug,
                from: outcome.from,
                to: outcome.to,
                up_to_date: outcome.up_to_date,
                rolled_back: outcome.rolled_back,
                error: outcome.error,
            })
        }
        // Refused before anything was fetched (the update verb's own
        // refusals): a failed decision, said on the ticket like any other.
        Err(e) => {
            let line = format!("approved update: plugin \"{slug}\" could not be updated: {e}");
            let line = finish("failed", line).await?;
            if let Some(task_id) = plugin.task_id.as_deref() {
                let _ = log_activity(pg, task_id, actor, "workbench", &line).await;
            }
            Ok(UpdateDecided {
                status: "failed",
                line,
                slug,
                from: plugin.installed_commit.clone(),
                to: Some(target),
                up_to_date: false,
                rolled_back: true,
                error: Some(e),
            })
        }
    }
}
