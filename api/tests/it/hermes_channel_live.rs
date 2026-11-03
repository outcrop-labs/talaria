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
        stopped: false,
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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use futures_util::future::BoxFuture;
use talaria_hermes_channel::check::Found;
use talaria_hermes_channel::ops::Busy;
use talaria_hermes_channel::prepare::Prepared;
use talaria_hermes_channel::prepare::Room;
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
    /// Scripted preparations, front first; none scripted is the host's
    /// answer (`missing`).
    prepare_script: Mutex<VecDeque<Prepared>>,
    /// Scripted answers of an agent's API, front first; none is an answer.
    answers_script: Mutex<HashMap<String, VecDeque<Result<(), String>>>>,
    busy: Mutex<HashMap<String, String>>,
    /// Agents whose work cannot be read (an answer Talaria cannot parse).
    unknown: Mutex<HashMap<String, String>>,
    /// Agents a person stopped through Talaria (the recorded Stop).
    stopped: Mutex<HashSet<String>>,
    /// What a failed roll of an agent leaves, front first: the old container
    /// still running, or the agent down — what docker's own failure paths
    /// leave (talaria-fleet-docker `recover_failed_recreate`).
    roll_effect: Mutex<HashMap<String, VecDeque<RollEffect>>>,
    /// The room each preparation was asked to keep.
    rooms: Mutex<Vec<Room>>,
    /// Docker's classic image store: an image's id is not its digest.
    classic: AtomicBool,
    /// Each busy question: (agent, lenient).
    asked: Mutex<Vec<(String, bool)>>,
    window: AtomicBool,
    stalled: AtomicBool,
    found: Mutex<Option<Found>>,
    /// Digests whose image is not on the host: `image_id` refuses them, and
    /// a preparation pulls them (its first ask is the pull in progress).
    missing: Mutex<HashSet<String>>,
    /// Digests that leave the host just after a preparation answers Ready.
    vanish: Mutex<HashSet<String>>,
    /// (agent, digest it was recreated on), in order.
    rolls: Mutex<Vec<(String, String)>>,
    /// (kind, topic, title) of every notification.
    notes: Mutex<Vec<(String, String, String)>>,
    canary_calls: AtomicI64,
    /// The images this test may remove (any other is "in use"), and those
    /// removed — the suite's other tests share the records table.
    removable: Mutex<HashSet<String>>,
    removed: Mutex<Vec<String>>,
    /// A roll of this agent waits here until released (the B1 race).
    hold: Mutex<Option<Hold>>,
    /// A busy question of this agent waits here until released.
    busy_hold: Mutex<Option<Hold>>,
    serial: AtomicI64,
}

#[derive(Debug, Clone, Copy)]
enum RollEffect {
    /// The recreate did not land: the agent kept running on its previous
    /// container.
    KeepOld,
    /// The agent stopped, and starting it again failed too.
    LeaveDown,
}

/// (agent, reached, release): the fake says `reached` and waits on
/// `release`.
type Hold = (
    String,
    std::sync::Arc<tokio::sync::Notify>,
    std::sync::Arc<tokio::sync::Notify>,
);

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
                        started_at: "t0".into(),
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
            prepare_script: Mutex::new(VecDeque::new()),
            answers_script: Mutex::new(HashMap::new()),
            busy: Mutex::new(HashMap::new()),
            unknown: Mutex::new(HashMap::new()),
            stopped: Mutex::new(HashSet::new()),
            roll_effect: Mutex::new(HashMap::new()),
            rooms: Mutex::new(Vec::new()),
            classic: AtomicBool::new(false),
            asked: Mutex::new(Vec::new()),
            window: AtomicBool::new(true),
            stalled: AtomicBool::new(false),
            found: Mutex::new(None),
            missing: Mutex::new(HashSet::new()),
            vanish: Mutex::new(HashSet::new()),
            rolls: Mutex::new(Vec::new()),
            notes: Mutex::new(Vec::new()),
            canary_calls: AtomicI64::new(0),
            removable: Mutex::new(HashSet::new()),
            removed: Mutex::new(Vec::new()),
            hold: Mutex::new(None),
            busy_hold: Mutex::new(None),
            serial: AtomicI64::new(1),
        }
    }
    fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn on(&self, slug: &str) -> String {
        self.containers.lock().unwrap()[slug].image.clone()
    }
    fn running(&self, slug: &str) -> bool {
        self.containers.lock().unwrap()[slug].running
    }
    fn crash(&self, slug: &str) {
        let mut c = self.containers.lock().unwrap();
        let s = c.get_mut(slug).unwrap();
        s.restart_count += 1;
    }
    /// A person stopped it through Talaria: exited, the Stop recorded.
    fn stop(&self, slug: &str) {
        self.stopped.lock().unwrap().insert(slug.to_string());
        self.down(slug);
    }
    /// Down with no Stop recorded (a roll that failed left it so).
    fn down(&self, slug: &str) {
        let mut c = self.containers.lock().unwrap();
        let s = c.get_mut(slug).unwrap();
        s.running = false;
        s.status = "exited".into();
    }
    /// The daemon restarted it (a reboot): a new start, the same container
    /// and restart count, its health starting.
    fn daemon_restart(&self, slug: &str) {
        let now = self.now.load(Ordering::SeqCst);
        let mut c = self.containers.lock().unwrap();
        let s = c.get_mut(slug).unwrap();
        s.started_at = chrono::DateTime::from_timestamp_millis(now)
            .unwrap()
            .to_rfc3339();
        s.health = Some("starting".into());
    }
    /// It keeps dying and docker keeps restarting it.
    fn crashloop(&self, slug: &str) {
        let mut c = self.containers.lock().unwrap();
        let s = c.get_mut(slug).unwrap();
        s.running = false;
        s.status = "restarting".into();
        s.restart_count += 3;
    }
    /// The container runs `digest` (it was created on it).
    /// Docker's id for an image: its digest on the containerd store, the
    /// config's id (here: another string) on the classic one.
    fn id_of(&self, digest: &str) -> String {
        if self.classic.load(Ordering::SeqCst) {
            format!("sha256:c0{}", &digest[9..])
        } else {
            digest.to_string()
        }
    }
    fn runs(&self, slug: &str, digest: &str) {
        let mut c = self.containers.lock().unwrap();
        c.get_mut(slug).unwrap().image = digest.to_string();
    }
    fn rolled(&self) -> Vec<(String, String)> {
        self.rolls.lock().unwrap().clone()
    }
    fn notes_like(&self, needle: &str) -> usize {
        self.notes
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, topic, title)| {
                k.contains(needle) || topic.contains(needle) || title.contains(needle)
            })
            .count()
    }
}

