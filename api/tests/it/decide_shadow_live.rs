// SHADOW MODE's round trip, against a real database.
//
// WHY THIS NEEDS A LIVE DATABASE AND THE UNIT TESTS DO NOT. The pure half of
// `shadow` — how an answer renders, what counts as agreement — is settled in
// the crate's own tests with no database anywhere near them. What those cannot
// reach is the half that only exists in Postgres: the insert's eleven binds
// against the column types the migration actually created, and the aggregate's
// `filter`/`percentile_disc` arithmetic. A `try_get::<Option<i32>>` against a
// column the migration made `real`, or an `avg(certainty) filter (where agreed
// = false)` that quietly counts the rows where `agreed` is NULL, are both
// invisible to a unit test and both change the number a person reads before
// switching a site over to a model.
//
// THE ROW SET IS THE POINT. Five agreements, three disagreements, and two
// SILENCES — comparisons where the port answered nothing at all. The silences
// are what the shape is for: they belong in `compared` and not in `answered`,
// and they must not drag the disagreement certainty toward zero by being
// averaged in as `agreed = false`. SQL's NULL comparison is what keeps them
// out, which is exactly the kind of thing worth pinning rather than believing.

use talaria_decide::shadow::{self, Compare, noul_agrees};
use talaria_decide::{Answer, Judgment};

use super::support;

/// A site id per test, and that is not tidiness. Cargo runs the tests in a
/// module on parallel threads, so two tests sharing one site id interleave
/// their writes and each reads the other's rows — which is precisely how the
/// first run of this file failed, reporting 11 comparisons where it wrote 10.
/// CI's module-by-module split keeps FILES off each other's rows; within a
/// file the ids have to do it.
const SITE_LEDGER: &str = "test:decide-shadow-ledger";
const SITE_UNCAL: &str = "test:decide-shadow-uncalibrated";

fn judgment(p: f64, calibrated: bool, latency_ms: u64) -> Judgment {
    Judgment {
        answer: Answer::Noul(p),
        calibrated,
        provider: "test-provider".into(),
        model: Some("test-model".into()),
        latency_ms,
        // Usage as a provider reports it. Carried here so the ledger's
        // cost columns are exercised against a real Postgres rather than
        // only against the unit tests' in-memory shapes.
        tokens_in: Some(120),
        tokens_out: Some(12),
        fanned: 1,
    }
}

