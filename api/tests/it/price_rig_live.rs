// Live-rig proof of the TALA-85 pricing rework (cargo test -- --ignored).
// The unit tests pin the matcher shapes; this file pins the two facts that
// only the real tables can prove:
//
//   a) a cloud usage row with no provider charge prices from a
//      provider_prices model-level row (variant '', the catalog upsert) —
//      the direct-provider path that went null after the rework;
//   b) stale provider activity does NOT override the ledger month cost, and
//      reports itself as stale.
//
// The rig: the real Postgres only. cost_overview takes a bare pool — no
// redis, no router, no state — so the support module's pg() is all this
// binary needs.
//
// House rule: #[ignore]d so the unit CI job skips it; the api-integration
// workflow runs every it/ module with --ignored against the real rig, so
// this file must hold up there. The assertions are LEDGER-ANCHORED — every
// month-cost claim is a comparison against the same priced-view sum
// cost_overview itself reads, computed from the same SQL — so a sibling
// module's row landing mid-test moves both sides equally and cannot fake a
// result. MONTH_WINDOW below only keeps the two fixtures from colliding,
// not correctness.
//
//   DATABASE_URL=... REDIS_URL=... TALARIA_SECRET_KEY=ci-dummy \
//     cargo test -p talaria-api --test it price_rig_live -- --ignored

use crate::support::pg;
use sqlx::PgPool;
use talaria_api_facades::gateway::usage::cost_overview;

/// The fixture prefix this binary owns — every row it makes is tagged, so a
/// failed previous run cannot shadow the next one.
const PREFIX: &str = "price-rig-live";