impl FleetOps for Fake {
    fn now_ms(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
    fn agents(&self) -> BoxFuture<'_, Result<Vec<ChannelAgent>, String>> {
        Box::pin(async move {
            let stopped = self.stopped.lock().unwrap().clone();
            Ok(self
                .agents
                .iter()
                .cloned()
                .map(|mut a| {
                    a.stopped = stopped.contains(&a.slug);
                    a
                })
                .collect())
        })
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
    fn prepare<'a>(&'a self, r: &'a ImageRecord, room: Room) -> BoxFuture<'a, Prepared> {
        Box::pin(async move {
            self.rooms.lock().unwrap().push(room);
            if let Some(p) = self.prepare_script.lock().unwrap().pop_front() {
                return p;
            }
            // The host's answer: an image not on it is pulled (the first ask
            // finds the download in progress), one on it is ready.
            if self.missing.lock().unwrap().remove(&r.digest) {
                return Prepared::Pulling("downloading".into());
            }
            if self.vanish.lock().unwrap().remove(&r.digest) {
                self.missing.lock().unwrap().insert(r.digest.clone());
            }
            Prepared::Ready
        })
    }
    fn canary<'a>(&'a self, _r: &'a ImageRecord) -> BoxFuture<'a, Result<String, Fault>> {
        Box::pin(async move {
            self.canary_calls.fetch_add(1, Ordering::SeqCst);
            self.canary_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok("the canary soaked".into()))
        })
    }
    fn snapshot<'a>(&'a self, a: &'a ChannelAgent) -> BoxFuture<'a, Result<Snapshot, String>> {
        Box::pin(async move {
            let mut s = self.containers.lock().unwrap()[&a.slug].clone();
            s.image = self.id_of(&s.image);
            Ok(s)
        })
    }
    fn image_id<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let digest = image
                .split_once('@')
                .map(|(_, d)| d.to_string())
                .ok_or_else(|| "no digest".to_string())?;
            if self.missing.lock().unwrap().contains(&digest) {
                return Err(format!(
                    "Error response from daemon: No such image: {image}"
                ));
            }
            Ok(self.id_of(&digest))
        })
    }
    fn roll<'a>(&'a self, a: &'a ChannelAgent, start: bool) -> BoxFuture<'a, Result<(), Fault>> {
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
            // The reconcile's IfRunning: a container that is not running when
            // the roll reaches it is left exactly as it is.
            if !start && !self.containers.lock().unwrap()[&a.slug].running {
                return Ok(());
            }
            let digest = images::assignments(&self.pg).await.unwrap()[&a.id].clone();
            let effect = self
                .roll_effect
                .lock()
                .unwrap()
                .get_mut(&a.slug)
                .and_then(|q| q.pop_front());
            match effect {
                Some(RollEffect::KeepOld) => {
                    return Err(Fault::Infra(
                        "the change did not land: the agent kept running on its previous container"
                            .into(),
                    ));
                }
                Some(RollEffect::LeaveDown) => {
                    let mut c = self.containers.lock().unwrap();
                    let s = c.get_mut(&a.slug).unwrap();
                    s.running = false;
                    s.status = "exited".into();
                    return Err(Fault::Infra(
                        "the agent stopped, and starting it again failed too".into(),
                    ));
                }
                None => {}
            }
            // A recreate starts it: any recorded Stop is cleared.
            self.stopped.lock().unwrap().remove(&a.slug);
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
                started_at: format!("s{n}"),
            };
            scripted
        })
    }
    fn render<'a>(&'a self, _a: &'a ChannelAgent) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
    fn answers<'a>(&'a self, a: &'a ChannelAgent) -> BoxFuture<'a, Result<(), Fault>> {
        Box::pin(async move {
            self.answers_script
                .lock()
                .unwrap()
                .get_mut(&a.slug)
                .and_then(|q| q.pop_front())
                .unwrap_or(Ok(()))
                .map_err(Fault::Image)
        })
    }
    fn log_faults<'a>(&'a self, _a: &'a ChannelAgent) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async { Vec::new() })
    }
    fn busy<'a>(
        &'a self,
        a: &'a ChannelAgent,
        lenient: bool,
    ) -> BoxFuture<'a, Result<Busy, String>> {
        Box::pin(async move {
            self.asked.lock().unwrap().push((a.slug.clone(), lenient));
            let held = self
                .busy_hold
                .lock()
                .unwrap()
                .clone()
                .filter(|(slug, _, _)| slug == &a.slug);
            if let Some((_, reached, release)) = held {
                *self.busy_hold.lock().unwrap() = None;
                reached.notify_one();
                release.notified().await;
            }
            if let Some(why) = self.busy.lock().unwrap().get(&a.slug).cloned() {
                return Ok(Busy::Working(why));
            }
            if let Some(why) = self.unknown.lock().unwrap().get(&a.slug).cloned() {
                return Ok(Busy::Unknown(why));
            }
            Ok(Busy::Idle)
        })
    }
    fn window_open<'a>(&'a self, _w: &'a Window) -> BoxFuture<'a, (bool, String)> {
        Box::pin(async move { (self.window.load(Ordering::SeqCst), "UTC".to_string()) })
    }
    fn notify<'a>(
        &'a self,
        kind: &'a str,
        topic: &'a str,
        title: &'a str,
        _body: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.notes.lock().unwrap().push((
                kind.to_string(),
                topic.to_string(),
                title.to_string(),
            ));
        })
    }
    fn remove_image<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let digest = image.split_once('@').map(|(_, d)| d).unwrap_or(image);
            if !self.removable.lock().unwrap().contains(digest) {
                return Err("image is being used by a running container".into());
            }
            self.removed.lock().unwrap().push(digest.to_string());
            Ok(())
        })
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
                stopped: false,
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
            fence: None,
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
    /// A further digest (`n` ≥ 2): recorded and prepared; good or a
    /// candidate.
    async fn extra(&self, n: u8, version: &str, good: bool) -> String {
        let d = digest(&self.tag, n);
        images::note_seen(&self.pg, &d, "test/hermes", Some(version))
            .await
            .unwrap();
        images::set_prepared(&self.pg, &d, 52, &serde_json::json!({}), false, None)
            .await
            .unwrap();
        if good {
            images::mark_good(&self.pg, &d).await.unwrap();
        }
        d
    }
    /// Hermes' stable names `d` from the next check, which is due now.
    async fn stable_is(&self, d: &str, version: &str) {
        *self.fake.found.lock().unwrap() = Some(Found {
            digest: d.into(),
            repository: "test/hermes".into(),
            version: Some(version.into()),
            new: false,
        });
        self.engine(&tuning())
            .patch(|s| {
                s.next_check_at = Some(0);
                Ok(())
            })
            .await
            .unwrap();
    }
    /// Agent `i` assigned, and running, `d`.
    async fn put(&self, i: usize, d: &str) {
        images::assign(&self.pg, &self.fake.agents[i].id, d)
            .await
            .unwrap();
        self.fake.runs(&self.slug(i), d);
    }
    async fn assigned(&self, i: usize) -> Option<String> {
        images::assignments(&self.pg)
            .await
            .unwrap()
            .get(&self.fake.agents[i].id)
            .cloned()
    }
    /// The fleet already runs `new` (good and current), `old` before it.
    async fn on_new(&self) {
        images::mark_good(&self.pg, &self.new).await.unwrap();
        for i in 0..self.fake.agents.len() {
            self.put(i, &self.new).await;
        }
        let (old, new) = (self.old.clone(), self.new.clone());
        self.engine(&tuning())
            .patch(move |s| {
                s.current = Some(new.clone());
                s.previous = Some(old);
                s.last_check = Some(images::CheckRecord {
                    at: 1,
                    digest: Some(new),
                    version: Some("v9.0.2".into()),
                    ..Default::default()
                });
                Ok(())
            })
            .await
            .unwrap();
    }
    /// What holds after every step: an agent the open rollout watches is
    /// assigned the rollout's image.
    async fn invariants(&self) {
        let s = self.state().await;
        let Some(r) = s.rollout.filter(|r| r.open()) else {
            return;
        };
        let assigned = images::assignments(&self.pg).await.unwrap();
        for watched in &r.watch {
            let a = self
                .fake
                .agents
                .iter()
                .find(|a| a.slug == watched.slug)
                .unwrap();
            assert_eq!(
                assigned.get(&a.id),
                Some(&r.digest),
                "{} is watched on the rollout's image but assigned another",
                watched.slug
            );
        }
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
            .any(|(k, _, title)| k == "hermes_update_failed" && title.contains("v9.0.2")),
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
    // Each retry waits longer than the last; all four come and go.
    for _ in 0..12 {
        if w.fake.canary_calls.load(Ordering::SeqCst) == 4 {
            break;
        }
        w.step(&t).await;
        w.fake.advance(60 * MIN);
    }
    assert_eq!(w.fake.canary_calls.load(Ordering::SeqCst), 4);
    let r = w.state().await.rollout.unwrap();
    assert_eq!(r.strikes, 0);
    assert_eq!(r.infra_failures, 4);
    assert_eq!(w.status_of(&w.new).await, Status::Candidate);
    assert!(w.fake.rolled().is_empty());
    // The host recovers: the canary passes and the rollout goes on.
    w.fake.stalled.store(false, Ordering::SeqCst);
    w.fake.advance(6 * 60 * MIN);
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
    // Not even downloaded before its time on stable.
    let r = w.state().await.rollout.unwrap();
    assert_eq!(r.phase, Phase::Preparing);
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

// ── Round three: the re-review's findings, each a test that failed first ────

/// Engine B1: a release Hermes pulls off stable while it waits never runs.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_waiting_release_gives_way_when_stable_moves_on() {
    let w = World::new("supersede", 1).await;
    let mut t = tuning();
    t.min_stable_ms = 48 * 60 * MIN;
    w.step(&t).await;
    assert_eq!(w.state().await.rollout.unwrap().digest, w.new);
    // Twelve hours in, Hermes moves stable on to a hotfix.
    w.fake.advance(12 * 60 * MIN);
    let hotfix = w.extra(2, "v9.0.3", false).await;
    w.stable_is(&hotfix, "v9.0.3").await;
    w.step(&t).await;
    let r = w.state().await.rollout.unwrap();
    assert_eq!(
        r.digest, hotfix,
        "the waiting release gave way to the hotfix"
    );
    assert_eq!(
        w.status_of(&w.new).await,
        Status::Candidate,
        "giving way bans nothing"
    );
    // The hotfix waits its own 48 h, counted from when stable named it.
    for _ in 0..47 {
        w.fake.advance(60 * MIN);
        w.step(&t).await;
    }
    assert!(w.fake.rolled().is_empty(), "{:?}", w.fake.rolled());
    w.fake.advance(2 * 60 * MIN);
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), hotfix);
    assert!(
        !w.fake.rolled().iter().any(|(_, d)| d == &w.new),
        "the release Hermes pulled never ran"
    );
    w.done().await;
}

