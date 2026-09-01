// The lease — the one mechanism in this tree for "exactly one process is
// doing this right now", and the two POLICIES built on it. Port of
// ui/src/server/runs/lease.ts, whole: the policies are genuinely different
// and forcing one on both would break the scheduler.
//
// THE PRIMITIVE, and why every piece is load-bearing:
// · SET NX PX      take the key only if nobody has it, and never hold it
//                  forever. A lease with no TTL is a deadlock waiting for the
//                  one process that can clear it to be the one that crashed.
// · a TOKEN        the value under the key identifies the holder, and every
//                  subsequent operation is a COMPARE-AND-SET against it.
//                  Without it, a process whose lease expired mid-work would
//                  go on renewing (and eventually deleting) a lease another
//                  instance now legitimately holds — worse than never having
//                  leased at all, because both believe they are alone.
// · minted HERE    acquire returns the token; callers never supply one.
//                  "Only the taker may renew or release" is then a fact about
//                  the API, and a caller cannot pass a stale token from a
//                  previous attempt.
//
// TWO POLICIES OVER ONE PRIMITIVE. Do not unify them; the difference is the
// point. THE SCHEDULER leases a named job for a PERIOD and deliberately holds
// the key past completion (`demote_lease`): a mutex only stops two instances
// running the job at the same moment, and would still let instance B run
// comms decay a minute after instance A finished — twice per interval. The
// key is the period's receipt, not a mutex. A RUN leases a row for ONE STEP
// and releases immediately: the next step should be claimable by ANY
// instance, and holding it would pin a long run to whichever instance
// started it. And the expiries mean opposite things: a scheduler lease
// expiring un-demoted means the run died and the job is due again; a RUN
// LEASE EXPIRING IS NOT A FAILURE — IT IS A RECLAIM SIGNAL. The row still
// holds its last persisted checkpoint and another instance should pick it up
// from there. That is why nothing in here marks anything failed.
//
// REDIS UNREACHABLE is decided per policy because "fail closed" spells
// differently for a job and for a run. The scheduler SKIPS the tick (a lost
// period is a missed hour of archiving; running unguarded risks a duplicate
// proactive DM, which is unrecoverable). A run DEFERS: it must not proceed
// unleased — two instances stepping one run is the one failure mode a
// checkpoint cannot recover from — but it must not be marked failed either.
// The cautionary tale is research.ts turning "the app restarted mid-run"
// into `error: "run went stale"`, destroying a run that had lost nothing.
// Hence `Unavailable`/`Blocked` rather than an error: a caller that ignores
// the distinction gets no lease, and a caller that reads it knows the
// difference between "someone else has this" and "I could not ask".
//
// TESTABILITY IS A DESIGN CONSTRAINT. Every edge to the outside world is the
// `LeaseBackend` trait (the Redis commands the lease needs, structurally, so
// a test drives a fake with no Redis server) plus a manually-advanceable
// clock in the fake — no real timers, no real expiry sleeps.

use futures_util::future::BoxFuture;
use redis::Script;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// The slice of a Redis client a lease needs — the trait exists so tests run
/// with no Redis server. The real backend speaks the same Lua the TS side
/// does, so live keys are shared correctly across runtimes.
pub trait LeaseBackend: Send {
    /// `SET key value NX PX ttl` — true when the key was taken.
    fn set_nx_px<'a>(
        &'a mut self,
        key: &'a str,
        value: &'a str,
        ttl_ms: u64,
    ) -> BoxFuture<'a, Result<bool, String>>;
    /// `PEXPIRE` only while the current value matches — the renew/demote CAS.
    fn pexpire_if_eq<'a>(
        &'a mut self,
        key: &'a str,
        value: &'a str,
        ttl_ms: u64,
    ) -> BoxFuture<'a, Result<bool, String>>;
    /// `DEL` only while the current value matches — the release CAS. An
    /// unconditional DEL from a process whose lease already expired would
    /// delete the lease another instance is working under.
    fn del_if_eq<'a>(
        &'a mut self,
        key: &'a str,
        value: &'a str,
    ) -> BoxFuture<'a, Result<bool, String>>;
    fn get<'a>(&'a mut self, key: &'a str) -> BoxFuture<'a, Result<Option<String>, String>>;
}

