// Live-DB proof of the Hermes stable channel's database half
// (cargo test -- --ignored): the per-digest records (`hermes_images`), the
// per-agent assignments (`hermes_agent_images`) and the in-use probe the
// rollout passes busy agents over with. The planning itself is a pure
// function with unit tests (talaria-hermes-images `plan_from`); what only
// Postgres can vouch for is the SQL — the casts, the upserts' answers, the
// cascade, and that each busy signal is read from the column it names.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it hermes_channel_live:: -- --ignored
//
// It never touches the channel's own settings row (`hermes_channel`), so it
// is safe beside a running stack.

use crate::support::pg;
use sqlx::postgres::PgPool;
use talaria_hermes_channel::ChannelAgent;
use talaria_hermes_channel::busy::{db_signals, why_busy};
use talaria_hermes_images as images;

const PREFIX: &str = "hermes-channel-test";

fn digest(tag: &str, n: u8) -> String {
    // 64 lowercase hex: the tag's bytes, padded — a digest no registry names.
    let mut hex: String = format!("{tag}{n}")
        .bytes()
        .map(|b| format!("{:02x}", b))
        .collect();
    hex.truncate(56);
    format!("sha256:{hex:0<64}")
}

async fn cleanup(pg: &PgPool, tag: &str) {
    for n in 0..4 {
        sqlx::query("delete from hermes_images where digest = $1")
            .bind(digest(tag, n))
            .execute(pg)
            .await
            .unwrap();
    }
    // Rows keyed by the agent's model or slug, which no foreign key removes.
    for sql in [
        "delete from usage_events where agent_model like $1",
        "delete from runs where kind = 'work-session' and input->>'agentModel' like $1",
        "delete from llm_api_keys where name like 'agent:' || $1",
    ] {
        sqlx::query(sql)
            .bind(format!("{PREFIX}-{tag}-%"))
            .execute(pg)
            .await
            .unwrap();
    }
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

async fn agent(pg: &PgPool, tag: &str, name: &str) -> (String, String) {
    let slug = format!("{PREFIX}-{tag}-{name}");
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
    (id, model)
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_digest_is_seen_once_prepared_once_and_bad_for_good() {
    let pg = pg().await;
    let tag = "rec";
    cleanup(&pg, tag).await;
    let d = digest(tag, 0);
    assert!(images::is_digest(&d), "{d}");

    // First sighting is new; the next is not, and keeps the version a
    // sighting without one does not know.
    assert!(
        images::note_seen(&pg, &d, "nousresearch/hermes-agent", Some("v0.22.6"))
            .await
            .unwrap()
    );
    assert!(
        !images::note_seen(&pg, &d, "nousresearch/hermes-agent", None)
            .await
            .unwrap()
    );
    // An untrusted tag is never kept.
    assert!(
        !images::note_seen(&pg, &d, "nousresearch/hermes-agent", Some("stable"))
            .await
            .unwrap()
    );
    let r = images::record(&pg, &d).await.unwrap().expect("recorded");
    assert_eq!(r.version.as_deref(), Some("v0.22.6"));
    assert_eq!(r.status, images::Status::Candidate);
    assert!(!r.runnable(), "seen is not prepared");

    // Prepared: the image's own config version, its stripped names, pulled
    // here; the image's self-reported version does not replace the tag's.
    let inspect =
        serde_json::json!({ "strippedEnv": ["OPENAI_API_KEY", "not a name"], "s6": true });
    images::set_prepared(&pg, &d, 52, &inspect, true, Some("v9.9.9"))
        .await
        .unwrap();
    let r = images::record(&pg, &d).await.unwrap().unwrap();
    assert_eq!((r.config_version, r.pulled_here), (Some(52), true));
    assert!(r.runnable());
    assert_eq!(r.version.as_deref(), Some("v0.22.6"));
    assert_eq!(r.stripped_env(), vec!["OPENAI_API_KEY"]);
    assert_eq!(
        r.image_ref(),
        format!("nousresearch/hermes-agent:v0.22.6@{d}")
    );

    // A digest with no tag takes the image's own version.
    let untagged = digest(tag, 1);
    images::note_seen(&pg, &untagged, "nousresearch/hermes-agent", None)
        .await
        .unwrap();
    images::set_prepared(
        &pg,
        &untagged,
        51,
        &serde_json::json!({}),
        false,
        Some("v0.22.5"),
    )
    .await
    .unwrap();
    assert_eq!(
        images::record(&pg, &untagged)
            .await
            .unwrap()
            .unwrap()
            .version
            .as_deref(),
        Some("v0.22.5")
    );

    // Good, then bad: bad sticks — a later "good" never revives it.
    images::mark_good(&pg, &d).await.unwrap();
    let r = images::record(&pg, &d).await.unwrap().unwrap();
    assert_eq!(r.status, images::Status::Good);
    assert!(r.good_since.is_some());
    images::mark_bad(&pg, &d, "the canary did not report healthy")
        .await
        .unwrap();
    images::mark_good(&pg, &d).await.unwrap();
    let r = images::record(&pg, &d).await.unwrap().unwrap();
    assert_eq!(r.status, images::Status::Bad);
    assert_eq!(
        r.reason.as_deref(),
        Some("the canary did not report healthy")
    );
    assert_eq!(r.good_since, None);

    // The compiled pin's seed never overwrites a record.
    images::seed_known_good(&pg, &d, "x", "v0.0.1", 1, "seed")
        .await
        .unwrap();
    assert_eq!(
        images::record(&pg, &d).await.unwrap().unwrap().status,
        images::Status::Bad
    );

    // Pruned: no longer on the host, so not runnable until prepared again.
    images::note_pruned(&pg, &untagged).await.unwrap();
    let r = images::record(&pg, &untagged).await.unwrap().unwrap();
    assert!(!r.runnable() && !r.pulled_here);
    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_assignment_is_one_per_agent_and_leaves_with_the_agent() {
    let pg = pg().await;
    let tag = "asg";
    cleanup(&pg, tag).await;
    let (id, _) = agent(&pg, tag, "ada").await;
    let (d1, d2) = (digest(tag, 0), digest(tag, 1));
    images::assign(&pg, &id, &d1).await.unwrap();
    images::assign(&pg, &id, &d2).await.unwrap();
    assert_eq!(images::assignments(&pg).await.unwrap().get(&id), Some(&d2));
    sqlx::query("delete from agent_defs where id = $1::uuid")
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
    assert_eq!(
        images::assignments(&pg).await.unwrap().get(&id),
        None,
        "cascaded"
    );
    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_mid_reply_or_mid_conversation_is_busy_and_a_quiet_one_is_not() {
    let pg = pg().await;
    let tag = "busy";
    cleanup(&pg, tag).await;
    let (id, model) = agent(&pg, tag, "bo").await;
    let ag = ChannelAgent {
        id: id.clone(),
        slug: format!("{PREFIX}-{tag}-bo"),
        department: format!("{PREFIX}-{tag}-bo"),
        model: model.clone(),
        display_name: "bo".into(),
        developer: false,
    };
    let user: String = sqlx::query_scalar(
        "insert into users (sub, email, name, role) values ($1, $1, 'b', 'member') returning id::text",
    )
    .bind(format!("{PREFIX}:{tag}:u"))
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(
        why_busy(db_signals(&pg, &ag).await.unwrap()),
        None,
        "a quiet agent"
    );

    let conv: String = sqlx::query_scalar(
        "insert into conversations (user_id, agent_model) values ($1::uuid, $2) returning id::text",
    )
    .bind(&user)
    .bind(&model)
    .fetch_one(&pg)
    .await
    .unwrap();
    // A reply being written an hour into its conversation: busy on the
    // stream, not on the conversation's age.
    let msg: String = sqlx::query_scalar(
        "insert into messages (conversation_id, seq, role, status, created_at, streamed_at) \
         values ($1::uuid, 1, 'assistant', 'streaming', now() - interval '1 hour', now()) \
         returning id::text",
    )
    .bind(&conv)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(
        why_busy(db_signals(&pg, &ag).await.unwrap()).as_deref(),
        Some("it is writing a reply")
    );
    // Its writer gone past the lease: an orphan, not a turn.
    sqlx::query(
        "update messages set streamed_at = now() - interval '20 minutes' where id = $1::uuid",
    )
    .bind(&msg)
    .execute(&pg)
    .await
    .unwrap();
    assert_eq!(why_busy(db_signals(&pg, &ag).await.unwrap()), None);
    // A message a minute ago: someone is in a conversation with it.
    sqlx::query(
        "insert into messages (conversation_id, seq, role, created_at) \
         values ($1::uuid, 2, 'user', now() - interval '1 minute')",
    )
    .bind(&conv)
    .execute(&pg)
    .await
    .unwrap();
    assert!(
        why_busy(db_signals(&pg, &ag).await.unwrap())
            .unwrap()
            .contains("a conversation with it")
    );
    sqlx::query("delete from conversations where id = $1::uuid")
        .bind(&conv)
        .execute(&pg)
        .await
        .unwrap();
    // A live work session.
    sqlx::query(
        "insert into runs (kind, state, input) values ('work-session', 'running', jsonb_build_object('agentModel', $1::text))",
    )
    .bind(&model)
    .execute(&pg)
    .await
    .unwrap();
    assert_eq!(
        why_busy(db_signals(&pg, &ag).await.unwrap()).as_deref(),
        Some("it has a work session in progress")
    );
    sqlx::query("delete from runs where kind = 'work-session' and input->>'agentModel' = $1")
        .bind(&model)
        .execute(&pg)
        .await
        .unwrap();
    // A model call a minute ago (a cron, a delegated turn).
    sqlx::query(
        "insert into usage_events (agent_model, source, created_at) values ($1, 'unattended', now() - interval '1 minute')",
    )
    .bind(&model)
    .execute(&pg)
    .await
    .unwrap();
    assert!(
        why_busy(db_signals(&pg, &ag).await.unwrap())
            .unwrap()
            .contains("model call")
    );
    sqlx::query("delete from usage_events where agent_model = $1")
        .bind(&model)
        .execute(&pg)
        .await
        .unwrap();
    assert_eq!(
        why_busy(db_signals(&pg, &ag).await.unwrap()),
        None,
        "quiet again"
    );
    // A workbench job the agent started.
    sqlx::query(
        "insert into workbench_jobs (agent_id, agent_model, repo, branch, status) \
         values ($1::uuid, $2, 'o/r', 'b', 'started')",
    )
    .bind(&id)
    .bind(&model)
    .execute(&pg)
    .await
    .unwrap();
    assert_eq!(
        why_busy(db_signals(&pg, &ag).await.unwrap()).as_deref(),
        Some("it has a workbench job in progress")
    );
    cleanup(&pg, tag).await;
}

// ── The state machine, against a scripted fleet ─────────────────────────────
//
// The engine (talaria-hermes-channel engine.rs) driven step by step through
// `step_with`, each test on its own state row and lock (`hermes-test:<tag>`),
// its own agents and digests, a clock it moves by hand, and a fleet whose
// rolls, canaries, health and busyness it scripts: what restarts onto what,
// when, and what the row says after — including an admin pressing Roll back
// or Pin while a step is mid-roll.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use futures_util::future::BoxFuture;
use talaria_hermes_channel::check::Found;
use talaria_hermes_channel::prepare::Prepared;
use talaria_hermes_channel::{Engine, Fault, FleetOps, Snapshot, Tuning};
use talaria_hermes_images::{ImageRecord, Phase, RolloutKind, Status, Window};

const MIN: i64 = 60_000;

struct Fake {
    pg: PgPool,
    now: AtomicI64,
    agents: Vec<ChannelAgent>,
    containers: Mutex<HashMap<String, Snapshot>>,
    /// Scripted roll results per agent, front first; none scripted is a
    /// clean roll.
    roll_script: Mutex<HashMap<String, VecDeque<Result<(), Fault>>>>,
    canary_script: Mutex<VecDeque<Result<String, Fault>>>,
    busy: Mutex<HashMap<String, String>>,
    window: AtomicBool,
    stalled: AtomicBool,
    found: Mutex<Option<Found>>,
    /// (agent, digest it was rolled onto), in order.
    rolls: Mutex<Vec<(String, String)>>,
    notes: Mutex<Vec<(String, String)>>,
    /// A roll of this agent waits here until released (the B1 race).
    hold: Mutex<
        Option<(
            String,
            std::sync::Arc<tokio::sync::Notify>,
            std::sync::Arc<tokio::sync::Notify>,
        )>,
    >,
    serial: AtomicI64,
}

impl Fake {
    fn new(pg: &PgPool, agents: Vec<ChannelAgent>, on: &str) -> Fake {
        let containers = agents
            .iter()
            .map(|a| {
                (
                    a.slug.clone(),
                    Snapshot {
                        exists: true,
                        running: true,
                        status: "running".into(),
                        health: Some("healthy".into()),
                        restart_count: 0,
                        container: format!("{}-0", a.slug),
                        image: on.to_string(),
                    },
                )
            })
            .collect();
        Fake {
            pg: pg.clone(),
            now: AtomicI64::new(talaria_agent_auth::now_ms()),
            agents,
            containers: Mutex::new(containers),
            roll_script: Mutex::new(HashMap::new()),
            canary_script: Mutex::new(VecDeque::new()),
            busy: Mutex::new(HashMap::new()),
            window: AtomicBool::new(true),
            stalled: AtomicBool::new(false),
            found: Mutex::new(None),
            rolls: Mutex::new(Vec::new()),
            notes: Mutex::new(Vec::new()),
            hold: Mutex::new(None),
            serial: AtomicI64::new(1),
        }
    }
    fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn on(&self, slug: &str) -> String {
        self.containers.lock().unwrap()[slug].image.clone()
    }
    fn crash(&self, slug: &str) {
        let mut c = self.containers.lock().unwrap();
        let s = c.get_mut(slug).unwrap();
        s.restart_count += 1;
    }
    fn rolled(&self) -> Vec<(String, String)> {
        self.rolls.lock().unwrap().clone()
    }
}

impl FleetOps for Fake {
    fn now_ms(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
    fn agents(&self) -> BoxFuture<'_, Result<Vec<ChannelAgent>, String>> {
        Box::pin(async move { Ok(self.agents.clone()) })
    }
    fn off_reason(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async { None })
    }
    fn check(&self) -> BoxFuture<'_, Result<Found, String>> {
        Box::pin(async move {
            self.found
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| "could not reach the registry".to_string())
        })
    }
    fn resolve<'a>(&'a self, _v: &'a str) -> BoxFuture<'a, Result<Found, String>> {
        Box::pin(async { Err("not in this test".to_string()) })
    }
    fn prepare<'a>(&'a self, _r: &'a ImageRecord) -> BoxFuture<'a, Prepared> {
        Box::pin(async { Prepared::Ready })
    }
    fn canary<'a>(&'a self, _r: &'a ImageRecord) -> BoxFuture<'a, Result<String, Fault>> {
        Box::pin(async move {
            self.canary_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok("the canary soaked".into()))
        })
    }
    fn snapshot<'a>(&'a self, a: &'a ChannelAgent) -> BoxFuture<'a, Result<Snapshot, String>> {
        Box::pin(async move { Ok(self.containers.lock().unwrap()[&a.slug].clone()) })
    }
    fn image_id<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            image
                .split_once('@')
                .map(|(_, d)| d.to_string())
                .ok_or_else(|| "no digest".to_string())
        })
    }
    fn roll<'a>(&'a self, a: &'a ChannelAgent, _start: bool) -> BoxFuture<'a, Result<(), Fault>> {
        Box::pin(async move {
            let held = self
                .hold
                .lock()
                .unwrap()
                .clone()
                .filter(|(slug, _, _)| slug == &a.slug);
            if let Some((_, reached, release)) = held {
                *self.hold.lock().unwrap() = None;
                reached.notify_one();
                release.notified().await;
            }
            let digest = images::assignments(&self.pg).await.unwrap()[&a.id].clone();
            self.rolls
                .lock()
                .unwrap()
                .push((a.slug.clone(), digest.clone()));
            let scripted = self
                .roll_script
                .lock()
                .unwrap()
                .get_mut(&a.slug)
                .and_then(|q| q.pop_front())
                .unwrap_or(Ok(()));
            let n = self.serial.fetch_add(1, Ordering::SeqCst);
            let mut c = self.containers.lock().unwrap();
            let s = c.get_mut(&a.slug).unwrap();
            *s = Snapshot {
                exists: true,
                running: scripted.is_ok(),
                status: if scripted.is_ok() {
                    "running"
                } else {
                    "restarting"
                }
                .into(),
                health: Some("healthy".into()),
                restart_count: 0,
                container: format!("{}-{n}", a.slug),
                image: digest,
            };
            scripted
        })
    }
    fn render<'a>(&'a self, _a: &'a ChannelAgent) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
    fn answers<'a>(&'a self, _a: &'a ChannelAgent) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn log_faults<'a>(&'a self, _a: &'a ChannelAgent) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async { Vec::new() })
    }
    fn busy<'a>(&'a self, a: &'a ChannelAgent) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move { Ok(self.busy.lock().unwrap().get(&a.slug).cloned()) })
    }
    fn window_open<'a>(&'a self, _w: &'a Window) -> BoxFuture<'a, (bool, String)> {
        Box::pin(async move { (self.window.load(Ordering::SeqCst), "UTC".to_string()) })
    }
    fn notify<'a>(
        &'a self,
        kind: &'a str,
        _topic: &'a str,
        title: &'a str,
        _body: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.notes
                .lock()
                .unwrap()
                .push((kind.to_string(), title.to_string()));
        })
    }
    fn remove_image<'a>(&'a self, _i: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn host_stalled(&self) -> bool {
        self.stalled.load(Ordering::SeqCst)
    }
}