/// Engine B1: stable going back to the fleet's own image cancels the wait.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_waiting_release_that_left_stable_is_cancelled() {
    let w = World::new("revert", 1).await;
    let mut t = tuning();
    t.min_stable_ms = 48 * 60 * MIN;
    w.step(&t).await;
    assert_eq!(w.state().await.rollout.unwrap().digest, w.new);
    w.stable_is(&w.old, "v9.0.1").await;
    w.step(&t).await;
    assert!(
        !w.state().await.rollout.unwrap().open(),
        "the wait for a release stable no longer names is over"
    );
    w.fake.advance(49 * 60 * MIN);
    w.step(&t).await;
    assert!(w.fake.rolled().is_empty(), "{:?}", w.fake.rolled());
    assert_eq!(w.status_of(&w.new).await, Status::Candidate);
    w.done().await;
}

/// Engine B2(a): an agent put back while the host is saturated leaves the
/// watch, and the rollout goes on once the host recovers.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_relapse_while_the_host_is_saturated_never_sticks_the_rollout() {
    let w = World::new("stallrel", 2).await;
    let t = tuning();
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    w.fake.advance(MIN);
    w.fake.stalled.store(true, Ordering::SeqCst);
    w.fake.crash(&w.slug(0));
    w.step(&t).await;
    w.invariants().await;
    assert_eq!(w.fake.on(&w.slug(0)), w.old, "put back at once");
    assert_eq!(w.status_of(&w.new).await, Status::Candidate);
    w.fake.stalled.store(false, Ordering::SeqCst);
    for _ in 0..12 {
        w.fake.advance(16 * MIN);
        w.step(&t).await;
        w.invariants().await;
    }
    let s = w.state().await;
    assert_eq!(
        s.rollout.as_ref().map(|r| r.phase),
        Some(Phase::Done),
        "{:?}",
        s.rollout.as_ref().and_then(|r| r.detail.clone())
    );
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    assert_eq!(w.fake.on(&w.slug(1)), w.new);
    w.done().await;
}

/// Engine B2(b): a catch-up's agent that fails one health question never
/// sticks the catch-up.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_catchup_agent_that_fails_once_never_sticks_the_catchup() {
    let w = World::new("catchrel", 2).await;
    let t = tuning();
    w.on_new().await;
    // Agent 1 was left behind on the old image by an earlier rollout.
    w.put(1, &w.old).await;
    // It answers its first question and times out on the next.
    w.fake
        .answers_script
        .lock()
        .unwrap()
        .insert(w.slug(1), VecDeque::from([Ok(()), Err("timed out".into())]));
    for _ in 0..6 {
        w.step(&t).await;
        w.invariants().await;
        w.fake.advance(16 * MIN);
    }
    let r = w.state().await.rollout.unwrap();
    assert_eq!(r.kind, RolloutKind::Catchup);
    assert!(!r.open(), "the catch-up closed: {:?}", r.detail);
    w.done().await;
}

/// Engine B2(c) and S6: a rollback tells a person about a stuck agent once,
/// never every 15 minutes.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_rollback_never_alerts_every_retry() {
    let w = World::new("rbloop", 2).await;
    let t = tuning();
    w.on_new().await;
    w.fake
        .busy
        .lock()
        .unwrap()
        .insert(w.slug(1), "it is writing a reply".into());
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    w.step(&t).await;
    // Agent 0 restarts once on the image it went back to.
    w.fake.advance(MIN);
    w.fake.crash(&w.slug(0));
    for _ in 0..8 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    let stuck = w.fake.notes_like("is stuck");
    assert!(
        stuck <= 1,
        "{stuck} stuck alerts: {:?}",
        w.fake.notes.lock().unwrap()
    );
    w.done().await;
}

