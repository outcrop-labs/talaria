// Automated QA judge — the read side this crate serves: a ticket's review
// rows and the gate's config. The gate itself (runJudgeForTask) runs in the
// judge harness (harness/defs/judge.rs), which writes the judge_reviews rows
// read here.

use crate::agent_auth::epoch_ms_to_iso;
use sqlx::PgPool;

/// One review row: what the quality gate concluded the last time it ran on
/// this ticket, and who ran it.
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

/// A ticket's reviews, newest first.
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

// ── Config ────────────────────────────────────────────────────────────────────
// One row in app_settings decides whether the gate runs, on which model, and
// with what global stance. The admin routes own the writes; every runner
// reads the same row.

const CONFIG_KEY: &str = "judge_config";

/// The judge's config in wire order ({enabled, model, mode}) — defaults
/// merged over the stored partial. A stored value of the wrong type falls
/// to the default per field.
pub async fn get_judge_config(pg: &PgPool) -> serde_json::Value {
    let stored = crate::gateway::settings::get_setting(pg, CONFIG_KEY, serde_json::json!({})).await;
    let mut out = serde_json::Map::new();
    out.insert(
        "enabled".into(),
        serde_json::Value::Bool(
            stored
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        ),
    );
    out.insert(
        "model".into(),
        match stored.get("model") {
            Some(serde_json::Value::String(m)) => serde_json::Value::String(m.clone()),
            _ => serde_json::Value::Null,
        },
    );
    out.insert(
        "mode".into(),
        serde_json::Value::String(
            stored
                .get("mode")
                .and_then(|v| v.as_str())
                .filter(|m| *m == "advisory" || *m == "enforcing")
                .unwrap_or("enforcing")
                .to_string(),
        ),
    );
    serde_json::Value::Object(out)
}

/// Store the whole config — a full-object write.
pub async fn set_judge_config(pg: &PgPool, config: &serde_json::Value) {
    let _ = crate::gateway::settings::set_setting(pg, CONFIG_KEY, config).await;
}
