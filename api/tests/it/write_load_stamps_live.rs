// Live census of the per-call stamps, the resource samples and the job state:
// one simulated hour of a six-agent instance, on synthetic rows, through the
// writers' own functions. What it measures, per workload:
//
//   · write transactions — the `pg_snapshot_xmax(pg_current_snapshot())`
//     delta (reading it assigns no xid);
//   · how many of them committed with `synchronous_commit = off`, from the
//     writers' probes;
//   · WAL bytes — the `pg_current_wal_insert_lsn()` delta.
//
// Each workload runs its own hour: the measured calls in windows, with the
// clock moved between them (the seams' `Instant`, the stamps aged in the
// database, the Hermes fake's clock) and a CHECKPOINT at the start of every
// five simulated minutes, as `checkpoint_timeout`'s default would, so each
// window pays the full-page images a real one does. Nothing between the
// windows is counted.
//
// The counters are cluster-wide: run it alone, on a database nothing else is
// connected to, and read the table it prints. It asserts nothing about the
// counts, which go in the pull request, not in CI. Its approval-SLA passes
// are the job's own, so on a dev database they do what the job's next pass
// would.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it \
//     write_load_stamps_live::write_load_stamps_live_census -- --ignored --nocapture
//
// Below it, the regression guards for the same writers, each on its own rows:
// a stamp lands once a window and asynchronously, a tick is one asynchronous
// transaction, and the job state is written only when it changes. The tests
// in this file take turns (`SERIAL`): the census's sample ticks prune every
// aged sample, and its counters would count the others' writes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use sqlx::PgPool;

use crate::support::pg;

/// One test of this file at a time.
static SERIAL: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const PREFIX: &str = "wls-census";
const AGENTS: usize = 6;
/// A production instance's 6.4k per-agent credential checks a day, ÷ 24.
const KEY_CHECKS: usize = 267;
const LLM_CALLS: usize = 6;
const SAMPLE_TICKS: usize = 60;
const ESCALATION_PASSES: usize = 12;
const DIGEST_PERSISTS: usize = 4;
const HERMES_STEPS: usize = 60;
const MINUTE: Duration = Duration::from_secs(60);

/// What one workload cost over its hour.
#[derive(Default, Clone, Copy)]
struct Tally {
    xacts: i64,
    asyncs: i64,
    wal: i64,
}

impl Tally {
    fn add(&mut self, before: (i64, i64), after: (i64, i64)) {
        self.xacts += after.0 - before.0;
        self.wal += after.1 - before.1;
    }
}

/// (next xid, WAL insert position in bytes). Neither read takes an xid.
async fn counters(pg: &PgPool) -> (i64, i64) {
    sqlx::query_as(
        "select pg_snapshot_xmax(pg_current_snapshot())::text::bigint, \
                pg_wal_lsn_diff(pg_current_wal_insert_lsn(), '0/0')::bigint",
    )
    .fetch_one(pg)
    .await
    .expect("counters")
}

/// A checkpoint, as the default `checkpoint_timeout` takes one every five
/// minutes: the next write to each page logs it whole again. A role that
/// may not checkpoint runs without them, and says so once.
async fn checkpoint(pg: &PgPool, said: &mut bool) {
    if let Err(e) = sqlx::query("checkpoint").execute(pg).await
        && !*said
    {
        println!("(no CHECKPOINT between windows: {e})");
        *said = true;
    }
}

async fn sweep(pg: &PgPool) {
    for sql in [
        "delete from agent_resource_samples where agent_model like 'wls-census-%'",
        "delete from agent_defs where slug like 'wls-census-%'",
        "delete from users where sub like 'wls-census:%'",
    ] {
        sqlx::query(sql).execute(pg).await.expect(sql);
    }
}

/// The census's agents: (agent_defs.id, model, container).
async fn agents(pg: &PgPool) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for i in 0..AGENTS {
        let slug = format!("{PREFIX}-a{i}");
        let model = format!("{slug}-model");
        let id: String = sqlx::query_scalar(
            "insert into agent_defs (slug, department, model, display_name) \
             values ($1, $1, $2, $1) returning id::text",
        )
        .bind(&slug)
        .bind(&model)
        .fetch_one(pg)
        .await
        .unwrap();
        sqlx::query(
            "insert into agent_keys (agent_id, key_hash, key_enc) values ($1::uuid, $2, 'census')",
        )
        .bind(&id)
        .bind(format!("{PREFIX}-{}", uuid::Uuid::new_v4()))
        .execute(pg)
        .await
        .unwrap();
        out.push((id, model, format!("{slug}-container")));
    }
    out
}