fn compare_for(site: &'static str, baseline: &str) -> Compare {
    Compare {
        site,
        subject_ref: Some("TEST-1".into()),
        baseline: baseline.into(),
        agrees: noul_agrees,
    }
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_shadow_ledger_counts_silences_in_compared_but_never_in_answered() {
    let pg = support::pg().await;
    sqlx::query("delete from decide_shadow where site = $1")
        .bind(SITE_LEDGER)
        .execute(&pg)
        .await
        .expect("a clean slate for this site");

    // Five the port got right, by the gate's own reading of agreement.
    // DISTINCT LATENCIES ON PURPOSE: the first draft gave all five the same
    // 110ms, which made the median 110 and the assertion unable to tell a
    // working percentile from a constant. Eight distinct values make the
    // expected p50/p99 readable off the list.
    for (p, baseline, ms) in [
        (0.93, "true", 120),
        (0.88, "true", 98),
        (0.07, "false", 110),
        (0.71, "true", 140),
        (0.12, "false", 101),
    ] {
        shadow::record(
            &pg,
            &compare_for(SITE_LEDGER, baseline),
            Some(&judgment(p, true, ms)),
        )
        .await;
    }
    // Three disagreements: two confident, one right at the coin flip. Their
    // certainties are 0.82, 0.92 and 0.04 — mean 0.5933… — and that mean is
    // the number the panel shows.
    shadow::record(
        &pg,
        &compare_for(SITE_LEDGER, "true"),
        Some(&judgment(0.09, true, 115)),
    )
    .await;
    shadow::record(
        &pg,
        &compare_for(SITE_LEDGER, "true"),
        Some(&judgment(0.04, true, 480)),
    )
    .await;
    shadow::record(
        &pg,
        &compare_for(SITE_LEDGER, "false"),
        Some(&judgment(0.52, true, 130)),
    )
    .await;
    // Two silences: the port answered nothing.
    shadow::record(&pg, &compare_for(SITE_LEDGER, "true"), None).await;
    shadow::record(&pg, &compare_for(SITE_LEDGER, "true"), None).await;

    let report = shadow::shadow_report(&pg).await;
    let site = report
        .iter()
        .find(|s| s.site == SITE_LEDGER)
        .expect("the site this test wrote appears in the report");

    assert_eq!(site.compared, 10, "every attempt, including the silences");
    assert_eq!(site.answered, 8, "only the comparisons the port replied to");
    assert_eq!(site.agreed, 5);
    assert_eq!(site.calibrated, 8);
    // percentile_disc picks a value that IS in the set, so these are real
    // observed latencies rather than interpolations. Sorted, the eight
    // answered calls are 98, 101, 110, 115, 120, 130, 140, 480.
    assert_eq!(site.p50_ms, Some(115));
    assert_eq!(site.p99_ms, Some(480));
    assert_eq!(site.provider.as_deref(), Some("test-provider"));
    assert!(site.newest.is_some(), "the timestamp decodes as text");

    // ── COST AND MODELS, against a real Postgres ────────────────────────────
    //
    // These columns exist because the port's calls never reach the token
    // ledger, so the panel is the only place a site's spend is visible. The
    // unit tests pin the SHAPE; this pins that the numbers survive a round
    // trip through the database, which is where an `integer` column and a
    // `u32` field get to disagree.
    assert_eq!(
        site.judgments, 8,
        "rows carrying one probability — the denominator for the uncalibrated warning,          and NOT `answered + acted`"
    );
    assert_eq!(
        site.metered, 8,
        "the eight answered calls each reported usage; the two silences reported none"
    );
    assert_eq!(
        site.tokens_in,
        8 * 120,
        "every answered judgment carried 120 billable input tokens"
    );
    assert_eq!(
        site.acted, 0,
        "nothing was acted on here — these are all comparisons"
    );
    // One model answered, so there is no confound to warn about. The tally is
    // what makes a silent alias rollover visible at all.
    assert_eq!(site.models.len(), 1, "{:?}", site.models);
    assert_eq!(site.models[0].model, "test-model");
    assert_eq!(site.models[0].judgments, 8);

    // THE ASSERTION THIS FILE EXISTS FOR. (0.82 + 0.92 + 0.04) / 3 = 0.5933…
    // If the two silences were being averaged in as disagreements, this would
    // come out at 0.356 — a provider that looks far more filterable than it is.
    let mean = site
        .mean_certainty_when_disagreed
        .expect("three disagreements have a mean certainty");
    assert!(
        (mean - 0.593_333).abs() < 1e-3,
        "disagreement certainty averaged over the real disagreements only, got {mean}"
    );

    sqlx::query("delete from decide_shadow where site = $1")
        .bind(SITE_LEDGER)
        .execute(&pg)
        .await
        .expect("the test cleans up after itself");
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_uncalibrated_answer_is_recorded_as_an_answer_without_a_confidence() {
    let pg = support::pg().await;
    let site_rows = |pg: sqlx::PgPool| async move {
        sqlx::query_as::<_, (i64, i64, Option<f64>)>(
            "select count(*)::bigint, count(certainty)::bigint, max(certainty)::float8 \
               from decide_shadow where site = $1",
        )
        .bind(SITE_UNCAL)
        .fetch_one(&pg)
        .await
        .expect("the probe query runs")
    };
    sqlx::query("delete from decide_shadow where site = $1")
        .bind(SITE_UNCAL)
        .execute(&pg)
        .await
        .expect("a clean slate");

    // A chat endpoint that dropped `logprobs`: a usable answer with no
    // distribution behind it. `certainty` is still written — it is the
    // ANSWER's own arithmetic — and `calibrated` is what tells a reader not
    // to trust it. Those are different columns on purpose: collapsing them
    // would lose the ability to say "it answered, and the number is not real".
    shadow::record(
        &pg,
        &compare_for(SITE_UNCAL, "false"),
        Some(&judgment(0.0, false, 90)),
    )
    .await;

    let (rows, with_certainty, _) = site_rows(pg.clone()).await;
    assert_eq!(rows, 1);
    assert_eq!(with_certainty, 1, "the answer's own certainty is recorded");

    let report = shadow::shadow_report(&pg).await;
    let site = report
        .iter()
        .find(|s| s.site == SITE_UNCAL)
        .expect("the site");
    assert_eq!(site.answered, 1);
    assert_eq!(
        site.calibrated, 0,
        "and it is counted as uncalibrated, which is what gates the threshold"
    );

    sqlx::query("delete from decide_shadow where site = $1")
        .bind(SITE_UNCAL)
        .execute(&pg)
        .await
        .expect("cleanup");
}