/// Engine B3: a rollback never starts an agent a person stopped.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_rollback_never_starts_an_agent_a_person_stopped() {
    let w = World::new("stopped", 2).await;
    let t = tuning();
    w.fake.stop(&w.slug(1));
    w.step(&t).await;
    w.fake.advance(10 * MIN + 1);
    w.step(&t).await;
    assert_eq!(w.assigned(1).await.as_deref(), Some(w.new.as_str()));
    assert!(!w.fake.running(&w.slug(1)), "assigned, not started");
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    for _ in 0..4 {
        w.fake.advance(16 * MIN);
        w.step(&t).await;
    }
    assert!(
        !w.fake.running(&w.slug(1)),
        "the rollback started an agent a person stopped"
    );
    assert_eq!(
        w.assigned(1).await.as_deref(),
        Some(w.old.as_str()),
        "it starts on the known good next time"
    );
    assert_eq!(w.fake.on(&w.slug(0)), w.old);
    w.done().await;
}

/// Engine B3: a Pin pressed over a rollback recreates an agent that is
/// crash-looping on the bad image.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_pin_over_a_rollback_recreates_an_agent_crash_looping_on_the_bad_image() {
    let w = World::new("pinloop", 2).await;
    let t = tuning();
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    w.fake.crashloop(&w.slug(0));
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    // Before any step moves it, the admin pins the old version instead.
    talaria_hermes_channel::pin_with(&w.engine(&t), Some(&w.old), None, "admin@test")
        .await
        .unwrap();
    for _ in 0..4 {
        w.fake.advance(16 * MIN);
        w.step(&t).await;
    }
    assert_eq!(w.fake.on(&w.slug(0)), w.old, "recreated off the bad image");
    assert!(w.fake.running(&w.slug(0)));
    w.done().await;
}

/// Engine B4 / surface S1: a state row this build cannot read never stops a
/// render — it plans the fleet's current image, or the compiled pin.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_state_row_this_build_cannot_read_never_stops_the_render() {
    let w = World::new("plan", 1).await;
    let compiled = images::Planned {
        image: "test/hermes:v0.0.1@sha256:0000000000000000000000000000000000000000000000000000000000000000".into(),
        config_version: 51,
    };
    let newer = serde_json::json!({
        "current": w.new,
        "rollout": { "id": "x", "digest": w.new, "phase": "verifying", "kind": "staged" }
    });
    talaria_gateway::settings::set_setting(&w.pg, &w.key, &newer)
        .await
        .unwrap();
    let plan = images::image_plan_at(&w.pg, &w.key, compiled.clone())
        .await
        .expect("a later release's rollout never stops the render");
    assert!(
        plan.default.image.ends_with(&w.new),
        "it plans the current image: {plan:?}"
    );
    assert!(
        plan.for_agent(&w.fake.agents[0].id).image.ends_with(&w.old),
        "an agent keeps its own image"
    );
    talaria_gateway::settings::set_setting(&w.pg, &w.key, &serde_json::json!("garbled"))
        .await
        .unwrap();
    let plan = images::image_plan_at(&w.pg, &w.key, compiled.clone())
        .await
        .expect("a garbled row never stops the render");
    assert_eq!(plan.default, compiled);
    assert!(plan.for_agent(&w.fake.agents[0].id).image.ends_with(&w.old));
    w.done().await;
}

/// Engine B5: an agent the step never touched is never restarted for a
/// failure that came before it.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_the_step_never_touched_is_never_restarted() {
    let w = World::new("untouched", 1).await;
    let t = tuning();
    // The candidate is prepared, then leaves the host (an operator's prune).
    w.fake.vanish.lock().unwrap().insert(w.new.clone());
    for _ in 0..4 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    assert_eq!(
        w.fake.rolled(),
        vec![(w.slug(0), w.new.clone())],
        "pulled again and moved once — never put back"
    );
    w.done().await;
}

/// Engine B5: a rollback whose image left the host pulls it again and
/// finishes, without an alert every retry.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_rollback_whose_image_left_the_host_pulls_it_and_finishes() {
    let w = World::new("rbpull", 2).await;
    let t = tuning();
    w.on_new().await;
    w.fake.vanish.lock().unwrap().insert(w.old.clone());
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    for _ in 0..6 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    let r = w.state().await.rollout.unwrap();
    assert_eq!(r.phase, Phase::Done, "{:?}", r.detail);
    assert_eq!(w.fake.on(&w.slug(0)), w.old);
    assert_eq!(w.fake.on(&w.slug(1)), w.old);
    assert!(w.fake.notes_like("is stuck") <= 1);
    w.done().await;
}

/// Engine B5: a bad image cannot escape its ban by failing only while the
/// host is saturated.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_bad_image_is_judged_even_while_the_host_stays_saturated() {
    let w = World::new("stallban", 1).await;
    let t = tuning();
    w.fake.stalled.store(true, Ordering::SeqCst);
    for _ in 0..12 {
        w.fake
            .canary_script
            .lock()
            .unwrap()
            .push_back(Err(Fault::Image(
                "the canary exited 4s into its soak".into(),
            )));
    }
    for _ in 0..12 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    assert_eq!(w.status_of(&w.new).await, Status::Bad);
    assert!(w.fake.rolled().is_empty());
    w.done().await;
}

/// Engine B5 / surface S6: a host failure that repeats backs off and is said
/// to a person once.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_host_failure_backs_off_and_is_said_once() {
    let w = World::new("backoff", 1).await;
    let t = tuning();
    for _ in 0..200 {
        w.fake
            .canary_script
            .lock()
            .unwrap()
            .push_back(Err(Fault::Infra(
                "docker compose … timed out after 600s".into(),
            )));
    }
    // A day of steps, one every 15 minutes.
    for _ in 0..96 {
        w.step(&t).await;
        w.fake.advance(15 * MIN);
    }
    let tries = w.fake.canary_calls.load(Ordering::SeqCst);
    assert!(tries <= 12, "{tries} canaries in a day");
    assert_eq!(
        w.fake.notes_like("hermes_channel_stuck"),
        1,
        "{:?}",
        w.fake.notes.lock().unwrap()
    );
    assert_eq!(w.status_of(&w.new).await, Status::Candidate);
    w.done().await;
}

/// Engine S1: a preparation that fails once is asked again before anything
/// is banned.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_preparation_that_fails_once_bans_nothing() {
    let w = World::new("prepone", 1).await;
    let t = tuning();
    w.fake
        .prepare_script
        .lock()
        .unwrap()
        .push_back(Prepared::Failed {
            why: "the image could not be inspected: no space left on device".into(),
            definitive: true,
        });
    for _ in 0..3 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    assert_ne!(w.status_of(&w.new).await, Status::Bad);
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    w.done().await;
}

