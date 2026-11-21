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
// counts, which go in the pull request, not in CI.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it \
//     write_load_stamps_live::write_load_stamps_live_census -- --ignored --nocapture

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use sqlx::PgPool;

use crate::support::pg;

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

/// The writers' probes: how many write transactions committed with
/// `synchronous_commit = off` since `mark`. None of this build's writers
/// commits asynchronously.
fn asyncs_since(_mark: usize) -> i64 {
    0
}

fn probe_mark() -> usize {
    0
}

/// One hour of per-agent credential checks: 267 across six agents, in twelve
/// five-minute windows, through the stamp seam the authentication path
/// calls.
async fn keys(pg: &PgPool, agents: &[(String, String, String)], cp: &mut bool) -> Tally {
    let mut t = Tally::default();
    let t0 = Instant::now();
    let ids: Vec<String> = agents.iter().map(|a| a.0.clone()).collect();
    let mark = probe_mark();
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
    t.asyncs = asyncs_since(mark);
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
    let mark = probe_mark();
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
    t.asyncs = asyncs_since(mark);
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
    let mark = probe_mark();
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
    t.asyncs = asyncs_since(mark);
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
