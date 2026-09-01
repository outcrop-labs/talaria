//! Live probe for the silent transcript-archive failure: a Rust fitness sweep
//! landed 338 cases in the report but filed zero `fitness_transcripts` rows,
//! and `record_transcript` swallows per-case errors at debug level. This test
//! makes the error visible. `#[ignore]` like the other live-DB tests — run
//! with DATABASE_URL sourced from ui/.env.

use talaria_api::fitness::evals::{EvalCaseScore, TaskVerdict};
use talaria_api::fitness::transcripts::record_transcript;
use talaria_api::harness::define::EvalBand;

fn score(harness: &str, case: &str, started_at: &str) -> EvalCaseScore {
    EvalCaseScore {
        harness: harness.into(),
        case: case.into(),
        band: EvalBand::Standard,
        skipped: None,
        contract_held: true,
        first_pass: true,
        repairs: 0,
        answered: true,
        task: TaskVerdict::Pass,
        task_error: None,
        gap: None,
        findings: 0,
        latency_ms: 1_000,
        started_at: started_at.into(),
        wall_ms: 2_000,
        prompt_tokens: 10,
        completion_tokens: 20,
        cost_usd: Some(0.001),
        estimated: false,
        timed_out: false,
        optimistic: false,
        error: None,
        prompt: None,
        raw: None,
        turns: None,
        upstream: None,
        calls: None,
    }
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn record_transcript_files_a_row_the_sweep_can_read_back() {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        panic!("set DATABASE_URL (source ui/.env) to run the ignored live tests")
    });
    let pg = sqlx::PgPool::connect(&url).await.expect("connect");

    // Realistic row first: the exact shape a landed case produces.
    record_transcript(
        &pg,
        "live-probe-model",
        "2026-09-01T15:45:52.054Z",
        &score("librarian", "probe-case", "2026-09-01T15:46:00.000Z"),
    )
    .await;

    let rows: i64 = sqlx::query_scalar(
        "select count(*) from fitness_transcripts \
         where model='live-probe-model' and harness='librarian' and case_name='probe-case'",
    )
    .fetch_one(&pg)
    .await
    .expect("count probe rows");
    if rows != 1 {
        // Surface the swallowed error verbatim: same statement, propagated.
        sqlx::query(
            "insert into fitness_transcripts \
             (model, run_started_at, harness, case_name, band, verdict, prompt, raw, turns, \
              tool_calls, upstream, latency_ms, wall_ms, started_at, prompt_tokens, completion_tokens) \
             values ($1, $2::timestamptz, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, \
              $14::timestamptz, $15, $16)",
        )
        .bind("live-probe-model")
        .bind("2026-09-01T15:45:52.054Z")
        .bind("librarian")
        .bind("probe-case-2")
        .bind("standard")
        .bind("pass")
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<serde_json::Value>::None)
        .bind(Option::<serde_json::Value>::None)
        .bind(Option::<serde_json::Value>::None)
        .bind(1_000i64)
        .bind(2_000i64)
        .bind("2026-09-01T15:46:00.000Z")
        .bind(10i64)
        .bind(20i64)
        .execute(&pg)
        .await
        .expect("the insert record_transcript swallows — here is the real error");
    }
    assert_eq!(rows, 1, "record_transcript filed nothing — the silent failure");

    sqlx::query("delete from fitness_transcripts where model='live-probe-model'")
        .execute(&pg)
        .await
        .expect("cleanup probe rows");
}