/// Engine S1: a rollback never bans the version it goes back to.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_rollback_never_bans_the_version_it_goes_back_to() {
    let w = World::new("rbban", 1).await;
    let t = tuning();
    w.on_new().await;
    for _ in 0..3 {
        w.fake
            .prepare_script
            .lock()
            .unwrap()
            .push_back(Prepared::Failed {
                why: "the image printed no inspection".into(),
                definitive: true,
            });
    }
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    for _ in 0..8 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    assert_eq!(w.status_of(&w.old).await, Status::Good);
    assert_eq!(w.fake.on(&w.slug(0)), w.old);
    w.done().await;
}

/// Engine S1: a Pin to a good version never bans it.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_pin_to_a_good_version_never_bans_it() {
    let w = World::new("pinban", 1).await;
    let t = tuning();
    w.on_new().await;
    // Every roll of it fails (a put-back's included): slow to boot on a
    // loaded host, which is not the image's fault.
    w.fake.roll_script.lock().unwrap().insert(
        w.slug(0),
        (0..6)
            .map(|_| {
                Err(Fault::Image(
                    "the recreated container did not report healthy within 900s".into(),
                ))
            })
            .collect(),
    );
    talaria_hermes_channel::pin_with(&w.engine(&t), Some(&w.old), None, "admin@test")
        .await
        .unwrap();
    for _ in 0..6 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    let s = w.state().await;
    assert_eq!(w.status_of(&w.old).await, Status::Good, "never banned");
    assert_eq!(s.pinned.as_deref(), Some(w.old.as_str()), "still pinned");
    assert!(
        !w.fake.rolled().iter().any(|(_, d)| d == &w.new),
        "never put back on the version the admin pinned away from: {:?}",
        w.fake.rolled()
    );
    w.done().await;
}

/// Engine S2: one failure of the canary and one of an agent, days apart,
/// are two questions — not an answer.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn strikes_on_different_things_do_not_add_up() {
    let w = World::new("strikes", 2).await;
    let t = tuning();
    w.fake
        .canary_script
        .lock()
        .unwrap()
        .push_back(Err(Fault::Image(
            "the canary exited before it reported healthy".into(),
        )));
    w.step(&t).await;
    w.fake.advance(6 * MIN);
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new, "the retried canary passed");
    w.fake.roll_script.lock().unwrap().insert(
        w.slug(1),
        VecDeque::from([Err(Fault::Image(
            "the recreated container stopped running before it reported healthy".into(),
        ))]),
    );
    for _ in 0..8 {
        w.fake.advance(11 * MIN);
        w.step(&t).await;
    }
    assert_ne!(w.status_of(&w.new).await, Status::Bad);
    assert_eq!(w.fake.on(&w.slug(1)), w.new);
    w.done().await;
}

/// Engine S3: an agent is never assigned the image a Roll back left, by a
/// step that read the row before the Roll back.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_assignment_is_never_written_after_a_roll_back_committed() {
    let w = std::sync::Arc::new(World::new("asgrace", 1).await);
    let t = std::sync::Arc::new(tuning());
    let reached = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    *w.fake.busy_hold.lock().unwrap() = Some((w.slug(0), reached.clone(), release.clone()));
    let (w2, t2) = (w.clone(), t.clone());
    let stepping = tokio::spawn(async move { w2.step(&t2).await });
    reached.notified().await;
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    // The rollback cannot move it in this step (its image left the host), so
    // what the stale step wrote is what the render would read.
    w.fake.vanish.lock().unwrap().insert(w.old.clone());
    release.notify_one();
    stepping.await.unwrap();
    assert_ne!(
        w.assigned(0).await.as_deref(),
        Some(w.new.as_str()),
        "the stale step assigned the banned image"
    );
    std::sync::Arc::try_unwrap(w).ok().unwrap().done().await;
}

/// Engine S5: an agent left behind window after window is said to a person.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_left_behind_window_after_window_is_said_once() {
    let w = World::new("leftbehind", 2).await;
    let t = tuning();
    w.fake.busy.lock().unwrap().insert(
        w.slug(1),
        "its scheduled job \"sweep\" is due in 2 min".into(),
    );
    for _ in 0..4 {
        // A night: the window opens, the step runs, the window closes.
        w.fake.window.store(true, Ordering::SeqCst);
        for _ in 0..3 {
            w.step(&t).await;
            w.fake.advance(11 * MIN);
        }
        w.fake.window.store(false, Ordering::SeqCst);
        for _ in 0..3 {
            w.step(&t).await;
            w.fake.advance(16 * MIN);
        }
        w.fake.advance(20 * 60 * MIN);
    }
    assert_eq!(
        w.fake.on(&w.slug(1)),
        w.old,
        "never restarted under its job"
    );
    assert_eq!(
        w.fake.notes_like("left behind"),
        1,
        "{:?}",
        w.fake.notes.lock().unwrap()
    );
    w.done().await;
}

/// Engine S8: a rollback moves only the agents on the image it leaves.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_rollback_moves_only_the_agents_on_the_image_it_leaves() {
    let w = World::new("rbonly", 2).await;
    let t = tuning();
    w.on_new().await;
    // Agent 1 never left an older good image (busy at every rollout since).
    let older = w.extra(2, "v9.0.0", true).await;
    w.put(1, &older).await;
    w.fake
        .busy
        .lock()
        .unwrap()
        .insert(w.slug(1), "it has a work session in progress".into());
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    for _ in 0..4 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    let s = w.state().await;
    assert!(
        s.events
            .iter()
            .any(|e| e.text.contains("every agent is back on")),
        "the rollback finished without waiting on agent 1: {:?}",
        s.events
    );
    assert_eq!(w.fake.on(&w.slug(0)), w.old);
    assert_eq!(w.fake.on(&w.slug(1)), older, "never touched");
    assert!(
        !w.fake.rolled().iter().any(|(slug, _)| slug == &w.slug(1)),
        "never restarted"
    );
    // What is left is an ordinary catch-up, which waits for it to be free.
    let r = s.rollout.unwrap();
    assert_eq!(r.kind, RolloutKind::Catchup);
    w.done().await;
}

/// The not-verified list: two steps at once — one runs, the other stands
/// aside.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn two_steps_at_once_take_turns() {
    let w = std::sync::Arc::new(World::new("twosteps", 1).await);
    let t = std::sync::Arc::new(tuning());
    let reached = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    *w.fake.hold.lock().unwrap() = Some((w.slug(0), reached.clone(), release.clone()));
    let (w2, t2) = (w.clone(), t.clone());
    let first = tokio::spawn(async move { w2.step(&t2).await });
    reached.notified().await;
    let second = talaria_hermes_channel::step_with(&w.pg, &w.key, &w.fake, &t)
        .await
        .unwrap();
    assert_eq!(second, None, "the second step stood aside");
    release.notify_one();
    first.await.unwrap();
    assert_eq!(w.fake.rolled().len(), 1);
    std::sync::Arc::try_unwrap(w).ok().unwrap().done().await;
}