fn tuning() -> Tuning {
    Tuning {
        min_stable_ms: 0,
        first_agent_soak_ms: 10 * MIN,
        agent_soak_ms: 3 * MIN,
        rollout_soak_ms: 15 * MIN,
        retry_confirm_ms: 5 * MIN,
        retry_infra_ms: 15 * MIN,
        forced_defer_ms: 3 * 60 * MIN,
        step_budget: std::time::Duration::from_secs(60),
    }
}

/// A test's world: its state row, two digests (`old` good and current, `new`
/// a prepared candidate stable names), and `n` agents on `old`.
struct World {
    pg: PgPool,
    key: String,
    tag: String,
    old: String,
    new: String,
    fake: Fake,
}

impl World {
    async fn new(tag: &str, n: usize) -> World {
        let pg = pg().await;
        cleanup(&pg, tag).await;
        let key = format!("hermes-test:{tag}");
        sqlx::query("delete from app_settings where key = $1")
            .bind(&key)
            .execute(&pg)
            .await
            .unwrap();
        let (old, new) = (digest(tag, 0), digest(tag, 1));
        for (d, v, good) in [(&old, "v9.0.1", true), (&new, "v9.0.2", false)] {
            images::note_seen(&pg, d, "test/hermes", Some(v))
                .await
                .unwrap();
            images::set_prepared(&pg, d, 52, &serde_json::json!({}), false, None)
                .await
                .unwrap();
            if good {
                images::mark_good(&pg, d).await.unwrap();
            }
        }
        let mut agents = Vec::new();
        for i in 0..n {
            let name = format!("a{i}");
            let (id, model) = agent(&pg, tag, &name).await;
            images::assign(&pg, &id, &old).await.unwrap();
            agents.push(ChannelAgent {
                id,
                slug: format!("{PREFIX}-{tag}-{name}"),
                department: format!("{PREFIX}-{tag}-{name}"),
                model,
                display_name: name,
                developer: false,
            });
        }
        let fake = Fake::new(&pg, agents, &old);
        let state = serde_json::json!({
            "current": old,
            "nextCheckAt": 9_999_999_999_999i64,
            "lastCheck": { "at": 1, "digest": new, "version": "v9.0.2" },
        });
        talaria_gateway::settings::set_setting(&pg, &key, &state)
            .await
            .unwrap();
        World {
            pg,
            key,
            tag: tag.into(),
            old,
            new,
            fake,
        }
    }
    fn engine<'a>(&'a self, t: &'a Tuning) -> Engine<'a> {
        Engine {
            pg: &self.pg,
            key: &self.key,
            ops: &self.fake,
            tuning: t,
        }
    }
    async fn step(&self, t: &Tuning) -> Vec<String> {
        talaria_hermes_channel::step_with(&self.pg, &self.key, &self.fake, t)
            .await
            .unwrap()
            .expect("the test's own lock")
    }
    async fn state(&self) -> talaria_hermes_images::ChannelState {
        images::load_state_at(&self.pg, &self.key).await.unwrap()
    }
    async fn status_of(&self, d: &str) -> Status {
        images::record(&self.pg, d).await.unwrap().unwrap().status
    }
    fn slug(&self, i: usize) -> String {
        self.fake.agents[i].slug.clone()
    }
    async fn done(self) {
        sqlx::query("delete from app_settings where key = $1")
            .bind(&self.key)
            .execute(&self.pg)
            .await
            .unwrap();
        cleanup(&self.pg, &self.tag).await;
    }
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_upgrade_canaries_then_moves_one_agent_at_a_time_soaks_and_finishes() {
    let w = World::new("flow", 2).await;
    let t = tuning();
    w.step(&t).await;
    // Canaried, then the first agent moved; the second waits for its soak.
    assert_eq!(w.fake.rolled(), vec![(w.slug(0), w.new.clone())]);
    let s = w.state().await;
    let r = s.rollout.clone().unwrap();
    assert_eq!((r.kind, r.phase), (RolloutKind::Upgrade, Phase::Rolling));
    assert!(r.canary_ok_at.is_some());
    w.step(&t).await;
    assert_eq!(
        w.fake.rolled().len(),
        1,
        "the first agent soaks 10 min first"
    );
    w.fake.advance(10 * MIN + 1);
    w.step(&t).await;
    assert_eq!(w.fake.rolled().len(), 2);
    assert_eq!(w.fake.on(&w.slug(1)), w.new);
    w.fake.advance(3 * MIN + 1);
    w.step(&t).await;
    assert_eq!(w.state().await.rollout.unwrap().phase, Phase::Soaking);
    assert_eq!(
        w.status_of(&w.new).await,
        Status::Candidate,
        "not good until it soaked"
    );
    w.fake.advance(15 * MIN + 1);
    w.step(&t).await;
    let s = w.state().await;
    assert_eq!(s.rollout.unwrap().phase, Phase::Done);
    assert_eq!(s.current.as_deref(), Some(w.new.as_str()));
    assert_eq!(s.previous.as_deref(), Some(w.old.as_str()));
    assert_eq!(w.status_of(&w.new).await, Status::Good);
    w.done().await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_crash_loop_in_the_soak_is_confirmed_then_bans_and_puts_every_agent_back() {
    let w = World::new("relapse", 2).await;
    let t = tuning();
    w.step(&t).await;
    // It came up healthy, and crashes a minute later: a restart.
    w.fake.advance(MIN);
    w.fake.crash(&w.slug(0));
    w.step(&t).await;
    let r = w.state().await.rollout.unwrap();
    assert_eq!(r.strikes, 1, "one failure is a question");
    assert_eq!(w.fake.on(&w.slug(0)), w.old, "the agent went back at once");
    assert_eq!(w.status_of(&w.new).await, Status::Candidate);
    // The retry that confirms it: moved again, crashes again.
    w.fake.advance(5 * MIN + 1);
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    w.fake.advance(MIN);
    w.fake.crash(&w.slug(0));
    w.step(&t).await;
    // Two are an answer: bad, and the fleet goes back, the second agent
    // never having moved.
    assert_eq!(w.status_of(&w.new).await, Status::Bad);
    for _ in 0..4 {
        w.fake.advance(16 * MIN);
        w.step(&t).await;
    }
    let s = w.state().await;
    let r = s.rollout.unwrap();
    assert_eq!((r.kind, r.phase), (RolloutKind::Rollback, Phase::Done));
    assert_eq!(s.current.as_deref(), Some(w.old.as_str()));
    assert_eq!(w.fake.on(&w.slug(0)), w.old);
    assert_eq!(w.fake.on(&w.slug(1)), w.old);
    assert!(
        !w.fake
            .rolled()
            .iter()
            .any(|(s, d)| s == &w.slug(1) && d == &w.new),
        "no agent moved while the first was failing"
    );
    let notes = w.fake.notes.lock().unwrap().clone();
    assert!(
        notes
            .iter()
            .any(|(k, title)| k == "hermes_update_failed" && title.contains("v9.0.2")),
        "the title names the version that failed: {notes:?}"
    );
    w.done().await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_failure_that_is_the_hosts_is_retried_and_never_bans() {
    let w = World::new("infra", 1).await;
    let t = tuning();
    for _ in 0..3 {
        w.fake
            .canary_script
            .lock()
            .unwrap()
            .push_back(Err(Fault::Infra(
                "docker compose … timed out after 600s".into(),
            )));
    }
    // An image failure while the host's disk is saturated is the host's.
    w.fake.stalled.store(true, Ordering::SeqCst);
    w.fake
        .canary_script
        .lock()
        .unwrap()
        .push_back(Err(Fault::Image(
            "the canary did not report healthy within 8 min".into(),
        )));
    for _ in 0..4 {
        w.step(&t).await;
        w.fake.advance(15 * MIN + 1);
    }
    let r = w.state().await.rollout.unwrap();
    assert_eq!(r.strikes, 0);
    assert_eq!(w.status_of(&w.new).await, Status::Candidate);
    assert!(w.fake.rolled().is_empty());
    // The host recovers: the canary passes and the rollout goes on.
    w.fake.stalled.store(false, Ordering::SeqCst);
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    w.done().await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn roll_back_pressed_mid_roll_leaves_the_fleet_on_known_good() {
    let w = std::sync::Arc::new(World::new("race", 2).await);
    let t = std::sync::Arc::new(tuning());
    // The first agent's roll waits until the test lets it go.
    let reached = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    *w.fake.hold.lock().unwrap() = Some((w.slug(0), reached.clone(), release.clone()));
    let (w2, t2) = (w.clone(), t.clone());
    let stepping = tokio::spawn(async move { w2.step(&t2).await });
    reached.notified().await;
    // Mid-roll, an admin presses Roll back.
    let said = talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    assert!(said.contains("marked bad"), "{said}");
    // The agent the stale step put on the bad image reads busy (Hermes
    // probing at boot, a crash loop): the rollback moves it back anyway.
    w.fake.busy.lock().unwrap().insert(
        w.slug(0),
        "it finished a model call in the last 5 minutes".into(),
    );
    release.notify_one();
    stepping.await.unwrap();
    for _ in 0..4 {
        w.fake.advance(16 * MIN);
        w.step(&t).await;
    }
    let s = w.state().await;
    let r = s.rollout.unwrap();
    assert_eq!(
        (r.kind, r.phase),
        (RolloutKind::Rollback, Phase::Done),
        "{:?}",
        r.detail
    );
    assert_eq!(
        s.current.as_deref(),
        Some(w.old.as_str()),
        "never the bad digest"
    );
    assert_eq!(
        w.fake.on(&w.slug(0)),
        w.old,
        "the agent the stale step moved went back"
    );
    assert_eq!(w.fake.on(&w.slug(1)), w.old);
    assert_eq!(w.status_of(&w.new).await, Status::Bad);
    std::sync::Arc::try_unwrap(w).ok().unwrap().done().await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_pin_mid_rollout_brings_the_agents_it_reached_back() {
    let w = World::new("pin", 2).await;
    let t = tuning();
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    // Pinned to what the fleet ran, while the first agent soaks.
    talaria_hermes_channel::pin_with(&w.engine(&t), Some(&w.old), None, "admin@test")
        .await
        .unwrap();
    for _ in 0..4 {
        w.fake.advance(16 * MIN);
        w.step(&t).await;
    }
    let s = w.state().await;
    assert_eq!(s.pinned.as_deref(), Some(w.old.as_str()));
    assert_eq!(s.rollout.unwrap().phase, Phase::Done);
    assert_eq!(w.fake.on(&w.slug(0)), w.old);
    assert_eq!(w.fake.on(&w.slug(1)), w.old);
    assert_eq!(
        w.status_of(&w.new).await,
        Status::Candidate,
        "a pin bans nothing"
    );
    // Rolling back from a pinned version moves the pin, not a ban.
    assert_eq!(w.status_of(&w.old).await, Status::Good);
    w.done().await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_state_row_that_cannot_be_read_moves_nothing_and_is_never_overwritten() {
    let w = World::new("corrupt", 1).await;
    let t = tuning();
    for stored in [
        serde_json::json!("not an object"),
        serde_json::json!({ "current": w.old, "rollout": { "id": "x", "phase": "verifying" } }),
    ] {
        talaria_gateway::settings::set_setting(&w.pg, &w.key, &stored)
            .await
            .unwrap();
        let out = talaria_hermes_channel::step_with(&w.pg, &w.key, &w.fake, &t).await;
        assert!(out.is_err(), "{out:?}");
        assert!(
            talaria_hermes_channel::roll_back_with(&w.engine(&t), "a")
                .await
                .is_err()
        );
        let after = talaria_gateway::settings::try_get_setting(&w.pg, &w.key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after, stored, "left exactly as it was");
        assert!(w.fake.rolled().is_empty());
    }
    w.done().await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_interrupted_move_is_verified_and_an_agent_it_left_down_is_started() {
    let w = World::new("resume", 1).await;
    let t = tuning();
    // A step was rolling the agent when the api stopped: assigned to the new
    // digest, its container stranded down.
    images::assign(&w.pg, &w.fake.agents[0].id, &w.new)
        .await
        .unwrap();
    {
        let mut c = w.fake.containers.lock().unwrap();
        let s = c.get_mut(&w.slug(0)).unwrap();
        s.running = false;
        s.status = "exited".into();
    }
    let slug = w.slug(0);
    let new = w.new.clone();
    let old = w.old.clone();
    w.engine(&t)
        .patch(move |s| {
            s.rollout = Some(talaria_hermes_images::Rollout {
                id: "r-resume".into(),
                digest: new,
                from: Some(old),
                kind: RolloutKind::Upgrade,
                phase: Phase::Rolling,
                agent: Some(slug),
                forced: true,
                ..Default::default()
            });
            Ok(())
        })
        .await
        .unwrap();
    w.step(&t).await;
    let r = w.state().await.rollout.unwrap();
    assert_eq!(r.moved, vec![w.slug(0)], "{:?}", r.detail);
    assert_eq!(r.agent, None);
    assert!(
        w.fake.containers.lock().unwrap()[&w.slug(0)].running,
        "started, not recorded down"
    );
    w.done().await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_busy_all_window_is_left_for_later_and_never_holds_the_release() {
    let w = World::new("busywin", 2).await;
    let t = tuning();
    w.fake.busy.lock().unwrap().insert(
        w.slug(1),
        "its scheduled job \"nightly\" is due in 4 min".into(),
    );
    w.step(&t).await;
    w.fake.advance(10 * MIN + 1);
    w.step(&t).await;
    let r = w.state().await.rollout.unwrap();
    assert!(
        r.skipped.iter().any(|s| s.slug == w.slug(1)),
        "passed over, not rolled"
    );
    // The window closes with it still busy: it is left for a later window,
    // and the rest soaks and finishes.
    w.fake.window.store(false, Ordering::SeqCst);
    w.step(&t).await;
    w.fake.advance(15 * MIN + 1);
    w.step(&t).await;
    let s = w.state().await;
    assert_eq!(s.current.as_deref(), Some(w.new.as_str()));
    assert_eq!(
        w.fake.on(&w.slug(1)),
        w.old,
        "never restarted under its job"
    );
    let assigned = images::assignments(&w.pg).await.unwrap();
    assert_eq!(assigned[&w.fake.agents[1].id], w.old, "held where it is");
    // It catches up in a later window, once it is free.
    w.fake.busy.lock().unwrap().clear();
    w.fake.window.store(true, Ordering::SeqCst);
    w.step(&t).await;
    let r = w.state().await.rollout.unwrap();
    assert_eq!(r.kind, RolloutKind::Catchup);
    assert_eq!(w.fake.on(&w.slug(1)), w.new);
    w.done().await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_release_waits_its_minimum_time_on_stable_unless_rolled_now() {
    let w = World::new("minage", 1).await;
    let mut t = tuning();
    t.min_stable_ms = 48 * 60 * MIN;
    w.step(&t).await;
    let r = w.state().await.rollout.unwrap();
    assert_eq!(r.phase, Phase::Waiting);
    assert!(r.detail.unwrap().contains("once it has been there 48 h"));
    assert!(w.fake.rolled().is_empty());
    talaria_hermes_channel::roll_now_with(&w.engine(&t))
        .await
        .unwrap();
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new, "Roll now does not wait");
    w.done().await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn allow_again_lifts_a_ban_and_a_banned_version_cannot_be_pinned() {
    let w = World::new("allow", 1).await;
    let t = tuning();
    images::mark_bad(&w.pg, &w.new, "the canary failed")
        .await
        .unwrap();
    w.step(&t).await;
    assert!(
        w.state().await.rollout.is_none(),
        "a bad digest is never rolled out"
    );
    assert!(
        talaria_hermes_channel::pin_with(&w.engine(&t), Some(&w.new), None, "a")
            .await
            .is_err()
    );
    talaria_hermes_channel::allow_with(&w.engine(&t), &w.new, "admin@test")
        .await
        .unwrap();
    w.step(&t).await;
    assert_eq!(w.state().await.rollout.unwrap().digest, w.new);
    w.done().await;
}
