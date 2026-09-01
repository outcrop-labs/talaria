// EVERY CASE OF EVERY FITNESS RUN, IN FULL — the audit trail the archived
// report could never be.
//
// WHY THE REPORT IS NOT ENOUGH, and this is the whole argument. `FitnessRecord`
// lives in one `app_settings` row read whole on every page load, so it keeps a
// transcript only for cases that FAILED something, capped at thirty. That is
// exactly right for a drill-down and exactly wrong for verification:
//
//   THE PASSING TRANSCRIPTS ARE THE ONES THAT ANSWER THE HARD QUESTION. "Did
//   this model do the work, or did our fixture accept something weak?" cannot be
//   answered from a failure. Several fixtures rewritten this month were rewritten
//   because a PASSING transcript turned out to show a model being credited for
//   the wrong thing — or a failing one showing a model being punished for obeying
//   an instruction we gave it. Every time, the evidence had to be re-bought by
//   re-running the sweep, because we had thrown it away for being green.
//
//   AND A CAP IS NOT A RETENTION POLICY. Thirty of two hundred and forty-seven
//   is a sample chosen by arrival order.
//
// So the numbers stay in the settings row — that is a summary and belongs there —
// and the EVIDENCE goes in a table: one row per case, written as it lands,
// pruned by run rather than by size. Nothing in the read path of the fitness
// page touches it; it is opened on purpose, per model, when somebody is auditing.

use sqlx::PgPool;

use crate::fitness::evals::EvalCaseScore;

/// One archived case, as it comes back out.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub model: String,
    pub run_started_at: String,
    pub harness: String,
    pub case: String,
    pub band: String,
    /// The same vocabulary the live feed colours by — see the live-feed module.
    pub verdict: String,
    pub prompt: Option<String>,
    pub raw: Option<String>,
    pub turns: Option<serde_json::Value>,
    pub tool_calls: Option<serde_json::Value>,
    pub upstream: Option<serde_json::Value>,
    pub latency_ms: i64,
    /// What the case cost the sweep, including retries.
    pub wall_ms: i64,
    pub started_at: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub created_at: String,
}

/// RUNS KEPT PER MODEL. Not cases: a run is the unit somebody audits, and
/// pruning by row count would leave half of one. Five is enough to compare a
/// model against itself across a month of releases and small enough that the
/// table stays a tool rather than a warehouse.
pub const KEEP_RUNS_PER_MODEL: i64 = 5;

/// Bounded here as well as at the call site, because this is the last thing
/// between a model's 200KB reply and a table row.
const TEXT_CAP: usize = 20_000;

pub fn verdict_of(c: &EvalCaseScore) -> &'static str {
    if c.skipped.is_some() {
        "skip"
    } else if c.timed_out {
        "timeout"
    } else if c.gap.is_some() {
        "gap"
    } else if !c.contract_held || c.error.is_some() {
        "error"
    } else if c.task == crate::fitness::evals::TaskVerdict::Fail {
        "fail"
    } else {
        "pass"
    }
}

fn capped(s: &str) -> Option<String> {
    // Cut by character, never by byte — a byte cut mid-scalar would write a
    // row Postgres rejects as invalid UTF-8 and lose the whole transcript.
    Some(s.chars().take(TEXT_CAP).collect())
}

/// The runner's typed lists land in jsonb columns, which speak `Value`.
fn json_of<T: serde::Serialize>(v: &Option<T>) -> Option<serde_json::Value> {
    v.as_ref().and_then(|x| serde_json::to_value(x).ok())
}