/// Write transactions the stamp probe saw for `ids` in `what`'s table that
/// committed with `synchronous_commit` off: each stamp that wrote a row. (A
/// stamp that matched none took no transaction id and wrote nothing.)
fn stamp_asyncs(what: &str, ids: &[String]) -> i64 {
    talaria_agent_auth::stamp_probe::seen()
        .iter()
        .filter(|s| {
            s.what == what && ids.contains(&s.id) && s.rows > 0 && s.synchronous_commit == "off"
        })
        .count() as i64
}

/// The same for the sample ticks recorded after the first `from`.
fn tick_asyncs(from: usize) -> i64 {
    talaria_fleet_resources::tick_probe::seen()[from..]
        .iter()
        .filter(|s| s.written + s.pruned > 0 && s.synchronous_commit == "off")
        .count() as i64
}

/// One hour of per-agent credential checks: 267 across six agents, in twelve
/// five-minute windows, through the stamp seam the authentication path
/// calls.
async fn keys(pg: &PgPool, agents: &[(String, String, String)], cp: &mut bool) -> Tally {
    let mut t = Tally::default();
    let t0 = Instant::now();
    let ids: Vec<String> = agents.iter().map(|a| a.0.clone()).collect();
    let mut k = 0;
    for w in 0..12 {
        checkpoint(pg, cp).await;
        let n = KEY_CHECKS * (w + 1) / 12 - KEY_CHECKS * w / 12;
        let before = counters(pg).await;
        let mut handles = Vec::new();
        for i in 0..n {
            let id = &ids[k % AGENTS];
            k += 1;
            let at = t0 + 5 * MINUTE * w as u32 + 5 * MINUTE * i as u32 / n as u32;
            if let Some(h) = talaria_agent_auth::note_key_used_at(pg, id, at) {
                handles.push(h);
            }
        }
        for h in handles {
            h.await.unwrap();
        }
        t.add(before, counters(pg).await);
        // Five minutes pass for the database too.
        sqlx::query(
            "update agent_keys set last_used_at = last_used_at - interval '5 minutes' \
             where agent_id = any($1::uuid[])",
        )
        .bind(&ids)
        .execute(pg)
        .await
        .unwrap();
    }
    t.asyncs = stamp_asyncs("agent_keys", &ids);
    t
}

/// Six calls on one LLM key, ten minutes apart, through the stamp seam both
/// routes call.
async fn llm_key(pg: &PgPool, cp: &mut bool) -> Tally {
    let mut t = Tally::default();
    let user = crate::support::fabricate_user(pg, PREFIX, "llm").await;
    let key: String = sqlx::query_scalar(
        "insert into llm_api_keys (user_id, name, key_hash, prefix) \
         values ($1::uuid, 'wls-census', $2, 'tlk_cens') returning id::text",
    )
    .bind(&user)
    .bind(format!("{PREFIX}-{}", uuid::Uuid::new_v4()))
    .fetch_one(pg)
    .await
    .unwrap();
    let t0 = Instant::now();
    for c in 0..LLM_CALLS {
        checkpoint(pg, cp).await;
        let before = counters(pg).await;
        let at = t0 + 10 * MINUTE * c as u32;
        if let Some(h) =
            talaria_routes_fleet::llm::llm_models::note_key_used_at(pg, &key, "[census]", at)
        {
            h.await.unwrap();
        }
        t.add(before, counters(pg).await);
        sqlx::query(
            "update llm_api_keys set last_used_at = last_used_at - interval '10 minutes' \
             where id = $1::uuid",
        )
        .bind(&key)
        .execute(pg)
        .await
        .unwrap();
    }
    t.asyncs = stamp_asyncs("llm_api_keys", &[key]);
    t
}