/// The production backend: a cloned-able shared connection over the same
/// Redis the TS process talks to, running the exact TS Lua for the CASes.
pub struct RedisLeases {
    conn: redis::aio::ConnectionManager,
}

impl RedisLeases {
    pub fn new(conn: redis::aio::ConnectionManager) -> Self {
        Self { conn }
    }
}

/// Extend (or shorten) a lease we still hold. Returns 1 when the expiry was
/// set, 0 when the value was no longer ours.
const CAS_PEXPIRE: &str = "
if redis.call('get', KEYS[1]) == ARGV[1] then
  return redis.call('pexpire', KEYS[1], ARGV[2])
end
return 0
";

/// Give up a lease we still hold, with the same check.
const CAS_DEL: &str = "
if redis.call('get', KEYS[1]) == ARGV[1] then
  return redis.call('del', KEYS[1])
end
return 0
";

fn cas_pexpire() -> &'static Script {
    static S: OnceLock<Script> = OnceLock::new();
    S.get_or_init(|| Script::new(CAS_PEXPIRE))
}

fn cas_del() -> &'static Script {
    static S: OnceLock<Script> = OnceLock::new();
    S.get_or_init(|| Script::new(CAS_DEL))
}

impl LeaseBackend for RedisLeases {
    fn set_nx_px<'a>(
        &'a mut self,
        key: &'a str,
        value: &'a str,
        ttl_ms: u64,
    ) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            let res: Option<String> = redis::cmd("SET")
                .arg(key)
                .arg(value)
                .arg("NX")
                .arg("PX")
                .arg(clamp_ttl(ttl_ms))
                .query_async(&mut self.conn)
                .await
                .map_err(|e| e.to_string())?;
            Ok(res.is_some())
        })
    }

    fn pexpire_if_eq<'a>(
        &'a mut self,
        key: &'a str,
        value: &'a str,
        ttl_ms: u64,
    ) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            let n: i64 = cas_pexpire()
                .key(key)
                .arg(value)
                .arg(clamp_ttl(ttl_ms))
                .invoke_async(&mut self.conn)
                .await
                .map_err(|e| e.to_string())?;
            Ok(n == 1)
        })
    }

    fn del_if_eq<'a>(
        &'a mut self,
        key: &'a str,
        value: &'a str,
    ) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            let n: i64 = cas_del()
                .key(key)
                .arg(value)
                .invoke_async(&mut self.conn)
                .await
                .map_err(|e| e.to_string())?;
            Ok(n == 1)
        })
    }

    fn get<'a>(&'a mut self, key: &'a str) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            redis::cmd("GET")
                .arg(key)
                .query_async(&mut self.conn)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

// ── Tokens and keys ───────────────────────────────────────────────────────────

/// Identifies THIS process in every lease value it writes. The uuid suffix on
/// each token makes a token unique per ATTEMPT; this prefix is what lets
/// `lease_holder` answer "that is my own lease" — the scheduler's normal case,
/// where the key it failed to take is the receipt from its own run earlier
/// this interval. Pids recycle after a restart, so the random half is not
/// decoration.
pub fn instance_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        format!(
            "{}-{}",
            std::process::id(),
            &Uuid::new_v4().simple().to_string()[..8]
        )
    })
}

fn new_token() -> String {
    format!("{}:{}", instance_id(), Uuid::new_v4())
}

/// The one key format. `namespace` separates the policies that share this
/// primitive ('sched' for a named job's period, 'run' for one run row) and
/// `v1` is there so a future change to what the value means can be made
/// without a fleet mid-deploy reading two meanings out of one key.
pub fn lease_key(namespace: &str, name: &str) -> String {
    format!("talaria:{namespace}:v1:{name}")
}

/// A claim on a key, held by THIS process. The value is opaque to callers: it
/// is the compare-and-set operand and the only proof of ownership there is.
#[derive(Debug, Clone)]
pub struct LeaseToken {
    pub key: String,
    pub value: String,
}

// ── Results ───────────────────────────────────────────────────────────────────

pub enum AcquireResult {
    Acquired(LeaseToken),
    /// Someone holds it — possibly us, from earlier. Ask `lease_holder` if the
    /// distinction matters; it costs a round trip, which is why it is not
    /// folded in here.
    Held,
    /// Redis could not be asked. NOT a failure of the leased work — the caller
    /// decides whether that means skip (scheduler) or defer (runs); it never
    /// means "mark it broken".
    Unavailable(String),
}