/// File one case. Called as the case lands, so a sweep an admin stops still has
/// every transcript it paid for.
///
/// NEVER THROWS. The audit trail is valuable and the sweep is more valuable: a
/// full disk or a locked table must cost the run its evidence, not its results.
/// The caller does not check a return value for the same reason.
pub async fn record_transcript(pg: &PgPool, model: &str, run_started_at: &str, c: &EvalCaseScore) {
    // AssertSqlSafe: every interpolation is a bound parameter.
    let sql = r#"
      insert into fitness_transcripts
        (model, run_started_at, harness, case_name, band, verdict, prompt, raw, turns, tool_calls, upstream,
         latency_ms, wall_ms, started_at, prompt_tokens, completion_tokens)
      values (
        $1, $2::timestamptz, $3, $4, $5, $6, $7, $8, $9, $10, $11,
        $12, $13, $14::timestamptz, $15, $16
      )
    "#;
    let prompt = c.prompt.as_deref().and_then(capped);
    let raw = c.raw.as_deref().and_then(capped);
    let verdict = verdict_of(c);
    let band = match c.band {
        crate::harness::define::EvalBand::Easy => "easy",
        crate::harness::define::EvalBand::Standard => "standard",
        crate::harness::define::EvalBand::Hard => "hard",
    };
    let turned = sqlx::query(sql)
        .bind(model)
        .bind(run_started_at)
        .bind(&c.harness)
        .bind(&c.case)
        .bind(band)
        .bind(verdict)
        .bind(prompt)
        .bind(raw)
        .bind(json_of(&c.turns))
        .bind(json_of(&c.calls))
        .bind(json_of(&c.upstream))
        .bind(c.latency_ms)
        .bind(c.wall_ms)
        .bind(&c.started_at)
        .bind(c.prompt_tokens)
        .bind(c.completion_tokens)
        .execute(pg)
        .await;
    // Deliberately silent per case: a broken audit table would otherwise print
    // 247 identical lines per sweep. `prune_transcripts` runs at the end of a
    // run and is where a persistent failure surfaces.
    if let Err(e) = turned {
        tracing::debug!(error = %e, harness = %c.harness, "fitness transcript not filed");
    }
}

/// Drop every run for this model beyond the newest `keep`.
///
/// BY RUN, using the run's own start time as its identity — which is what makes
/// "the last five runs" a truthful phrase rather than "the last five thousand
/// rows, whatever that spans".
pub async fn prune_transcripts(pg: &PgPool, model: &str, keep: i64) -> Result<u64, sqlx::Error> {
    // AssertSqlSafe: every interpolation is a bound parameter.
    let sql = r#"
      delete from fitness_transcripts
      where model = $1
        and run_started_at not in (
          select run_started_at from fitness_transcripts
          where model = $1
          group by run_started_at
          order by run_started_at desc
          limit $2
        )
    "#;
    Ok(sqlx::query(sql)
        .bind(model)
        .bind(keep)
        .execute(pg)
        .await?
        .rows_affected())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptRun {
    pub run_started_at: String,
    pub cases: i64,
}

/// The runs this model has archived evidence for, newest first.
pub async fn transcript_runs(pg: &PgPool, model: &str) -> Result<Vec<TranscriptRun>, sqlx::Error> {
    // AssertSqlSafe: every interpolation is a bound parameter.
    let sql = r#"
      select (extract(epoch from run_started_at) * 1000)::bigint as run_started_ms,
             count(*)::bigint as cases
      from fitness_transcripts
      where model = $1
      group by run_started_at
      order by run_started_at desc
    "#;
    #[derive(Debug, sqlx::FromRow)]
    struct Row {
        run_started_ms: i64,
        cases: i64,
    }
    let rows: Vec<Row> = sqlx::query_as(sql).bind(model).fetch_all(pg).await?;
    Ok(rows
        .into_iter()
        .map(|r| TranscriptRun {
            run_started_at: crate::agent_auth::epoch_ms_to_iso(r.run_started_ms),
            cases: r.cases,
        })
        .collect())
}

/// Every case of one run, in the order the harnesses declare them.
/// `run_started_at` None means the newest run on record.
pub async fn read_transcripts(
    pg: &PgPool,
    model: &str,
    run_started_at: Option<&str>,
) -> Result<Vec<Transcript>, sqlx::Error> {
    // AssertSqlSafe: every interpolation is a bound parameter.
    let sql = r#"
      select model,
             (extract(epoch from run_started_at) * 1000)::bigint as run_started_ms,
             harness, case_name, band, verdict,
             prompt, raw, turns, tool_calls, upstream,
             latency_ms::int8, wall_ms::int8,
             (extract(epoch from started_at) * 1000)::bigint as started_ms,
             prompt_tokens::int8, completion_tokens::int8,
             (extract(epoch from created_at) * 1000)::bigint as created_ms
      from fitness_transcripts
      where model = $1
        and run_started_at = coalesce(
          $2::timestamptz,
          (select max(run_started_at) from fitness_transcripts where model = $1)
        )
      order by created_at
    "#;
    #[derive(Debug, sqlx::FromRow)]
    struct Row {
        model: String,
        run_started_ms: i64,
        harness: String,
        case_name: String,
        band: String,
        verdict: String,
        prompt: Option<String>,
        raw: Option<String>,
        turns: Option<serde_json::Value>,
        tool_calls: Option<serde_json::Value>,
        upstream: Option<serde_json::Value>,
        latency_ms: i64,
        wall_ms: i64,
        started_ms: Option<i64>,
        prompt_tokens: i64,
        completion_tokens: i64,
        created_ms: i64,
    }
    let rows: Vec<Row> = sqlx::query_as(sql)
        .bind(model)
        .bind(run_started_at)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Transcript {
            model: r.model,
            run_started_at: crate::agent_auth::epoch_ms_to_iso(r.run_started_ms),
            harness: r.harness,
            case: r.case_name,
            band: r.band,
            verdict: r.verdict,
            prompt: r.prompt,
            raw: r.raw,
            turns: r.turns,
            tool_calls: r.tool_calls,
            upstream: r.upstream,
            latency_ms: r.latency_ms,
            wall_ms: r.wall_ms,
            started_at: r.started_ms.map(crate::agent_auth::epoch_ms_to_iso),
            prompt_tokens: r.prompt_tokens,
            completion_tokens: r.completion_tokens,
            created_at: crate::agent_auth::epoch_ms_to_iso(r.created_ms),
        })
        .collect())
}

