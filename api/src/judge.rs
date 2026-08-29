// Automated QA judge — the port of ui/src/server/judge.ts, grown slice by
// slice. The tasks family serves a ticket's review rows (listJudgeReviews),
// which crosses here; the gate itself (runJudgeForTask) runs the judge
// harness, so it crosses with the harness plane at the scheduler handover —
// until then the TS gate keeps writing these rows and Rust keeps reading
// them, which is the coexistence shape every shared table in this port uses.

use crate::agent_auth::epoch_ms_to_iso;
use sqlx::PgPool;

/// One review row (judge.ts JudgeReview): what the quality gate concluded the
/// last time it ran on this ticket, and who ran it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeReview {
    pub id: String,
    pub model: Option<String>,
    pub verdict: String,
    pub summary: String,
    pub issues: Vec<String>,
    pub created_at: String,
}

/// A ticket's reviews, newest first — judge.ts listJudgeReviews.
pub async fn list_judge_reviews(
    pg: &PgPool,
    task_id: &str,
) -> Result<Vec<JudgeReview>, sqlx::Error> {
    // (id, model, verdict, summary, issues, created_ms)
    type ReviewRow = (String, Option<String>, String, String, Vec<String>, i64);
    let rows: Vec<ReviewRow> = sqlx::query_as(
        "select id::text, model, verdict, summary, issues, \
                (trunc(extract(epoch from created_at) * 1000))::bigint \
         from judge_reviews where task_id = $1::uuid order by created_at desc",
    )
    .bind(task_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, model, verdict, summary, issues, created_ms)| JudgeReview {
                id,
                model,
                verdict,
                summary,
                issues,
                created_at: epoch_ms_to_iso(created_ms),
            },
        )
        .collect())
}