pub enum LeaseResult {
    Ok,
    /// The compare-and-set found a different value (or none): the lease
    /// expired and may already belong to someone else. For a job that means
    /// the run overran its TTL; for a run it means another instance has
    /// reclaimed the row and THIS process must stop writing to it.
    Lost,
    Unavailable(String),
}

/// Whether the holder of a lease we failed to take is us. Null (None) when
/// the key is gone or the answer could not be read — this is diagnostics, and
/// it must never be the reason anything fails.
pub enum LeaseHolder {
    SelfHeld,
    Other,
}

// ── The primitive ─────────────────────────────────────────────────────────────

/// A PX of 0 or less is an error to Redis; clamped here, once, rather than at
/// four call sites. (TS also floors fractional ms; u64 has no fraction.)
pub fn clamp_ttl(ms: u64) -> u64 {
    ms.max(1)
}

/// Take `key` for `ttl_ms`, or report why not. Never fails.
pub async fn acquire_lease(
    backend: &mut dyn LeaseBackend,
    key: &str,
    ttl_ms: u64,
) -> AcquireResult {
    let value = new_token();
    match backend.set_nx_px(key, &value, ttl_ms).await {
        Ok(true) => AcquireResult::Acquired(LeaseToken {
            key: key.to_string(),
            value,
        }),
        Ok(false) => AcquireResult::Held,
        Err(error) => AcquireResult::Unavailable(error),
    }
}

pub async fn lease_holder(backend: &mut dyn LeaseBackend, key: &str) -> Option<LeaseHolder> {
    match backend.get(key).await {
        Ok(Some(value)) => {
            let prefix = format!("{}:", instance_id());
            Some(if value.starts_with(&prefix) {
                LeaseHolder::SelfHeld
            } else {
                LeaseHolder::Other
            })
        }
        _ => None,
    }
}

async fn cas_pexpire_call(
    backend: &mut dyn LeaseBackend,
    token: &LeaseToken,
    ttl_ms: u64,
) -> LeaseResult {
    match backend
        .pexpire_if_eq(&token.key, &token.value, ttl_ms)
        .await
    {
        Ok(true) => LeaseResult::Ok,
        Ok(false) => LeaseResult::Lost,
        Err(error) => LeaseResult::Unavailable(error),
    }
}

/// Keep a lease we hold alive for another `ttl_ms`, from now.
///
/// `renew_lease` and `demote_lease` are the SAME Redis operation and the
/// intent is the entire difference between them — which is exactly why they
/// have two names. Renewing says "I am still working"; demoting says "I am
/// done and this period is spent". A reader of a call site should not have to
/// work out which one a bare pexpire meant.
pub async fn renew_lease(
    backend: &mut dyn LeaseBackend,
    token: &LeaseToken,
    ttl_ms: u64,
) -> LeaseResult {
    cas_pexpire_call(backend, token, ttl_ms).await
}

/// Hold a lease PAST the work it protected, for `hold_ms`, and stop renewing
/// it. The scheduler's policy and nothing else's: it is what turns "not at
/// the same moment" into "once per interval, fleet-wide". A run must not do
/// this — the next step belongs to whichever instance is free.
pub async fn demote_lease(
    backend: &mut dyn LeaseBackend,
    token: &LeaseToken,
    hold_ms: u64,
) -> LeaseResult {
    cas_pexpire_call(backend, token, hold_ms).await
}

/// Drop a lease we hold so the next claimant can have it immediately.
///
/// `Lost` here is not an error to report loudly — it means the lease had
/// already expired and someone else is holding the key, and deleting it is
/// precisely what we must not do.
pub async fn release_lease(backend: &mut dyn LeaseBackend, token: &LeaseToken) -> LeaseResult {
    match backend.del_if_eq(&token.key, &token.value).await {
        Ok(true) => LeaseResult::Ok,
        Ok(false) => LeaseResult::Lost,
        Err(error) => LeaseResult::Unavailable(error),
    }
}