/// The not-verified list: the prune, on a fake clock — the previous image
/// once the current one has been good three days, and a candidate nobody
/// rolled out after a week; never the current, an agent's, or one this
/// instance did not pull.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_prune_removes_the_previous_and_the_abandoned_and_keeps_the_rest() {
    let w = World::new("prune", 1).await;
    let t = tuning();
    let mine = w.extra(2, "v9.0.3", true).await;
    let abandoned = w.extra(3, "v9.0.4", false).await;
    // old: current, good four days; new: previous; `mine`: agent 0's; the
    // abandoned candidate first seen eight days ago. All pulled here.
    for (d, sql) in [
        (
            &w.old,
            "update hermes_images set pulled_here = true, good_since = now() - interval '4 days' where digest = $1",
        ),
        (
            &w.new,
            "update hermes_images set pulled_here = true, status = 'good', good_since = now() - interval '9 days' where digest = $1",
        ),
        (
            &mine,
            "update hermes_images set pulled_here = true where digest = $1",
        ),
        (
            &abandoned,
            "update hermes_images set pulled_here = true, first_seen = now() - interval '8 days' where digest = $1",
        ),
    ] {
        sqlx::query(sql).bind(d).execute(&w.pg).await.unwrap();
    }
    w.put(0, &mine).await;
    for d in [&w.old, &w.new, &mine, &abandoned] {
        w.fake.removable.lock().unwrap().insert(d.to_string());
    }
    let (old, new) = (w.old.clone(), w.new.clone());
    let (state, _) = w
        .engine(&t)
        .patch(move |s| {
            s.current = Some(old);
            s.previous = Some(new);
            Ok(())
        })
        .await
        .unwrap();
    let said = talaria_hermes_channel::prune(&w.engine(&t), &state).await;
    let mut removed = w.fake.removed.lock().unwrap().clone();
    removed.sort();
    let mut want = vec![w.new.clone(), abandoned.clone()];
    want.sort();
    assert_eq!(removed, want, "{said:?}");
    for d in [&w.new, &abandoned] {
        let r = images::record(&w.pg, d).await.unwrap().unwrap();
        assert!(!r.pulled_here && !r.runnable(), "no longer on the host");
    }
    assert!(
        images::record(&w.pg, &w.old)
            .await
            .unwrap()
            .unwrap()
            .runnable()
    );
    w.done().await;
}

/// Engine S4: a step whose lock went with its database connection (a
/// failover, a terminated backend) stops at its next write, before it moves
/// anything else — while the process lives on.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_step_that_lost_its_lock_stops_before_it_moves_anything_else() {
    let w = std::sync::Arc::new(World::new("fence", 2).await);
    let t = std::sync::Arc::new(tuning());
    let reached = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    *w.fake.hold.lock().unwrap() = Some((w.slug(0), reached.clone(), release.clone()));
    let (w2, t2) = (w.clone(), t.clone());
    let first = tokio::spawn(async move {
        talaria_hermes_channel::step_with(&w2.pg, &w2.key, &w2.fake, &t2).await
    });
    reached.notified().await;
    // The lock's backend goes away while the step is mid-roll.
    let killed: Vec<bool> = sqlx::query_scalar(
        "select pg_terminate_backend(pid) from pg_locks where locktype = 'advisory' \
         and granted and objid = (hashtext($1)::bigint & 4294967295)::oid",
    )
    .bind(format!("talaria:hermes-channel:{}", w.key))
    .fetch_all(&w.pg)
    .await
    .unwrap();
    assert_eq!(killed, vec![true], "the step's lock connection");
    release.notify_one();
    let out = first.await.unwrap();
    assert!(
        out.as_ref().is_err_and(|e| e.contains("lock was lost")),
        "{out:?}"
    );
    assert_eq!(w.fake.rolled().len(), 1, "nothing moved after the loss");
    // The next step takes the lock afresh and carries on.
    w.fake.advance(11 * MIN);
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    std::sync::Arc::try_unwrap(w).ok().unwrap().done().await;
}

/// Engine S3's "current at once", without stranding anyone: a Roll back
/// with nothing open moves the agents that follow `current` with no
/// assignment of their own, too.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_roll_back_moves_the_agents_that_only_follow_current() {
    let w = World::new("rbfollow", 2).await;
    let t = tuning();
    w.on_new().await;
    // Neither agent has an assignment of its own: both follow `current`.
    sqlx::query("delete from hermes_agent_images where agent_id = any($1::uuid[])")
        .bind(
            w.fake
                .agents
                .iter()
                .map(|a| a.id.clone())
                .collect::<Vec<_>>(),
        )
        .execute(&w.pg)
        .await
        .unwrap();
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    assert_eq!(
        w.state().await.current.as_deref(),
        Some(w.old.as_str()),
        "current moved at once"
    );
    for _ in 0..3 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    assert_eq!(w.fake.on(&w.slug(0)), w.old);
    assert_eq!(w.fake.on(&w.slug(1)), w.old);
    w.done().await;
}

// ── Round four: the re-reviews' findings, each a test that failed first ─────

/// What must hold once a rollout is over: every agent a person did not stop
/// and the rollout did not leave behind runs, on the image its assignment
/// names.
async fn settled(w: &World) {
    let s = w.state().await;
    let deferred = s
        .rollout
        .as_ref()
        .map(|r| r.deferred.clone())
        .unwrap_or_default();
    let current = s.current.clone().unwrap_or_default();
    let assigned = images::assignments(&w.pg).await.unwrap();
    let stopped = w.fake.stopped.lock().unwrap().clone();
    for a in &w.fake.agents {
        if stopped.contains(&a.slug) || deferred.contains(&a.slug) {
            continue;
        }
        let want = assigned.get(&a.id).cloned().unwrap_or(current.clone());
        assert!(w.fake.running(&a.slug), "{} is down", a.slug);
        assert_eq!(
            w.fake.on(&a.slug),
            want,
            "{} runs another image than its assignment",
            a.slug
        );
    }
}

/// Engine NB1(a): a rollback whose roll of one agent does not land (docker
/// kept the old container) moves it again, and never reports it back.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_rollback_whose_roll_does_not_land_moves_the_agent_again() {
    let w = World::new("rbland", 2).await;
    let t = tuning();
    w.on_new().await;
    w.fake
        .roll_effect
        .lock()
        .unwrap()
        .insert(w.slug(0), VecDeque::from([RollEffect::KeepOld]));
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    for _ in 0..6 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    let r = w.state().await.rollout.unwrap();
    assert_eq!(
        (r.kind, r.phase),
        (RolloutKind::Rollback, Phase::Done),
        "{:?}",
        r.detail
    );
    assert_eq!(
        w.fake.on(&w.slug(0)),
        w.old,
        "never left on the banned image"
    );
    settled(&w).await;
    w.done().await;
}

/// Engine NB1(b): a put-back that fails leaves the agent on the new image;
/// when that image is then rolled back, the agent goes back too.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_whose_put_back_failed_is_moved_by_the_rollback() {
    let w = World::new("putfail", 2).await;
    let t = tuning();
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    // It crashes; the put-back's own roll does not land.
    w.fake
        .roll_effect
        .lock()
        .unwrap()
        .insert(w.slug(0), VecDeque::from([RollEffect::KeepOld]));
    w.fake.advance(MIN);
    w.fake.crash(&w.slug(0));
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new, "still on the new image");
    // An admin rolls the release back.
    talaria_hermes_channel::roll_back_with(&w.engine(&t), "admin@test")
        .await
        .unwrap();
    for _ in 0..4 {
        w.fake.advance(16 * MIN);
        w.step(&t).await;
    }
    assert_eq!(
        w.fake.on(&w.slug(0)),
        w.old,
        "the rollback found it on the banned image"
    );
    settled(&w).await;
    w.done().await;
}