/// Held across each test's fixture: both tests write endpoints with
/// different names but sweep the same usage_events tag, and the sweep must
/// not run under the sibling's inserts. Correctness no longer depends on
/// this lock — the assertions are ledger-anchored — it just keeps the two
/// fixtures out of each other's resets.
static MONTH_WINDOW: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Delete this binary's rows. The cascades (provider_prices,
/// provider_spend on llm_endpoints; usage_events is swept by its own tag)
/// take the direction production deletes run.
async fn reset(pg: &PgPool) {
    sqlx::query("delete from usage_events where endpoint like $1")
        .bind(format!("{PREFIX}-%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from llm_endpoints where name like $1")
        .bind(format!("{PREFIX}-%"))
        .execute(pg)
        .await
        .unwrap();
}

/// One fixture: a cloud endpoint (provider openai — a direct provider with
/// no published prices of its own) plus a model-level catalog price for
/// `model` ($3 in / $15 out per mtok, the shape refresh_auto_prices
/// upserts). Returns the endpoint's name.
async fn fixture(pg: &PgPool, tag: &str, model: &str, in_mtok: f64, out_mtok: f64) -> String {
    let name = format!("{PREFIX}-{tag}");
    let (id,): (String,) = sqlx::query_as(
        "insert into llm_endpoints (name, provider, class, models, api_key_env, base_url) \
         values ($1, 'openai', 'cloud', $2::jsonb, null, null) returning id::text",
    )
    .bind(&name)
    .bind(format!(r#"{:?}"#, model))
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into provider_prices \
           (endpoint_id, provider, model, variant, price_in_per_mtok, price_out_per_mtok, \
            source, fetched_at) \
         values ($1::uuid, 'openai', $2, '', $3, $4, 'openrouter.catalog', now())",
    )
    .bind(&id)
    .bind(model)
    .bind(in_mtok)
    .bind(out_mtok)
    .execute(pg)
    .await
    .unwrap();
    name
}

/// One cloud usage row on the fixture endpoint, no provider charge.
async fn cloud_row(pg: &PgPool, endpoint: &str, model: &str, prompt: i32, completion: i32) {
    sqlx::query(
        "insert into usage_events \
           (agent_model, source, endpoint_class, llm_model, endpoint, \
            prompt_tokens, completion_tokens, cost_variant) \
         values ($1, 'price-rig-live', 'cloud', $2, $3, $4, $5, null)",
    )
    .bind(format!("{PREFIX}:agent"))
    .bind(model)
    .bind(endpoint)
    .bind(prompt)
    .bind(completion)
    .execute(pg)
    .await
    .unwrap();
}

/// The priced view cost_overview's month window reads — the exact CTE,
/// lifted once so the tests compare against the same SQL the product uses.
/// The multiplier constants are the gateway's CACHE_WRITE/READ_MULTIPLIER.
const PRICED_CTE: &str = "with priced as (
    select u.*,
      case when u.endpoint_class = 'local' then 0
           when u.provider_cost is not null then u.provider_cost
           when u.endpoint_class = 'cloud'
                and pp.in_tok is not null and pp.out_tok is not null then
             ((u.prompt_tokens + u.cache_write_tokens * 1.25
               + u.cache_read_tokens * 0.1) * pp.in_tok
              + u.completion_tokens * pp.out_tok) / 1e6
           else null
      end as cost,
      case when u.endpoint_class = 'local' then 'local'
           when u.provider_cost is not null then 'provider'
           when u.endpoint_class = 'cloud'
                and pp.in_tok is not null and pp.out_tok is not null then 'derived'
           else null
      end as cost_basis
    from usage_events u
    left join llm_endpoints e on e.name = u.endpoint
    left join lateral (
      select p.price_in_per_mtok as in_tok, p.price_out_per_mtok as out_tok
      from provider_prices p
      where p.endpoint_id = e.id
        and p.model = u.llm_model
        and (
          (coalesce(u.cost_variant, '') <> '' and p.variant = u.cost_variant)
          or (
            coalesce(u.cost_variant, '') = ''
            and p.variant = ''
            and e.provider is distinct from 'openrouter'
          )
        )
      order by p.fetched_at desc
      limit 1
    ) pp on true)";

/// This endpoint's own row in the priced view: (cost, cost_basis). Null
/// when the fixture row is missing — the caller's expect says which.
async fn endpoint_row_cost(pg: &PgPool, endpoint: &str) -> Option<(f64, String)> {
    let sql =
        format!("{PRICED_CTE} select cost::float8, cost_basis from priced where endpoint = $1");
    sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(endpoint)
        .fetch_optional(pg)
        .await
        .unwrap()
}

/// The 30-day ledger sum, computed with the SAME priced CTE cost_overview's
/// month window uses — the number a stale report must never move.
async fn ledger_month_cost(pg: &PgPool) -> f64 {
    let sql = format!(
        "{PRICED_CTE} \
         select coalesce(sum(cost), 0)::float8 as cost \
         from priced where created_at > now() - interval '30 days'"
    );
    let (cost,): (f64,) = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .fetch_one(pg)
        .await
        .unwrap();
    cost
}

#[tokio::test]
#[ignore = "needs a live rig"]
async fn a_cloud_row_without_a_provider_charge_prices_from_the_catalog_row() {
    let pg = pg().await;
    // Keep the sibling's reset out of this fixture (MONTH_WINDOW).
    let _window = MONTH_WINDOW.lock().await;
    reset(&pg).await;

    let name = fixture(&pg, "priced", "test-model-x", 3.0, 15.0).await;
    cloud_row(&pg, &name, "test-model-x", 1000, 500).await;

    let overview = cost_overview(&pg).await.unwrap();
    // The month tile carries the ledger number — the same sum the priced
    // view reports — with our row priced in. (1000 × $3 + 500 × $15)/1e6.
    let ledger = ledger_month_cost(&pg).await;
    let month = overview.totals.month.cost.as_f64().unwrap();
    assert!(
        (month - ledger).abs() < 1e-9,
        "month cost {month} should be the ledger sum {ledger}"
    );
    // Our row is what moved it: priced 0.0105, basis derived.
    let (cost, basis) = endpoint_row_cost(&pg, &name)
        .await
        .expect("the fixture row is in the priced view");
    assert!(
        (cost - 0.0105).abs() < 1e-9,
        "row cost {cost} should be 0.0105"
    );
    assert_eq!(basis, "derived");
}

#[tokio::test]
#[ignore = "needs a live rig"]
async fn stale_provider_activity_leaves_the_ledger_month_cost() {
    let pg = pg().await;
    // Keep the sibling's reset out of this fixture (MONTH_WINDOW).
    let _window = MONTH_WINDOW.lock().await;
    reset(&pg).await;

    let name = fixture(&pg, "stale", "test-model-s", 3.0, 15.0).await;
    cloud_row(&pg, &name, "test-model-s", 1000, 500).await;
    // Our priced row is in the ledger before the stale report lands.
    let (cost, basis) = endpoint_row_cost(&pg, &name)
        .await
        .expect("the fixture row is in the priced view");
    assert!(
        (cost - 0.0105).abs() < 1e-9,
        "row cost {cost} should be 0.0105"
    );
    assert_eq!(basis, "derived");

    // A stale activity report: 13 hours old, past the 12h freshness window —
    // the shape a dead ingest leaves behind. Without the guard it would
    // replace the ledger number forever.
    sqlx::query(
        "insert into provider_spend \
           (endpoint_id, provider, variant, model, window_start, window_end, cost_usd, source, \
            fetched_at) \
         select id, 'openai', '', '', now() - interval '30 minutes', \
                now() - interval '29 minutes', 99.0, 'openrouter.activity', \
                now() - interval '13 hours' \
         from llm_endpoints where name = $1",
    )
    .bind(&name)
    .execute(&pg)
    .await
    .unwrap();

    let overview = cost_overview(&pg).await.unwrap();
    // The regression: a stale report may NEVER make the month tile diverge
    // from the ledger sum — both sides read the same priced view, so a
    // sibling's concurrent write moves them equally and cannot fake this.
    let ledger = ledger_month_cost(&pg).await;
    let month = overview.totals.month.cost.as_f64().unwrap();
    assert!(
        (month - ledger).abs() < 1e-9,
        "stale report moved the month cost off the ledger: month {month}, ledger {ledger}"
    );
    assert!(month < 99.0, "the stale 99.0 must not own the month tile");
    let reported = overview
        .totals
        .provider_reported
        .expect("the activity row makes a providerReported");
    assert!(reported.stale, "a 13h-old report is stale");
    assert!(
        (reported.cost.as_f64().unwrap() - 99.0).abs() < 1e-9,
        "the stale cost is still reported for the label"
    );
}