// ── The heartbeat ─────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct HeartbeatOptions {
    /// How often to renew. Defaults to a THIRD of the TTL, so a single failed
    /// renewal — a blip, a failover — is survivable rather than fatal: there
    /// are two more attempts before the lease actually lapses.
    pub every_ms: Option<u64>,
    /// The compare-and-set failed: this process no longer holds the lease and
    /// another instance may already be doing the work. Nothing here stops the
    /// work — the caller owns that decision, and for a run it is a serious
    /// one (stop persisting; the row is not yours any more).
    pub on_lost: Option<LostCallback>,
    /// Redis could not be reached for a renewal. Distinct from `on_lost`
    /// because the lease may well still be ours — we simply could not say so.
    pub on_error: Option<ErrorCallback>,
}

/// The heartbeat's callbacks, named so `HeartbeatOptions` stays readable.
pub type LostCallback = Arc<dyn Fn() + Send + Sync>;
pub type ErrorCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// The heartbeat's interval: a third of the (clamped) TTL, never under a
/// second.
pub fn heartbeat_every_ms(ttl_ms: u64, every_ms: Option<u64>) -> u64 {
    every_ms.unwrap_or(clamp_ttl(ttl_ms) / 3).max(1_000)
}

/// One renewal beat, as a function — the unit the tests drive by hand instead
/// of racing a real interval. Applies the callbacks exactly as the loop would.
pub async fn renewal_beat(
    backend: &mut dyn LeaseBackend,
    token: &LeaseToken,
    ttl_ms: u64,
    opts: &HeartbeatOptions,
) {
    match renew_lease(backend, token, ttl_ms).await {
        LeaseResult::Ok => {}
        LeaseResult::Lost => {
            if let Some(on_lost) = &opts.on_lost {
                on_lost();
            }
        }
        LeaseResult::Unavailable(error) => {
            if let Some(on_error) = &opts.on_error {
                on_error(&error);
            }
        }
    }
}

/// A running renewal loop. `stop()` consumes the guard and is required on
/// every exit path from the leased work, including the failing ones.
pub struct LeaseHeartbeat {
    task: Option<JoinHandle<()>>,
}

impl LeaseHeartbeat {
    pub fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Renew a lease in the background while the work it protects runs, so work
/// that legitimately takes longer than one TTL is not stolen mid-flight. The
/// TTL stays SHORT and is renewed rather than being set generously up front:
/// the TTL is also how long a CRASHED holder's lease blocks everyone else,
/// and those two pressures pull in opposite directions. Renewal is what lets
/// both win.
///
/// Takes the real connection (it spawns); the beat logic it loops over is
/// `renewal_beat`, which is what the tests drive.
pub fn keep_lease_alive(
    conn: redis::aio::ConnectionManager,
    token: LeaseToken,
    ttl_ms: u64,
    opts: HeartbeatOptions,
) -> LeaseHeartbeat {
    let every = Duration::from_millis(heartbeat_every_ms(ttl_ms, opts.every_ms));
    let task = tokio::spawn(async move {
        let mut backend = RedisLeases::new(conn);
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            renewal_beat(&mut backend, &token, ttl_ms, &opts).await;
        }
    });
    LeaseHeartbeat { task: Some(task) }
}

// ── The run policy ────────────────────────────────────────────────────────────
//
// A thin, deliberately boring layer over the primitive, so the run runtime
// never spells the policy out by hand: one step's duration, released
// immediately, an expiry that means reclaim rather than failure.

/// The namespace runs lease under. Separate from 'sched' so a run whose id
/// ever collided with a job name could not take the job's lease.
pub const RUN_LEASE_NS: &str = "run";

pub fn run_lease_key(run_id: &str) -> String {
    lease_key(RUN_LEASE_NS, run_id)
}

pub enum RunClaim {
    /// This process may step the run. Release it when the step ends — on
    /// EVERY path, including the failing ones — or the run sits idle until
    /// the TTL lapses.
    Claimed(LeaseToken),
    /// Another instance is stepping this run right now. Not an error and not
    /// a reason to touch the row: come back later, or move on to another run.
    Busy,
    /// Redis could not be asked, so this process cannot know whether it is
    /// alone. LEAVE THE ROW ALONE — do not step it, and above all do not mark
    /// it failed. The checkpoint is durable and a later sweep will claim it.
    Blocked(String),
}