/// Engine NB1(c): an agent the engine itself left down (no Stop recorded)
/// is started again, never read as stopped by a person.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_the_engine_left_down_is_started_again() {
    let w = World::new("leftdown", 1).await;
    let t = tuning();
    // The move and its put-back both leave the agent down.
    w.fake.roll_effect.lock().unwrap().insert(
        w.slug(0),
        VecDeque::from([RollEffect::LeaveDown, RollEffect::LeaveDown]),
    );
    for _ in 0..6 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    assert!(w.fake.running(&w.slug(0)), "started again");
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    w.done().await;
}

/// Engine NB2: a catch-up agent that fails on the fleet's image every time
/// is moved once, then left alone until an admin acts.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_catchup_agent_that_keeps_failing_is_moved_once_then_left_alone() {
    let w = World::new("catchloop", 2).await;
    let t = tuning();
    w.on_new().await;
    w.put(1, &w.old).await;
    // It fails its first health question on the fleet's image, every time.
    w.fake.answers_script.lock().unwrap().insert(
        w.slug(1),
        (0..50)
            .map(|_| Err("its API server answered 500 to /v1/models".to_string()))
            .collect(),
    );
    // A night of steps.
    for _ in 0..60 {
        w.step(&t).await;
        w.fake.advance(3 * MIN);
    }
    let onto_new = |w: &World| {
        w.fake
            .rolled()
            .iter()
            .filter(|(s, d)| s == &w.slug(1) && d == &w.new)
            .count()
    };
    assert_eq!(
        onto_new(&w),
        1,
        "moved once, not every step: {:?}",
        w.fake.rolled()
    );
    assert_eq!(w.fake.notes_like("failed on v9.0.2"), 1);
    // An admin's Roll now tries it again.
    talaria_hermes_channel::roll_now_with(&w.engine(&t))
        .await
        .ok();
    for _ in 0..3 {
        w.step(&t).await;
        w.fake.advance(3 * MIN);
    }
    assert_eq!(onto_new(&w), 2);
    w.done().await;
}

/// Surface B1: an agent whose Hermes does not say whether it is working is
/// not restarted on its own, and is on Roll now.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_whose_work_cannot_be_read_moves_on_roll_now() {
    let w = World::new("unknown", 1).await;
    let t = tuning();
    w.fake.unknown.lock().unwrap().insert(
        w.slug(0),
        "whether it is working could not be read (/health/detailed: no answer)".into(),
    );
    for _ in 0..3 {
        w.step(&t).await;
        w.fake.advance(11 * MIN);
    }
    assert!(w.fake.rolled().is_empty(), "not on its own");
    talaria_hermes_channel::roll_now_with(&w.engine(&t))
        .await
        .unwrap();
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new, "Roll now restarts it anyway");
    w.done().await;
}

/// Surface B1: an agent left behind keeps being said, every week.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_left_behind_is_said_again_every_week() {
    let w = World::new("weekly", 2).await;
    let t = tuning();
    w.fake
        .busy
        .lock()
        .unwrap()
        .insert(w.slug(1), "its scheduled job \"sweep\" is running".into());
    for _ in 0..12 {
        w.fake.window.store(true, Ordering::SeqCst);
        for _ in 0..3 {
            w.step(&t).await;
            w.fake.advance(11 * MIN);
        }
        w.fake.window.store(false, Ordering::SeqCst);
        for _ in 0..3 {
            w.step(&t).await;
            w.fake.advance(16 * MIN);
        }
        w.fake.advance(20 * 60 * MIN);
    }
    assert_eq!(
        w.fake.notes_like("left behind"),
        2,
        "night 3 and a week later: {:?}",
        w.fake.notes.lock().unwrap()
    );
    w.done().await;
}

/// Engine S1: a pin to a known-good version stops at its first agent that
/// will not come up healthy, and Roll now goes on.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_pin_stops_at_its_first_unhealthy_agent() {
    let w = World::new("pinhalt", 2).await;
    let t = tuning();
    w.on_new().await;
    w.fake.roll_script.lock().unwrap().insert(
        w.slug(0),
        VecDeque::from([Err(Fault::Image(
            "the recreated container stopped running before it reported healthy".into(),
        ))]),
    );
    talaria_hermes_channel::pin_with(&w.engine(&t), Some(&w.old), None, "admin@test")
        .await
        .unwrap();
    for _ in 0..3 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    assert_eq!(w.fake.on(&w.slug(1)), w.new, "nobody else is moved onto it");
    assert!(w.state().await.rollout.unwrap().halted.is_some());
    talaria_hermes_channel::roll_now_with(&w.engine(&t))
        .await
        .unwrap();
    for _ in 0..2 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    assert_eq!(w.fake.on(&w.slug(1)), w.old, "Roll now goes on");
    w.done().await;
}

/// Engine S3: a reboot during the soak is the host's, never the image's.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_reboot_during_the_soak_is_not_the_images_failure() {
    let w = World::new("reboot", 1).await;
    let t = tuning();
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new);
    w.fake.advance(2 * MIN);
    w.fake.daemon_restart(&w.slug(0));
    w.step(&t).await;
    w.fake.advance(2 * MIN);
    w.step(&t).await;
    let r = w.state().await.rollout.unwrap();
    assert_eq!((r.strikes, r.image_failures), (0, 0), "{:?}", r.detail);
    w.done().await;
}

/// Engine S4: an image no agent ran is not called good.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_image_no_agent_ran_is_not_called_good() {
    let w = World::new("unran", 1).await;
    let t = tuning();
    w.fake.stop(&w.slug(0));
    for _ in 0..4 {
        w.step(&t).await;
        w.fake.advance(16 * MIN);
    }
    let s = w.state().await;
    assert_eq!(s.rollout.unwrap().phase, Phase::Done);
    assert_eq!(
        w.status_of(&w.new).await,
        Status::Candidate,
        "nobody ran it"
    );
    w.done().await;
}

/// Engine N6: an image that fails something new each time is banned on its
/// third failure.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn three_failures_of_different_things_ban() {
    let w = World::new("three", 2).await;
    let t = tuning();
    w.fake
        .canary_script
        .lock()
        .unwrap()
        .push_back(Err(Fault::Image(
            "the canary exited before it reported healthy".into(),
        )));
    let crash = || {
        Err(Fault::Image(
            "the recreated container stopped running before it reported healthy".into(),
        ))
    };
    w.fake
        .roll_script
        .lock()
        .unwrap()
        .insert(w.slug(0), VecDeque::from([crash(), Ok(())]));
    w.fake
        .roll_script
        .lock()
        .unwrap()
        .insert(w.slug(1), VecDeque::from([crash()]));
    for _ in 0..12 {
        w.step(&t).await;
        w.fake.advance(11 * MIN);
    }
    assert_eq!(w.status_of(&w.new).await, Status::Bad);
    w.done().await;
}