/// Delete archived transcripts — one model, or every one. Returns the row count,
/// because "cleared" with no number is a claim an admin cannot check.
pub async fn clear_transcripts(pg: &PgPool, model: Option<&str>) -> Result<u64, sqlx::Error> {
    let n = match model {
        Some(m) => {
            // AssertSqlSafe: the interpolation is a bound parameter.
            sqlx::query("delete from fitness_transcripts where model = $1")
                .bind(m)
                .execute(pg)
                .await?
        }
        None => {
            sqlx::query("delete from fitness_transcripts")
                .execute(pg)
                .await?
        }
    };
    Ok(n.rows_affected())
}

#[cfg(test)]
mod tests {
    // No TS test file exists for transcripts; the two pure pieces — the verdict
    // vocabulary and the text cap — are load-bearing for every row written, so
    // they are pinned here without a database.
    use super::*;
    use crate::fitness::evals::TaskVerdict;
    use crate::harness::define::EvalBand;

    fn case() -> EvalCaseScore {
        EvalCaseScore {
            harness: "h".to_string(),
            case: "one".to_string(),
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
            latency_ms: 1,
            started_at: "2026-08-01T00:00:00.000Z".to_string(),
            wall_ms: 1,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            estimated: false,
            timed_out: false,
            optimistic: false,
            error: None,
            prompt: None,
            raw: None,
            turns: None,
            calls: None,
            upstream: None,
        }
    }

    #[test]
    fn verdict_of_reads_skip_timeout_gap_error_fail_pass_in_that_order() {
        let mut c = case();
        assert_eq!(verdict_of(&c), "pass");

        // Order is precedence: a skip beats everything, even a timeout.
        c.skipped = Some("why".to_string());
        c.timed_out = true;
        assert_eq!(verdict_of(&c), "skip");
        c.skipped = None;
        assert_eq!(verdict_of(&c), "timeout");

        c.timed_out = false;
        c.gap = Some("unanswerable".to_string());
        c.error = Some("boom".to_string());
        assert_eq!(verdict_of(&c), "gap");

        c.gap = None;
        assert_eq!(verdict_of(&c), "error");
        c.error = None;
        c.contract_held = false;
        assert_eq!(verdict_of(&c), "error");
        c.contract_held = true;
        c.task = TaskVerdict::Fail;
        assert_eq!(verdict_of(&c), "fail");
    }

    #[test]
    fn text_cap_cuts_by_character_never_mid_scalar() {
        let long = "é".repeat(TEXT_CAP + 10);
        let capped = capped(&long).expect("always Some");
        assert_eq!(capped.chars().count(), TEXT_CAP);
        // Two bytes per é — a byte cut at 20_000 would have split one.
        assert_eq!(capped.len(), TEXT_CAP * 2);
    }
}