/// Claim a run for ONE STEP.
///
/// `step_ms` is the step's declared outside bound (`RunDefinition.max_step_ms`),
/// not the run's. That is the whole difference from the scheduler, and it is
/// what makes a crashed instance cheap: the row becomes claimable again
/// roughly one step after the process holding it died, rather than one run.
///
/// AT-LEAST-ONCE LIVES HERE. This lease stops two instances stepping a run at
/// the same time; it does NOT stop a step that ran and died before persisting
/// its checkpoint from running again when someone reclaims the run. Nothing
/// can, short of the step itself being idempotent — so persist the checkpoint
/// BEFORE the side effect wherever the ordering allows, and where it does
/// not, say so at the step.
pub async fn acquire_run_lease(
    backend: &mut dyn LeaseBackend,
    run_id: &str,
    step_ms: u64,
) -> RunClaim {
    match acquire_lease(backend, &run_lease_key(run_id), step_ms).await {
        AcquireResult::Acquired(token) => RunClaim::Claimed(token),
        AcquireResult::Held => RunClaim::Busy,
        AcquireResult::Unavailable(error) => RunClaim::Blocked(error),
    }
}

/// Hand the run back the moment the step is done, so ANY instance can take
/// the next one. Deliberately a delete and not a demote: there is no "this
/// period is spent" for a run, and holding the key would pin the run to this
/// process.
pub async fn release_run_lease(backend: &mut dyn LeaseBackend, claim: &LeaseToken) -> LeaseResult {
    release_lease(backend, claim).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An in-memory Redis with real expiry semantics on a manually
    /// advanceable clock — the whole primitive is testable with no server
    /// and no sleeping.
    struct FakeRedis {
        now_ms: u64,
        map: HashMap<String, (String, u64)>, // value, expires_at
        fail: bool,
    }

    impl FakeRedis {
        fn new() -> Self {
            Self {
                now_ms: 10_000,
                map: HashMap::new(),
                fail: false,
            }
        }
        fn advance(&mut self, ms: u64) {
            self.now_ms += ms;
        }
        fn live(&self, key: &str) -> Option<&(String, u64)> {
            self.map.get(key).filter(|(_, exp)| *exp > self.now_ms)
        }
        fn raw(&self, key: &str) -> Option<&String> {
            self.map.get(key).map(|(v, _)| v)
        }
    }

    impl LeaseBackend for FakeRedis {
        fn set_nx_px<'a>(
            &'a mut self,
            key: &'a str,
            value: &'a str,
            ttl_ms: u64,
        ) -> BoxFuture<'a, Result<bool, String>> {
            Box::pin(async move {
                if self.fail {
                    return Err("connection dropped".into());
                }
                if self.live(key).is_some() {
                    return Ok(false);
                }
                self.map.insert(
                    key.to_string(),
                    (value.to_string(), self.now_ms + clamp_ttl(ttl_ms)),
                );
                Ok(true)
            })
        }

        fn pexpire_if_eq<'a>(
            &'a mut self,
            key: &'a str,
            value: &'a str,
            ttl_ms: u64,
        ) -> BoxFuture<'a, Result<bool, String>> {
            Box::pin(async move {
                if self.fail {
                    return Err("connection dropped".into());
                }
                match self.live(key) {
                    Some((held, _)) if held == value => {
                        let exp = self.now_ms + clamp_ttl(ttl_ms);
                        self.map.get_mut(key).unwrap().1 = exp;
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            })
        }

        fn del_if_eq<'a>(
            &'a mut self,
            key: &'a str,
            value: &'a str,
        ) -> BoxFuture<'a, Result<bool, String>> {
            Box::pin(async move {
                if self.fail {
                    return Err("connection dropped".into());
                }
                match self.live(key) {
                    Some((held, _)) if held == value => {
                        self.map.remove(key);
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            })
        }

        fn get<'a>(&'a mut self, key: &'a str) -> BoxFuture<'a, Result<Option<String>, String>> {
            Box::pin(async move {
                if self.fail {
                    return Err("connection dropped".into());
                }
                Ok(self.live(key).map(|(v, _)| v.clone()))
            })
        }
    }

    #[tokio::test]
    async fn acquisition_take_hold_expire() {
        let mut f = FakeRedis::new();
        let first = acquire_lease(&mut f, "talaria:sched:v1:comms-decay", 60_000).await;
        let AcquireResult::Acquired(token) = first else {
            panic!("free key must be acquirable");
        };
        // The token names the process and is unique per attempt.
        assert!(token.value.starts_with(&format!("{}:", instance_id())));
        let second = new_token();
        assert_ne!(token.value, second);
        // Someone holds it (us, from earlier in this test).
        assert!(matches!(
            acquire_lease(&mut f, "talaria:sched:v1:comms-decay", 60_000).await,
            AcquireResult::Held
        ));
        // Expired keys are free again: a run lease expiring is a reclaim
        // signal, not a failure — the next claimant gets the row.
        f.advance(60_001);
        assert!(matches!(
            acquire_lease(&mut f, "talaria:sched:v1:comms-decay", 60_000).await,
            AcquireResult::Acquired(_)
        ));
    }

    #[tokio::test]
    async fn ttl_is_clamped_not_rejected() {
        let mut f = FakeRedis::new();
        // A PX of 0 would be a Redis error; the clamp makes it 1ms.
        assert!(matches!(
            acquire_lease(&mut f, "talaria:run:v1:r1", 0).await,
            AcquireResult::Acquired(_)
        ));
        f.advance(2);
        assert!(f.live("talaria:run:v1:r1").is_none());
    }

    #[tokio::test]
    async fn renew_and_demote_are_the_same_cas() {
        let mut f = FakeRedis::new();
        let AcquireResult::Acquired(token) =
            acquire_lease(&mut f, "talaria:sched:v1:j", 30_000).await
        else {
            panic!()
        };
        // Renew from mid-work: still ours, expiry pushed out.
        f.advance(10_000);
        assert!(matches!(
            renew_lease(&mut f, &token, 30_000).await,
            LeaseResult::Ok
        ));
        f.advance(29_999);
        assert!(f.live("talaria:sched:v1:j").is_some());
        f.advance(2);
        assert!(f.live("talaria:sched:v1:j").is_none());
        // Demote holds the key PAST the work, for the period's remainder —
        // the scheduler's receipt. Same operation, different intent; the key
        // is not deleted and nobody else can take the period.
        let AcquireResult::Acquired(t2) =
            acquire_lease(&mut f, "talaria:sched:v1:j2", 30_000).await
        else {
            panic!()
        };
        assert!(matches!(
            demote_lease(&mut f, &t2, 120_000).await,
            LeaseResult::Ok
        ));
        assert!(matches!(
            acquire_lease(&mut f, "talaria:sched:v1:j2", 30_000).await,
            AcquireResult::Held
        ));
    }

    #[tokio::test]
    async fn a_lost_lease_is_never_touched_again() {
        let mut f = FakeRedis::new();
        let AcquireResult::Acquired(mine) = acquire_lease(&mut f, "talaria:run:v1:r9", 5_000).await
        else {
            panic!()
        };
        // Our lease lapses; another instance takes the row.
        f.advance(6_000);
        let AcquireResult::Acquired(theirs) =
            acquire_lease(&mut f, "talaria:run:v1:r9", 5_000).await
        else {
            panic!()
        };
        // Renewing and releasing with the stale token both report Lost, and
        // neither disturbs the new holder — that is the compare-and-set doing
        // the only job it has.
        assert!(matches!(
            renew_lease(&mut f, &mine, 5_000).await,
            LeaseResult::Lost
        ));
        assert!(matches!(
            release_lease(&mut f, &mine).await,
            LeaseResult::Lost
        ));
        assert_eq!(f.raw("talaria:run:v1:r9"), Some(&theirs.value));
    }

    #[tokio::test]
    async fn release_hands_the_key_back_immediately() {
        let mut f = FakeRedis::new();
        let RunClaim::Claimed(token) = acquire_run_lease(&mut f, "run-42", 5_000).await else {
            panic!()
        };
        assert!(matches!(
            release_run_lease(&mut f, &token).await,
            LeaseResult::Ok
        ));
        // Deliberately a delete, not a demote: the next step belongs to
        // whichever instance is free.
        assert!(matches!(
            acquire_run_lease(&mut f, "run-42", 5_000).await,
            RunClaim::Claimed(_)
        ));
    }

    #[tokio::test]
    async fn holder_distinguishes_self_other_and_gone() {
        let mut f = FakeRedis::new();
        // Our own receipt from earlier this interval: the scheduler's normal
        // "held" case.
        let AcquireResult::Acquired(mine) =
            acquire_lease(&mut f, "talaria:sched:v1:job", 60_000).await
        else {
            panic!()
        };
        assert!(matches!(
            lease_holder(&mut f, "talaria:sched:v1:job").await,
            Some(LeaseHolder::SelfHeld)
        ));
        // A foreign holder (what a TS-process lease looks like from here).
        f.map.insert(
            "talaria:sched:v1:other".into(),
            ("99999-abcd1234:theirs".into(), f.now_ms + 60_000),
        );
        assert!(matches!(
            lease_holder(&mut f, "talaria:sched:v1:other").await,
            Some(LeaseHolder::Other)
        ));
        // Gone or unreadable is None — diagnostics, never a failure.
        assert!(
            lease_holder(&mut f, "talaria:sched:v1:missing")
                .await
                .is_none()
        );
        assert_eq!(mine.key, "talaria:sched:v1:job");
    }

    #[tokio::test]
    async fn redis_unreachable_reports_its_shape_not_a_panic() {
        let mut f = FakeRedis::new();
        f.fail = true;
        // The scheduler's answer: skip the tick.
        assert!(matches!(
            acquire_lease(&mut f, "talaria:sched:v1:job", 60_000).await,
            AcquireResult::Unavailable(_)
        ));
        // The run's answer: defer, and above all do not mark anything failed.
        assert!(matches!(
            acquire_run_lease(&mut f, "r1", 5_000).await,
            RunClaim::Blocked(_)
        ));
        f.fail = false;
        let AcquireResult::Acquired(token) =
            acquire_lease(&mut f, "talaria:sched:v1:job", 60_000).await
        else {
            panic!()
        };
        f.fail = true;
        assert!(matches!(
            renew_lease(&mut f, &token, 60_000).await,
            LeaseResult::Unavailable(_)
        ));
        assert!(matches!(
            release_lease(&mut f, &token).await,
            LeaseResult::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn the_beat_routes_lost_and_error_to_different_callbacks() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut f = FakeRedis::new();
        let lost = Arc::new(AtomicUsize::new(0));
        let errored = Arc::new(AtomicUsize::new(0));
        let l1 = lost.clone();
        let e1 = errored.clone();
        let opts = HeartbeatOptions {
            every_ms: None,
            on_lost: Some(Arc::new(move || {
                l1.fetch_add(1, Ordering::SeqCst);
            })),
            on_error: Some(Arc::new(move |_| {
                e1.fetch_add(1, Ordering::SeqCst);
            })),
        };
        let AcquireResult::Acquired(token) =
            acquire_lease(&mut f, "talaria:run:v1:beat", 3_000).await
        else {
            panic!()
        };
        // A healthy beat fires nothing.
        renewal_beat(&mut f, &token, 3_000, &opts).await;
        assert_eq!(lost.load(Ordering::SeqCst), 0);
        assert_eq!(errored.load(Ordering::SeqCst), 0);
        // Someone else took the row: on_lost — the sharpest signal in the
        // runtime. Anything written to the row from now on is from a ghost.
        f.advance(4_000);
        let AcquireResult::Acquired(_theirs) =
            acquire_lease(&mut f, "talaria:run:v1:beat", 3_000).await
        else {
            panic!()
        };
        renewal_beat(&mut f, &token, 3_000, &opts).await;
        assert_eq!(lost.load(Ordering::SeqCst), 1);
        // Unreachable is NOT lost — the lease may still be ours.
        f.fail = true;
        renewal_beat(&mut f, &token, 3_000, &opts).await;
        assert_eq!(lost.load(Ordering::SeqCst), 1);
        assert_eq!(errored.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_heartbeat_interval_is_a_third_of_the_ttl_floor_one_second() {
        // A single failed renewal is survivable: two more attempts before
        // the lease actually lapses.
        assert_eq!(heartbeat_every_ms(30_000, None), 10_000);
        assert_eq!(heartbeat_every_ms(2_000, None), 1_000);
        assert_eq!(heartbeat_every_ms(0, None), 1_000);
        assert_eq!(heartbeat_every_ms(30_000, Some(5_000)), 5_000);
    }

    #[test]
    fn key_shapes_pin_the_wire_format() {
        // The same format the TS process spells: a Rust lease and a TS lease
        // contend for the SAME keys during coexistence, so the shape is
        // load-bearing, not cosmetic.
        assert_eq!(
            lease_key("sched", "comms-decay"),
            "talaria:sched:v1:comms-decay"
        );
        assert_eq!(
            run_lease_key("0b2f2f18-6c51-487f"),
            "talaria:run:v1:0b2f2f18-6c51-487f"
        );
    }
}