/// `docker stats --format '{{json .}}'` for the census's containers.
fn stats_json(agents: &[(String, String, String)], tick: usize) -> String {
    agents
        .iter()
        .enumerate()
        .map(|(i, (_, _, container))| {
            serde_json::json!({
                "Name": container,
                "CPUPerc": format!("{}.{:02}%", (tick + i) % 40, (tick * 7 + i) % 100),
                "MemUsage": format!("{}MiB / 4GiB", 600 + ((tick * 13 + i * 31) % 200)),
                "PIDs": format!("{}", 40 + (tick + i) % 9),
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sixty one-minute ticks of six agents through the sampler's own write,
/// fed synthetic `docker stats` output. Before each, a minute's worth of
/// samples ages past the week the prune keeps — the steady state, where every
/// tick's prune has rows to remove.
async fn samples(pg: &PgPool, agents: &[(String, String, String)], cp: &mut bool) -> Tally {
    let mut t = Tally::default();
    let by_container: HashMap<String, String> = agents
        .iter()
        .map(|(_, model, container)| (container.clone(), model.clone()))
        .collect();
    let models: Vec<String> = agents.iter().map(|a| a.1.clone()).collect();
    let containers: Vec<String> = agents.iter().map(|a| a.2.clone()).collect();
    let from = talaria_fleet_resources::tick_probe::seen().len();
    for tick in 0..SAMPLE_TICKS {
        if tick % 5 == 0 {
            checkpoint(pg, cp).await;
        }
        sqlx::query(
            "insert into agent_resource_samples \
               (agent_model, container, cpu_percent, mem_bytes, pids, taken_at) \
             select m, c, 1.0, 629145600, 40, now() - interval '7 days 1 minute' \
             from unnest($1::text[], $2::text[]) as u(m, c)",
        )
        .bind(&models)
        .bind(&containers)
        .execute(pg)
        .await
        .unwrap();
        let before = counters(pg).await;
        talaria_fleet_resources::record_tick(pg, &stats_json(agents, tick), &by_container).await;
        t.add(before, counters(pg).await);
    }
    t.asyncs = tick_asyncs(from);
    t
}

/// Twelve approval-SLA passes (the announce sweep, then the stages), five
/// minutes apart, with nothing pending that changes between them. A warm-up
/// pass first, uncounted, settles whatever an earlier run left.
async fn escalation(pg: &PgPool, cp: &mut bool) -> Tally {
    let mut t = Tally::default();
    // The announce map exists on any instance that ever announced.
    sqlx::query(
        "insert into app_settings (key, value) \
         values ('approval_announce_state', '{\"announced\": {}}'::jsonb) \
         on conflict (key) do nothing",
    )
    .execute(pg)
    .await
    .unwrap();
    let state = crate::support::app_state().await;
    let deps = talaria_digest::real_digest_deps(
        &state,
        talaria_api::realtime::RealtimeDeps::publish_only(None),
    );
    let now = talaria_agent_auth::now_ms();
    talaria_digest::run_approval_escalation(&deps, now)
        .await
        .unwrap();
    for p in 0..ESCALATION_PASSES {
        checkpoint(pg, cp).await;
        let before = counters(pg).await;
        talaria_digest::run_approval_escalation(&deps, now + (p as i64 + 1) * 5 * 60_000)
            .await
            .unwrap();
        t.add(before, counters(pg).await);
    }
    t
}

/// Four digest passes' closing write, fifteen minutes apart, with the ledger
/// unchanged. The stored ledger is put back afterwards.
async fn digest(pg: &PgPool, cp: &mut bool) -> Tally {
    let mut t = Tally::default();
    let saved = talaria_settings::try_get_setting(pg, "digest_state")
        .await
        .unwrap();
    let mut sent_on: BTreeMap<String, String> = saved
        .as_ref()
        .and_then(|v| v.get("sentOn"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    sent_on.insert(uuid::Uuid::new_v4().to_string(), "2026-11-21".into());
    let live: HashSet<String> = sent_on.keys().cloned().collect();
    talaria_digest::persist_digest_state(pg, &live, &sent_on)
        .await
        .unwrap();
    for _ in 0..DIGEST_PERSISTS {
        checkpoint(pg, cp).await;
        let before = counters(pg).await;
        talaria_digest::persist_digest_state(pg, &live, &sent_on)
            .await
            .unwrap();
        t.add(before, counters(pg).await);
    }
    match saved {
        Some(v) => talaria_settings::set_setting(pg, "digest_state", &v)
            .await
            .unwrap(),
        None => {
            sqlx::query("delete from app_settings where key = 'digest_state'")
                .execute(pg)
                .await
                .unwrap();
        }
    }
    t
}

/// Sixty one-minute steps of the Hermes channel with an Upgrade waiting out
/// its 48 hours on stable — the channel's quiet state for two days after
/// every release — on hermes_channel_live's fixture and its own state row.
async fn hermes(pg: &PgPool, cp: &mut bool) -> Tally {
    use crate::hermes_channel_live::{MIN, World, tuning};
    let mut t = Tally::default();
    let w = World::new("census", 1).await;
    let mut tune = tuning();
    tune.min_stable_ms = 48 * 60 * MIN;
    // The first step opens the rollout; the census starts at its quiet.
    w.step(&tune).await;
    for s in 0..HERMES_STEPS {
        if s % 5 == 0 {
            checkpoint(pg, cp).await;
        }
        w.fake.advance(MIN);
        let before = counters(pg).await;
        w.step(&tune).await;
        t.add(before, counters(pg).await);
    }
    w.done().await;
    t
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL); a measurement, run alone"]
async fn write_load_stamps_live_census() {
    let _turn = SERIAL.lock().await;
    talaria_agent_auth::stamp_probe::start();
    talaria_fleet_resources::tick_probe::start();
    let pg = pg().await;
    sweep(&pg).await;
    let agents = agents(&pg).await;
    let mut cp = false;
    let rows = [
        (
            format!("agent keys ({KEY_CHECKS} checks)"),
            keys(&pg, &agents, &mut cp).await,
        ),
        (
            format!("llm key ({LLM_CALLS} calls)"),
            llm_key(&pg, &mut cp).await,
        ),
        (
            format!("samples ({SAMPLE_TICKS} ticks x {AGENTS})"),
            samples(&pg, &agents, &mut cp).await,
        ),
        (
            format!("escalation ({ESCALATION_PASSES} passes)"),
            escalation(&pg, &mut cp).await,
        ),
        (
            format!("digest ({DIGEST_PERSISTS} persists)"),
            digest(&pg, &mut cp).await,
        ),
        (
            format!("hermes ({HERMES_STEPS} steps)"),
            hermes(&pg, &mut cp).await,
        ),
    ];
    sweep(&pg).await;

    let sync: String = sqlx::query_scalar("show synchronous_commit")
        .fetch_one(&pg)
        .await
        .unwrap();
    println!(
        "\nwrite_load_stamps_live census: one simulated hour, {AGENTS} agents (synchronous_commit = {sync})"
    );
    println!(
        "{:<28} {:>11} {:>7} {:>7} {:>11}",
        "workload", "write xacts", "sync", "async", "WAL bytes"
    );
    let mut total = Tally::default();
    for (name, t) in &rows {
        println!(
            "{:<28} {:>11} {:>7} {:>7} {:>11}",
            name,
            t.xacts,
            t.xacts - t.asyncs,
            t.asyncs,
            t.wal
        );
        total.xacts += t.xacts;
        total.asyncs += t.asyncs;
        total.wal += t.wal;
    }
    println!(
        "{:<28} {:>11} {:>7} {:>7} {:>11}",
        "total",
        total.xacts,
        total.xacts - total.asyncs,
        total.asyncs,
        total.wal
    );
}

// ── Regression guards ────────────────────────────────────────────────────────

/// (xmin, xmax) of one row, as text.
async fn row_version(pg: &PgPool, sql: &'static str, id: &str) -> (String, String) {
    sqlx::query_as(sql)
        .bind(id)
        .fetch_one(pg)
        .await
        .expect("the row")
}

const AGENT_KEY_ROW: &str =
    "select xmin::text, xmax::text from agent_keys where agent_id = $1::uuid";
const LLM_KEY_ROW: &str = "select xmin::text, xmax::text from llm_api_keys where id = $1::uuid";
const SETTING_ROW: &str = "select xmin::text, xmax::text from app_settings where key = $1";

/// The stamp probe's records for `id`, once the first has landed (the stamps
/// are detached; up to ten seconds) and a moment more for any straggler.
async fn stamps_for(id: &str) -> Vec<talaria_agent_auth::stamp_probe::Seen> {
    let mine = || {
        talaria_agent_auth::stamp_probe::seen()
            .into_iter()
            .filter(|s| s.id == id)
            .collect::<Vec<_>>()
    };
    for _ in 0..100 {
        if !mine().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    mine()
}

/// An agent of this file's own, with a credential: (agent id, secret).
async fn keyed_agent(pg: &PgPool, name: &str, last_used: Option<&str>) -> (String, String) {
    let slug = format!("wls-auth-{name}");
    let id: String = sqlx::query_scalar(
        "insert into agent_defs (slug, department, model, display_name, managed) \
         values ($1, $1, $1 || '-model', $1, true) returning id::text",
    )
    .bind(&slug)
    .fetch_one(pg)
    .await
    .unwrap();
    let secret = format!("tak_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "insert into agent_keys (agent_id, key_hash, key_enc, last_used_at) \
         values ($1::uuid, $2, 'test', now() - $3::interval)",
    )
    .bind(&id)
    .bind(talaria_auth::sha256_hex(&secret))
    .bind(last_used)
    .execute(pg)
    .await
    .unwrap();
    (id, secret)
}

fn agent_headers(secret: &str) -> axum::http::HeaderMap {
    let mut h = axum::http::HeaderMap::new();
    h.insert("x-api-key", secret.parse().unwrap());
    h
}

async fn sweep_auth(pg: &PgPool) {
    sqlx::query("delete from agent_defs where slug like 'wls-auth-%'")
        .execute(pg)
        .await
        .unwrap();
}

/// A hundred authentications of one agent inside the window, at once, write
/// its agent_keys row once: one stamp, asynchronously committed, and none
/// after it.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_hundred_agent_authentications_in_the_window_stamp_once() {
    let _turn = SERIAL.lock().await;
    talaria_agent_auth::stamp_probe::start();
    let pg = pg().await;
    sweep_auth(&pg).await;
    let (id, secret) = keyed_agent(&pg, "hundred", None).await;
    let before = row_version(&pg, AGENT_KEY_ROW, &id).await;
    let headers = agent_headers(&secret);
    let calls = (0..100).map(|_| talaria_agent_auth::agent_caller(&pg, &headers));
    for caller in futures_util::future::join_all(calls).await {
        let caller = caller.expect("authenticated").expect("an agent");
        assert_eq!(caller.id.as_deref(), Some(id.as_str()));
    }
    let stamps = stamps_for(&id).await;
    assert_eq!(stamps.len(), 1, "one stamp for a hundred calls: {stamps:?}");
    assert_eq!(stamps[0].rows, 1);
    assert_eq!(
        stamps[0].synchronous_commit, "off",
        "committed asynchronously"
    );
    let stamped = row_version(&pg, AGENT_KEY_ROW, &id).await;
    assert_ne!(stamped.0, before.0, "the row was written");
    for _ in 0..100 {
        talaria_agent_auth::agent_caller(&pg, &headers)
            .await
            .expect("authenticated");
    }
    assert_eq!(
        stamps_for(&id).await.len(),
        1,
        "and never again inside the window"
    );
    assert_eq!(row_version(&pg, AGENT_KEY_ROW, &id).await, stamped);
    let migrated: bool = sqlx::query_scalar(
        "select last_used_at is not null from agent_keys where agent_id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(migrated);
    sweep_auth(&pg).await;
}

/// A stamp older than the window is refreshed on the next call, and a stamp
/// inside it is not rewritten by another process (the UPDATE's own guard):
/// it matches no row and leaves the row as it was.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_stamp_past_the_window_is_refreshed_and_one_inside_it_is_not() {
    let _turn = SERIAL.lock().await;
    talaria_agent_auth::stamp_probe::start();
    let pg = pg().await;
    sweep_auth(&pg).await;
    let (id, secret) = keyed_agent(&pg, "stale", Some("6 minutes")).await;
    talaria_agent_auth::agent_caller(&pg, &agent_headers(&secret))
        .await
        .expect("authenticated")
        .expect("an agent");
    let stamps = stamps_for(&id).await;
    assert_eq!(stamps.len(), 1, "{stamps:?}");
    assert_eq!(stamps[0].rows, 1, "a six-minute-old stamp is refreshed");
    let fresh: bool = sqlx::query_scalar(
        "select last_used_at > now() - interval '1 minute' from agent_keys \
         where agent_id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(fresh);
    let stamped = row_version(&pg, AGENT_KEY_ROW, &id).await;
    assert_eq!(
        talaria_agent_auth::stamp_agent_key(&pg, &id).await.unwrap(),
        0,
        "a second process's stamp inside the window matches nothing"
    );
    assert_eq!(row_version(&pg, AGENT_KEY_ROW, &id).await, stamped);
    sweep_auth(&pg).await;
}

/// A key never used is stamped on its first call, so the legacy migration
/// status counts the agent as moved over at once, exactly as before.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_never_used_agent_key_is_stamped_on_its_first_call() {
    let _turn = SERIAL.lock().await;
    talaria_agent_auth::stamp_probe::start();
    let pg = pg().await;
    sweep_auth(&pg).await;
    let (id, secret) = keyed_agent(&pg, "first", None).await;
    let pending = |s: &talaria_agent_auth::LegacyMigrationStatus| {
        s.pending.iter().any(|m| m == "wls-auth-first-model")
    };
    let status = talaria_agent_auth::legacy_migration_status(&pg)
        .await
        .unwrap();
    assert!(pending(&status), "never used: still pending");
    talaria_agent_auth::agent_caller(&pg, &agent_headers(&secret))
        .await
        .expect("authenticated")
        .expect("an agent");
    let stamps = stamps_for(&id).await;
    assert_eq!(stamps.len(), 1, "{stamps:?}");
    assert_eq!(stamps[0].rows, 1);
    let status = talaria_agent_auth::legacy_migration_status(&pg)
        .await
        .unwrap();
    assert!(!pending(&status), "migrated on its first call");
    sweep_auth(&pg).await;
}

/// An LLM key of this test's own, on a user of its own: (key id, secret).
async fn llm_key_of(pg: &PgPool, tag: &str, last_used: Option<&str>) -> (String, String) {
    crate::support::sweep_user_rows(pg, "wls-llm", tag).await;
    let user = crate::support::fabricate_user(pg, "wls-llm", tag).await;
    let secret = format!("tlk_{}", uuid::Uuid::new_v4().simple());
    let id: String = sqlx::query_scalar(
        "insert into llm_api_keys (user_id, name, key_hash, prefix, last_used_at) \
         values ($1::uuid, 'wls-llm', $2, 'tlk_wls', now() - $3::interval) returning id::text",
    )
    .bind(&user)
    .bind(talaria_auth::sha256_hex(&secret))
    .bind(last_used)
    .fetch_one(pg)
    .await
    .unwrap();
    (id, secret)
}

/// GET /api/llm/v1/models with the key, through the real router.
async fn models_call(app: &axum::Router, secret: &str) -> u16 {
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/llm/v1/models")
        .header("authorization", format!("Bearer {secret}"))
        .body(axum::body::Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status().as_u16()
}

/// The same three for an LLM key, through the models route: a hundred calls
/// in the window stamp once and asynchronously, a stamp past the window is
/// refreshed while one inside it is not rewritten, and a key never used is
/// stamped on its first call.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_llm_key_is_stamped_once_a_window_and_asynchronously() {
    let _turn = SERIAL.lock().await;
    talaria_agent_auth::stamp_probe::start();
    let pg = pg().await;
    let app = talaria_api::routes::router(crate::support::app_state().await);

    // A hundred calls, the first ever among them.
    let (id, secret) = llm_key_of(&pg, "hundred", None).await;
    let before = row_version(&pg, LLM_KEY_ROW, &id).await;
    for _ in 0..100 {
        assert_eq!(models_call(&app, &secret).await, 200);
    }
    let stamps = stamps_for(&id).await;
    assert_eq!(stamps.len(), 1, "one stamp for a hundred calls: {stamps:?}");
    assert_eq!((stamps[0].what, stamps[0].rows), ("llm_api_keys", 1));
    assert_eq!(stamps[0].synchronous_commit, "off");
    let stamped = row_version(&pg, LLM_KEY_ROW, &id).await;
    assert_ne!(stamped.0, before.0, "the first call stamped the key");
    assert_eq!(
        talaria_routes_fleet::llm::llm_models::stamp_key(&pg, &id)
            .await
            .unwrap(),
        0,
        "a second process's stamp inside the window matches nothing"
    );
    assert_eq!(row_version(&pg, LLM_KEY_ROW, &id).await, stamped);

    // Past the window.
    let (stale, stale_secret) = llm_key_of(&pg, "stale", Some("6 minutes")).await;
    assert_eq!(models_call(&app, &stale_secret).await, 200);
    let stamps = stamps_for(&stale).await;
    assert_eq!(stamps.len(), 1, "{stamps:?}");
    assert_eq!(stamps[0].rows, 1, "a six-minute-old stamp is refreshed");
    let fresh: bool = sqlx::query_scalar(
        "select last_used_at > now() - interval '1 minute' from llm_api_keys where id = $1::uuid",
    )
    .bind(&stale)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(fresh);

    for tag in ["hundred", "stale"] {
        crate::support::sweep_user_rows(&pg, "wls-llm", tag).await;
    }
}

/// One tick of six agents is one transaction: six rows sharing one xmin,
/// `synchronous_commit` off inside it and on again after, and the aged
/// samples pruned in that same transaction.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_sample_tick_is_one_asynchronous_transaction_with_its_prune() {
    use std::str::FromStr;
    let _turn = SERIAL.lock().await;
    talaria_fleet_resources::tick_probe::start();
    let url = std::env::var("DATABASE_URL").expect(crate::support::DATABASE_URL_EXPECT);
    // One connection: the read after the tick is the session the tick ran on.
    let pg = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(sqlx::postgres::PgConnectOptions::from_str(&url).unwrap())
        .await
        .unwrap();
    sqlx::query("delete from agent_resource_samples where agent_model like 'wls-samples-%'")
        .execute(&pg)
        .await
        .unwrap();
    let agents: Vec<(String, String, String)> = (0..AGENTS)
        .map(|i| {
            (
                String::new(),
                format!("wls-samples-a{i}-model"),
                format!("wls-samples-a{i}-container"),
            )
        })
        .collect();
    let by_container: HashMap<String, String> = agents
        .iter()
        .map(|(_, m, c)| (c.clone(), m.clone()))
        .collect();
    // Two aged samples an agent: past the week the prune keeps.
    sqlx::query(
        "insert into agent_resource_samples (agent_model, container, taken_at) \
         select m, m, now() - interval '8 days' \
         from unnest($1::text[]) as u(m), generate_series(1, 2)",
    )
    .bind(agents.iter().map(|a| a.1.clone()).collect::<Vec<_>>())
    .execute(&pg)
    .await
    .unwrap();
    let from = talaria_fleet_resources::tick_probe::seen().len();
    talaria_fleet_resources::record_tick(&pg, &stats_json(&agents, 0), &by_container).await;
    let ticks = talaria_fleet_resources::tick_probe::seen()[from..].to_vec();
    assert_eq!(ticks.len(), 1, "{ticks:?}");
    assert_eq!(
        ticks[0].synchronous_commit, "off",
        "committed asynchronously"
    );
    assert_eq!(ticks[0].written, AGENTS as u64);
    assert!(
        ticks[0].pruned >= 2 * AGENTS as u64,
        "the aged samples went in the tick's own transaction: {ticks:?}"
    );
    let (rows, xmins, aged): (i64, i64, i64) = sqlx::query_as(
        "select count(*) filter (where taken_at > now() - interval '1 day'), \
                count(distinct xmin::text) filter (where taken_at > now() - interval '1 day'), \
                count(*) filter (where taken_at < now() - interval '7 days') \
         from agent_resource_samples where agent_model like 'wls-samples-%'",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!((rows, xmins, aged), (AGENTS as i64, 1, 0));
    let sync: String = sqlx::query_scalar("show synchronous_commit")
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(
        sync, "on",
        "synchronous_commit = off stayed inside the tick"
    );
    sqlx::query("delete from agent_resource_samples where agent_model like 'wls-samples-%'")
        .execute(&pg)
        .await
        .unwrap();
}

/// A job-state save of an equal value — the same jsonb in another key
/// order — is not written: no new row version, no lock. A different value
/// is; and `set_setting`, an admin's save, still writes every time.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_setting_saved_unchanged_is_not_written() {
    use serde_json::json;
    use talaria_settings::{set_setting, set_setting_if_changed};
    let _turn = SERIAL.lock().await;
    let pg = pg().await;
    let key = format!("wls-settings-test:{}", uuid::Uuid::new_v4());
    assert!(
        set_setting_if_changed(&pg, &key, &json!({"a": {"x": 1, "y": [1, 2]}, "b": "two"}))
            .await
            .unwrap(),
        "an absent key is written"
    );
    let first = row_version(&pg, SETTING_ROW, &key).await;
    assert!(
        !set_setting_if_changed(&pg, &key, &json!({"b": "two", "a": {"y": [1, 2], "x": 1}}))
            .await
            .unwrap(),
        "the same jsonb in another key order is not a change"
    );
    assert_eq!(row_version(&pg, SETTING_ROW, &key).await, first);
    assert!(
        set_setting_if_changed(&pg, &key, &json!({"a": {"x": 1, "y": [2, 1]}, "b": "two"}))
            .await
            .unwrap(),
        "a different value is written"
    );
    let changed = row_version(&pg, SETTING_ROW, &key).await;
    assert_ne!(changed.0, first.0);
    let stored = talaria_settings::try_get_setting(&pg, &key).await.unwrap();
    assert_eq!(
        stored,
        Some(json!({"a": {"x": 1, "y": [2, 1]}, "b": "two"}))
    );
    set_setting(&pg, &key, &json!({"a": {"x": 1, "y": [2, 1]}, "b": "two"}))
        .await
        .unwrap();
    assert_ne!(
        row_version(&pg, SETTING_ROW, &key).await.0,
        changed.0,
        "an admin's save always writes"
    );
    sqlx::query("delete from app_settings where key = $1")
        .bind(&key)
        .execute(&pg)
        .await
        .unwrap();
}

/// A prune of the announce marks that drops nothing leaves the row as it
/// was; one that drops a decided approval's mark writes. Every mark already
/// there is passed as live, so a dev database's marks are kept.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_announce_prune_that_drops_nothing_is_not_written() {
    let _turn = SERIAL.lock().await;
    let pg = pg().await;
    let key = "approval_announce_state";
    sqlx::query(
        "insert into app_settings (key, value) values ($1, '{\"announced\": {}}'::jsonb) \
         on conflict (key) do nothing",
    )
    .bind(key)
    .execute(&pg)
    .await
    .unwrap();
    // A decided approval's mark, from long ago.
    sqlx::query(
        "update app_settings set value = jsonb_build_object('announced', \
           coalesce(value -> 'announced', '{}'::jsonb) \
             || '{\"wls-decided\": \"2001-01-01T00:00:00.000Z\"}'::jsonb) \
         where key = $1",
    )
    .bind(key)
    .execute(&pg)
    .await
    .unwrap();
    let marks: Vec<String> = sqlx::query_scalar(
        "select jsonb_object_keys(value -> 'announced') from app_settings where key = $1",
    )
    .bind(key)
    .fetch_all(&pg)
    .await
    .unwrap();
    let now = talaria_agent_auth::now_iso();
    let before = row_version(&pg, SETTING_ROW, key).await;
    talaria_approvals::prune_announced(&pg, &marks, &now)
        .await
        .unwrap();
    assert_eq!(
        row_version(&pg, SETTING_ROW, key).await,
        before,
        "nothing dropped, nothing written, no lock"
    );
    let live: Vec<String> = marks.into_iter().filter(|m| m != "wls-decided").collect();
    talaria_approvals::prune_announced(&pg, &live, &now)
        .await
        .unwrap();
    assert_ne!(row_version(&pg, SETTING_ROW, key).await.0, before.0);
    let left: bool = sqlx::query_scalar(
        "select value -> 'announced' ? 'wls-decided' from app_settings where key = $1",
    )
    .bind(key)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(!left, "the decided approval's mark was dropped");
}