/// Surface S1: an Accept covers the problems the admin was shown; one the
/// canary finds afterwards holds the release again.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_accepted_release_is_held_again_for_a_problem_nobody_saw() {
    let w = World::new("accept", 1).await;
    let t = tuning();
    sqlx::query(
        "update hermes_images set inspect = inspect || jsonb_build_object('contract', \
           jsonb_build_object('problems', jsonb_build_array('the schema moved'))) where digest = $1",
    )
    .bind(&w.new)
    .execute(&w.pg)
    .await
    .unwrap();
    talaria_hermes_channel::accept_with(&w.engine(&t), &w.new, "admin@test")
        .await
        .unwrap();
    w.fake
        .canary_script
        .lock()
        .unwrap()
        .push_back(Err(Fault::Held(vec!["/api/jobs answered 404".into()])));
    for _ in 0..3 {
        w.step(&t).await;
        w.fake.advance(11 * MIN);
    }
    assert!(w.fake.rolled().is_empty(), "held again");
    let rec = images::record(&w.pg, &w.new).await.unwrap().unwrap();
    assert!(rec.held());
    w.done().await;
}

/// Engine S2: an Upgrade that gives way after moving agents takes them back
/// to the fleet's image first, banning nothing.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_upgrade_that_gives_way_takes_the_agents_it_moved_back() {
    let w = World::new("withdrawn", 2).await;
    let mut t = tuning();
    w.step(&t).await;
    assert_eq!(w.fake.on(&w.slug(0)), w.new, "the first agent moved");
    // Hermes withdraws it from stable for a hotfix mid-rollout; the hotfix
    // waits its 48 h.
    t.min_stable_ms = 48 * 60 * MIN;
    w.fake.advance(2 * MIN);
    let hotfix = w.extra(2, "v9.0.3", false).await;
    w.stable_is(&hotfix, "v9.0.3").await;
    w.step(&t).await;
    for _ in 0..3 {
        w.fake.advance(MIN);
        w.step(&t).await;
    }
    assert_eq!(w.fake.on(&w.slug(0)), w.old, "back on the fleet's image");
    assert_eq!(w.fake.on(&w.slug(1)), w.old);
    assert_eq!(
        w.status_of(&w.new).await,
        Status::Candidate,
        "giving way bans nothing"
    );
    let r = w.state().await.rollout.unwrap();
    assert_eq!(
        (r.kind, r.digest.clone()),
        (RolloutKind::Upgrade, hotfix),
        "then the hotfix waits"
    );
    settled(&w).await;
    w.done().await;
}

/// Surface B1, on docker's classic image store (an image's id is not its
/// digest): an agent on an inspected image is read strictly, one on an image
/// no record describes leniently.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_running_image_is_found_by_its_docker_id_on_either_store() {
    for classic in [false, true] {
        let w = World::new(if classic { "idclassic" } else { "idcontainerd" }, 2).await;
        let t = tuning();
        w.fake.classic.store(classic, Ordering::SeqCst);
        // The fleet's image passed the contract check; agent 1 runs one no
        // record describes (a pre-channel :latest).
        sqlx::query(
            "update hermes_images set inspect = inspect || '{\"contract\": {\"problems\": []}}'::jsonb where digest = $1",
        )
        .bind(&w.old)
        .execute(&w.pg)
        .await
        .unwrap();
        w.fake.runs(&w.slug(1), &digest(&w.tag, 9));
        for _ in 0..4 {
            w.step(&t).await;
            w.fake.advance(11 * MIN);
        }
        let asked = w.fake.asked.lock().unwrap().clone();
        let first = |slug: &str| asked.iter().find(|(s, _)| s == slug).map(|(_, l)| *l);
        assert_eq!(
            first(&w.slug(0)),
            Some(false),
            "classic {classic}: inspected, read strictly: {asked:?}"
        );
        assert_eq!(
            first(&w.slug(1)),
            Some(true),
            "classic {classic}: never inspected, read leniently: {asked:?}"
        );
        w.done().await;
    }
}

/// Surface B1: a host upgraded from a release before the channel — agents on
/// the `:latest` that release left, no channel state but the first check —
/// moves them onto the fleet's image first, then follows stable.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_host_upgraded_from_main_moves_onto_the_pin_first_then_follows_stable() {
    let w = World::new("frommain", 2).await;
    let t = tuning();
    let latest = digest(&w.tag, 9);
    w.fake.runs(&w.slug(0), &latest);
    w.fake.runs(&w.slug(1), &latest);
    for _ in 0..12 {
        w.step(&t).await;
        w.fake.advance(11 * MIN);
    }
    let rolled = w.fake.rolled();
    assert_eq!(
        rolled,
        vec![
            (w.slug(0), w.old.clone()),
            (w.slug(1), w.old.clone()),
            (w.slug(0), w.new.clone()),
            (w.slug(1), w.new.clone()),
        ],
        "the pin first, then stable"
    );
    assert_eq!(w.status_of(&w.new).await, Status::Good);
    settled(&w).await;
    w.done().await;
}

/// Surface B1: an agent on the pre-channel `:latest` that is busy every
/// night is left for the next rollout, and the rest of the fleet still
/// follows stable.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_busy_every_night_does_not_hold_the_fleet_off_stable() {
    let w = World::new("busydrift", 2).await;
    let t = tuning();
    let latest = digest(&w.tag, 9);
    w.fake.runs(&w.slug(0), &latest);
    w.fake
        .busy
        .lock()
        .unwrap()
        .insert(w.slug(0), "its scheduled job \"sweep\" is running".into());
    for _ in 0..3 {
        w.fake.window.store(true, Ordering::SeqCst);
        for _ in 0..4 {
            w.step(&t).await;
            w.fake.advance(11 * MIN);
        }
        w.fake.window.store(false, Ordering::SeqCst);
        for _ in 0..3 {
            w.step(&t).await;
            w.fake.advance(16 * MIN);
        }
        w.fake.advance(20 * 60 * MIN);
    }
    assert_eq!(
        w.fake.on(&w.slug(1)),
        w.new,
        "the rest follows stable: {:?}",
        w.fake.rolled()
    );
    assert_eq!(w.fake.on(&w.slug(0)), latest, "the busy one waits");
    w.done().await;
}

/// Surface S3: a problem the canary finds on every step is listed once.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_contract_problem_found_again_is_listed_once() {
    let w = World::new("dedupe", 1).await;
    let p = vec!["/api/jobs answered 404".to_string()];
    for _ in 0..3 {
        images::note_contract(&w.pg, &w.new, &p).await.unwrap();
    }
    images::note_contract(
        &w.pg,
        &w.new,
        &["/health/detailed answered 500".to_string()],
    )
    .await
    .unwrap();
    let rec = images::record(&w.pg, &w.new).await.unwrap().unwrap();
    let mut got = rec.contract_problems();
    got.sort();
    assert_eq!(
        got,
        vec!["/api/jobs answered 404", "/health/detailed answered 500"]
    );
    w.done().await;
}
