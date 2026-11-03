// THE STATE MACHINE — one step of the channel, against any [`FleetOps`].
//
//   preparing → waiting → canary → rolling → soaking → done
//       │          ↑         │         │          │
//       │          └ retry ──┴─────────┴──────────┴──→ failed: a rollback
//       └──→ cancelled: stable moved on (or back) before it moved anyone
//
// THE ROW IS THE TRUTH, AND EVERY WRITE NAMES ITS ROLLOUT. Admin actions
// replace the rollout in the state row under its row lock at any moment (a
// Roll back or a Pin pressed mid-step). Every write the engine makes into a
// rollout is refused once the row holds another rollout (or the rollout
// closed) — [`StateError::Superseded`] — and what belongs with that write (an
// agent's assignment, an image marked good or bad) is written in its
// transaction or not at all. A step that is superseded stops, re-reads, and
// carries on with whatever rollout the row holds now.
//
// WHAT IS LEFT TO MOVE IS READ, NEVER REMEMBERED. An agent is still to move
// while its assignment is not the rollout's image (for a rollback: while it
// is the image being left). There is no list of who moved to drift from
// where the agents are: an agent put back is simply to move again.
//
// WHAT A PERSON SET, STAYS. An agent a person stopped is assigned the image
// and starts on it when a person starts it — never started by the channel.
// One that is down without being stopped (crash-looping, a recreate left
// half done) is recreated whatever it was doing: it cannot be doing anything.
//
// NOTHING NEW IS GOOD UNTIL IT HAS SOAKED. An image not yet known good here
// boots as a canary first and soaks; each agent moved onto it is watched —
// running, healthy, its restart count unchanged since it came up, no new
// config fault in its log — before the next one moves, every watched agent is
// asked again before every move, and once the last has moved the whole
// rollout soaks before the image is called good. Going back to a known good
// image (a rollback, a catch-up, a pin to a version that ran well here) needs
// none of that: its agents are leaving something worse.
//
// ONE FAILURE IS A QUESTION; TWO OF THE SAME THING ARE AN ANSWER. A failure
// of the image (the canary, its preparation, one agent) puts that agent back
// and is retried once to confirm; the same thing failing again marks the
// image bad and rolls back every agent on it. The strike clears when the
// thing passes, so failures of different things never add up. A known-good
// image is never banned: its failures are asked again, and said.
//
// THE HOST'S FAILURES BACK OFF AND ARE SAID ONCE. Docker slow or refusing, a
// pull that fails, a disk without room: tried again after 15 min, then twice
// as long each time to six hours, and the third tells the admins. A
// saturated disk excuses at most two image failures; the next is judged.

use std::time::Duration;

use sqlx::PgPool;

use talaria_hermes_images::{
    self as images, ChannelState, Effects, ImageRecord, Phase, Rollout, RolloutKind, StateError,
    Status, Watched,
};

use crate::ops::{Busy, ChannelAgent, Fault, FleetOps, Snapshot, on_channel};
use crate::prepare::Prepared;

/// The numbers the state machine runs on. [`Tuning::from_env`] is the real
/// one; the tests shrink them.
#[derive(Debug, Clone)]
pub struct Tuning {
    /// A digest must have been named by stable this long (since a check
    /// first saw it there) before an Upgrade downloads and rolls it out on
    /// its own.
    pub min_stable_ms: i64,
    /// The first agent moved is watched this long before the next moves.
    pub first_agent_soak_ms: i64,
    /// Every later agent is watched this long before the next moves.
    pub agent_soak_ms: i64,
    /// Once every agent has moved, the whole rollout is watched this long.
    pub rollout_soak_ms: i64,
    /// After a first image failure, the retry that confirms it waits this.
    pub retry_confirm_ms: i64,
    /// After a first failure that was the host's, the retry waits this;
    /// each further one twice as long, to [`BACKOFF_CAP_MS`].
    pub retry_infra_ms: i64,
    /// A forced rollout (Roll now) leaves an agent that stayed busy this
    /// long since it began rolling for a later window.
    pub forced_defer_ms: i64,
    /// How long one step keeps moving agents.
    pub step_budget: Duration,
}

const MINUTE: i64 = 60 * 1000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// The longest a host failure's retry waits.
pub const BACKOFF_CAP_MS: i64 = 6 * HOUR;
/// The host failure in a row that tells the admins.
pub const INFRA_ALERT_AFTER: u32 = 3;
/// Image failures a saturated disk may excuse in one rollout.
pub const MASKED_MAX: u32 = 2;
/// Windows in a row an agent may be left behind busy before a person hears.
pub const DEFER_ALERT_AFTER: u32 = 3;
/// The longest minimum age on stable the environment may ask for.
const MIN_STABLE_HOURS_MAX: u64 = 30 * 24;

impl Tuning {
    /// The defaults, each overridable from the api's environment
    /// (`TALARIA_HERMES_MIN_STABLE_HOURS`, `TALARIA_HERMES_*_SOAK_SECS`).
    pub fn from_env() -> Tuning {
        let secs = |var: &str, default: i64| {
            crate::env_secs(var, default as u64).min(7 * 24 * 3600) as i64 * 1000
        };
        Tuning {
            min_stable_ms: crate::env_secs("TALARIA_HERMES_MIN_STABLE_HOURS", 48)
                .min(MIN_STABLE_HOURS_MAX) as i64
                * HOUR,
            first_agent_soak_ms: secs("TALARIA_HERMES_FIRST_AGENT_SOAK_SECS", 10 * 60),
            agent_soak_ms: secs("TALARIA_HERMES_AGENT_SOAK_SECS", 3 * 60),
            rollout_soak_ms: secs("TALARIA_HERMES_ROLLOUT_SOAK_SECS", 15 * 60),
            retry_confirm_ms: 5 * MINUTE,
            retry_infra_ms: 15 * MINUTE,
            forced_defer_ms: 3 * HOUR,
            step_budget: Duration::from_secs(12 * 60),
        }
    }
}

/// The wait before the `n`th retry of a host failure: `first`, doubling, to
/// [`BACKOFF_CAP_MS`].
pub fn backoff(first: i64, n: u32) -> i64 {
    let doublings = n.saturating_sub(1).min(20);
    first.saturating_mul(1i64 << doublings).min(BACKOFF_CAP_MS)
}

/// Whether the rollout's image still has to prove itself here (canary,
/// soaks, and a ban if it fails twice): anything not yet known good, unless
/// the rollout is a rollback.
pub fn watching(r: &Rollout, record: &ImageRecord) -> bool {
    r.kind != RolloutKind::Rollback && record.status != Status::Good
}

/// The engine over one state row.
pub struct Engine<'a> {
    pub pg: &'a PgPool,
    /// The `app_settings` key of the state (the channel's own is
    /// `talaria_hermes_images::STATE_KEY`; a test uses its own).
    pub key: &'a str,
    pub ops: &'a dyn FleetOps,
    pub tuning: &'a Tuning,
    /// The step's lock, asked before every write: a step whose lock went
    /// with its database connection stops before it touches another agent.
    /// None for an admin's action (which writes under the row lock alone).
    pub fence: Option<&'a crate::Fence>,
}

/// What one agent's turn came to.
enum Move {
    /// It runs the target now; its watch entry when it is watched.
    Moved(Option<Watched>, String),
    /// Passed over for now, and why.
    Skipped(String),
    /// The rollout's image is not on the host: prepare it again.
    Unprepared(String),
    /// On a known-good image, recreated, and not healthy there: it is left
    /// there (the best image there is), and an admin is told.
    Left(String),
    /// It failed after the step recreated it.
    Failed(Fault),
}

/// What asking the watched agents again found.
enum Recheck {
    Fine,
    /// One relapsed: its slug and the fault.
    Relapse(String, Fault),
    /// Docker would not answer for one: not this step.
    Unknown(String),
}

impl<'a> Engine<'a> {
    fn now(&self) -> i64 {
        self.ops.now_ms()
    }

    pub async fn load(&self) -> Result<ChannelState, StateError> {
        images::load_state_at(self.pg, self.key).await
    }

    pub async fn patch<T, F>(&self, f: F) -> Result<(ChannelState, T), StateError>
    where
        F: FnOnce(&mut ChannelState) -> Result<T, StateError>,
    {
        images::patch_state_at(self.pg, self.key, f).await
    }

    pub async fn patch_fx<T, F>(&self, f: F) -> Result<(ChannelState, T), StateError>
    where
        F: FnOnce(&mut ChannelState, &mut Effects) -> Result<T, StateError>,
    {
        images::patch_state_fx_at(self.pg, self.key, f).await
    }

    /// The step still holds its lock, or it stops here.
    async fn fenced(&self) -> Result<(), StateError> {
        match self.fence {
            Some(f) if !f.held().await => Err(StateError::Store(
                "the channel's lock was lost with its database connection; this step stops before it moves anything else".into(),
            )),
            _ => Ok(()),
        }
    }

    /// A write into the rollout named `id`, with what belongs with it:
    /// refused (`Superseded`) once the row holds another rollout, or this one
    /// closed — and then none of it happens.
    async fn write_fx<T, F>(&self, id: &str, f: F) -> Result<T, StateError>
    where
        F: FnOnce(&mut Rollout, &mut ChannelState, &mut Effects) -> T,
    {
        self.fenced().await?;
        let now = self.now();
        self.patch_fx(|s, fx| {
            if s.rollout_if(id).is_none() {
                return Err(StateError::Superseded);
            }
            let mut r = s.rollout.take().expect("checked above");
            let out = f(&mut r, s, fx);
            r.updated_at = now;
            s.rollout = Some(r);
            Ok(out)
        })
        .await
        .map(|(_, out)| out)
    }

    async fn write<T, F>(&self, id: &str, f: F) -> Result<T, StateError>
    where
        F: FnOnce(&mut Rollout, &mut ChannelState) -> T,
    {
        self.write_fx(id, |r, s, _| f(r, s)).await
    }

    async fn detail(&self, id: &str, text: &str) -> Result<(), StateError> {
        let text = text.to_string();
        self.write(id, move |r, _| r.detail = Some(text)).await
    }

    /// The rollout named `id` as the row holds it now.
    async fn reload(&self, id: &str) -> Result<Rollout, StateError> {
        self.load()
            .await?
            .rollout
            .filter(|r| r.id == id && r.open())
            .ok_or(StateError::Superseded)
    }

    /// Tell every admin, at most once a day for `topic`.
    async fn alert_once(
        &self,
        kind: &str,
        topic: &str,
        title: &str,
        body: &str,
    ) -> Result<(), StateError> {
        self.alert_every(kind, topic, title, body, DAY).await
    }

    /// Tell every admin, at most once per `period` for `topic`.
    async fn alert_every(
        &self,
        kind: &str,
        topic: &str,
        title: &str,
        body: &str,
        period: i64,
    ) -> Result<(), StateError> {
        let now = self.now();
        let t = topic.to_string();
        let (_, due) = self
            .patch(move |s| {
                s.alerted.retain(|_, at| now - *at < 31 * DAY);
                if s.alerted.get(&t).is_some_and(|at| now - at < period) {
                    return Ok(false);
                }
                s.alerted.insert(t, now);
                Ok(true)
            })
            .await?;
        if due {
            self.ops.notify(kind, topic, title, body).await;
        }
        Ok(())
    }

    /// The compiled pin's digest.
    fn compiled(&self) -> String {
        crate::compiled_pin().0
    }

    fn current(&self, state: &ChannelState) -> String {
        state.current.clone().unwrap_or_else(|| self.compiled())
    }

    /// Each agent on the channel with the digest it runs from its next
    /// render: its assignment, or the fleet's current one.
    async fn effective(
        &self,
        state: &ChannelState,
        agents: &[ChannelAgent],
    ) -> Result<Vec<(ChannelAgent, String)>, String> {
        let assigned = images::assignments(self.pg)
            .await
            .map_err(|e| format!("the agents' images could not be read: {e}"))?;
        let current = self.current(state);
        Ok(agents
            .iter()
            .filter(|a| on_channel(a))
            .map(|a| {
                (
                    a.clone(),
                    assigned
                        .get(&a.id)
                        .cloned()
                        .unwrap_or_else(|| current.clone()),
                )
            })
            .collect())
    }

    async fn record(&self, digest: &str) -> Result<Option<ImageRecord>, String> {
        images::record(self.pg, digest)
            .await
            .map_err(|e| format!("the image record could not be read: {e}"))
    }

    async fn label_of(&self, digest: &str) -> String {
        match self.record(digest).await {
            Ok(Some(r)) => r.label(),
            _ => images::label(None, digest),
        }
    }

    // ── The step ────────────────────────────────────────────────────────

    /// One step, the lock already held: check when due, alert on a stuck
    /// channel, open a rollout when one is called for, advance it. Answers
    /// what happened.
    pub async fn step(&self) -> Result<Vec<String>, String> {
        let state = self.load().await.map_err(|e| e.to_string())?;
        if let Some(why) = self.ops.off_reason().await {
            tracing::debug!("[hermes] standing aside: {why}");
            return Ok(Vec::new());
        }
        let mut said = Vec::new();
        let now = self.now();
        match state.next_check_at {
            None => {
                let at = now + crate::jitter(0, crate::FIRST_CHECK_WITHIN_MS);
                self.patch(|s| {
                    s.next_check_at = Some(at);
                    Ok(())
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            Some(due) if now >= due => {
                said.push(self.run_check().await.map_err(|e| e.to_string())?)
            }
            Some(_) => {}
        }
        self.alert_if_stuck().await.map_err(|e| e.to_string())?;
        if let Some(opened) = self.open_rollout().await? {
            said.push(opened);
        }
        said.extend(self.advance().await?);
        Ok(said)
    }

    /// Check the channel and record what it said, whatever it said.
    pub async fn run_check(&self) -> Result<String, StateError> {
        let now = self.now();
        let found = self.ops.check().await;
        let next = now
            + crate::CHECK_EVERY_MS
            + crate::jitter(-crate::CHECK_JITTER_MS, crate::CHECK_JITTER_MS);
        let sentence = match &found {
            Ok(f) => format!(
                "stable is {}{}",
                images::label(f.version.as_deref(), &f.digest),
                if f.new { " (new)" } else { "" }
            ),
            Err(e) => format!("the check failed: {e}"),
        };
        if let Err(e) = &found {
            tracing::warn!("[hermes] the stable check failed — agents stay where they are: {e}");
        }
        self.patch(|s| {
            // A failed check keeps what the last good one found, so a
            // registry outage never opens or closes anything.
            let mut keep = s.last_check.clone().unwrap_or_default();
            match &found {
                Ok(f) => {
                    // The minimum age counts from when stable first named
                    // this digest, and starts again whenever it moves.
                    let since = if keep.digest.as_deref() == Some(f.digest.as_str()) {
                        keep.since.unwrap_or(keep.at)
                    } else {
                        now
                    };
                    keep.digest = Some(f.digest.clone());
                    keep.version = f.version.clone();
                    keep.error = None;
                    keep.since = Some(since);
                    s.last_good_check_at = Some(now);
                    s.extra.remove("checksFailingSince");
                }
                Err(e) => {
                    keep.error = Some(e.chars().take(500).collect());
                    s.extra
                        .entry("checksFailingSince")
                        .or_insert_with(|| serde_json::json!(now));
                }
            }
            keep.at = now;
            s.last_check = Some(keep);
            s.next_check_at = Some(next);
            Ok(())
        })
        .await?;
        Ok(sentence)
    }

    /// Tell admins once a day while the channel is stuck: its checks have
    /// failed for three days, or a rollout has been open for a week.
    async fn alert_if_stuck(&self) -> Result<(), StateError> {
        let state = self.load().await?;
        let now = self.now();
        if let Some(since) = state
            .extra
            .get("checksFailingSince")
            .and_then(|v| v.as_i64())
            && now - since > 3 * DAY
        {
            let error = state
                .last_check
                .as_ref()
                .and_then(|c| c.error.clone())
                .unwrap_or_default();
            self.alert_once(
                crate::KIND_CHANNEL_STUCK,
                "checks-failing",
                "Agent image checks have failed for 3 days",
                &format!(
                    "The channel could not reach {} since {} days ago, so agents cannot follow Hermes stable: {error}",
                    crate::stable_ref(),
                    (now - since) / DAY
                ),
            )
            .await?;
        }
        if let Some(r) = state.rollout.as_ref().filter(|r| r.open())
            && now - r.started_at > 7 * DAY
        {
            let label = self.label_of(&r.digest).await;
            self.alert_once(
                crate::KIND_CHANNEL_STUCK,
                &format!("rollout-stuck:{}", r.id),
                &format!("The agent image rollout to {label} has been open for a week"),
                &format!(
                    "It started {} days ago and has not finished: {}",
                    (now - r.started_at) / DAY,
                    r.detail.clone().unwrap_or_default()
                ),
            )
            .await?;
        }
        Ok(())
    }

    /// Open the rollout the channel calls for, when none is open: the pinned
    /// digest, a new stable digest that is not bad, or the fleet's own image
    /// for agents an earlier rollout left behind. An open rollout that has
    /// not got an agent in hand gives way: a waiting catch-up to a new stable
    /// release, and an Upgrade (not Roll now's) to whatever stable names now
    /// — so a release Hermes pulls off stable is never rolled out.
    pub async fn open_rollout(&self) -> Result<Option<String>, String> {
        let state = self.load().await.map_err(|e| e.to_string())?;
        let current = self.current(&state);
        let stable = state.last_check.as_ref().and_then(|c| c.digest.clone());
        let stable_record = match &stable {
            Some(d) => self.record(d).await?,
            None => None,
        };
        let stable_ok = stable_record
            .as_ref()
            .is_some_and(|r| r.status != Status::Bad && r.digest != current);
        let open = state.rollout.clone().filter(Rollout::open);
        if let Some(open) = &open {
            let in_hand = open.agent.is_some();
            let gives_way = match open.kind {
                RolloutKind::Catchup => !in_hand && state.pinned.is_none() && stable_ok,
                RolloutKind::Upgrade => {
                    !in_hand && !open.forced && stable.as_deref().is_some_and(|d| d != open.digest)
                }
                _ => false,
            };
            if !gives_way {
                return Ok(None);
            }
        }
        let agents = self.ops.agents().await?;
        let digests = self.effective(&state, &agents).await?;
        // A catch-up that failed on this image is not opened again for it.
        let behind = |slug: &str| state.catchup_failed.get(slug) != Some(&current);
        // An Upgrade giving way with agents already on its image — moved
        // before stable stopped naming it — takes them back to the fleet's
        // image first, a rollback without a ban: a release Hermes withdrew is
        // never left running, unwatched, while the next one waits its time.
        let withdrawn = open
            .as_ref()
            .filter(|o| o.kind == RolloutKind::Upgrade && o.digest != current)
            .filter(|o| digests.iter().any(|(_, d)| d == &o.digest))
            .map(|o| o.digest.clone());
        let mut next = match &state.pinned {
            _ if withdrawn.is_some() => Some((current.clone(), RolloutKind::Rollback)),
            Some(p) if p != &current || digests.iter().any(|(_, d)| d != p) => {
                Some((p.clone(), RolloutKind::Pin))
            }
            Some(_) => None,
            None if stable_ok => Some((stable.clone().expect("stable_ok"), RolloutKind::Upgrade)),
            None if digests
                .iter()
                .any(|(a, d)| d != &current && behind(&a.slug)) =>
            {
                Some((current.clone(), RolloutKind::Catchup))
            }
            None if open.is_none() && self.drifted(&state, &digests).await? => {
                Some((current.clone(), RolloutKind::Catchup))
            }
            None => None,
        };
        let mut record = None;
        if let Some((target, _)) = &next {
            match self.record(target).await? {
                Some(r) if r.status != Status::Bad => record = Some(r),
                _ => next = None,
            }
        }
        if next.is_none() && open.is_none() {
            return Ok(None);
        }
        let now = self.now();
        let replaced = open.as_ref().map(|r| r.id.clone());
        let mut sentences = Vec::new();
        if let Some(old) = open.as_ref().filter(|r| r.kind == RolloutKind::Upgrade) {
            let old_label = self.label_of(&old.digest).await;
            sentences.push(match &stable {
                Some(d) if d == &current => format!(
                    "{old_label} left stable (it names the fleet's own image again): it is not rolled out"
                ),
                Some(d) => format!(
                    "{old_label} left stable for {}: it is not rolled out",
                    self.label_of(d).await
                ),
                None => format!("{old_label} is not rolled out"),
            });
        }
        let rollout = match (next, record) {
            (Some((target, kind)), Some(record)) => {
                let label = record.label();
                let prev = if kind == RolloutKind::Catchup {
                    digests
                        .iter()
                        .filter(|(_, d)| d != &target)
                        .map(|(a, d)| (a.slug.clone(), d.clone()))
                        .collect()
                } else {
                    Default::default()
                };
                sentences.push(match kind {
                    RolloutKind::Rollback => {
                        format!("moving the agents already on it back to {label} first")
                    }
                    RolloutKind::Pin => format!("pinned to {label}: moving the fleet onto it"),
                    RolloutKind::Catchup => {
                        format!("moving the agents an earlier rollout left behind onto {label}")
                    }
                    _ => format!("{label} is new on stable"),
                });
                Some(Rollout {
                    prev,
                    id: images::new_rollout_id(now),
                    digest: target,
                    // A withdrawn release's rollback leaves that release.
                    from: Some(withdrawn.clone().unwrap_or(current)),
                    kind,
                    phase: Phase::Preparing,
                    started_at: now,
                    updated_at: now,
                    // A pin is an admin's action, and a rollback is the way
                    // off a release: both run now.
                    forced: matches!(kind, RolloutKind::Pin | RolloutKind::Rollback),
                    ..Default::default()
                })
            }
            _ => None,
        };
        let sentence = sentences.join("; ");
        let note = sentence.clone();
        self.patch(move |s| {
            // Opened only if the row still holds what was read: an admin's
            // action in between wins.
            let open_now = s
                .rollout
                .as_ref()
                .filter(|r| r.open())
                .map(|r| r.id.clone());
            if open_now != replaced {
                return Err(StateError::Superseded);
            }
            match rollout {
                Some(r) => s.rollout = Some(r),
                None => {
                    if let Some(old) = s.rollout.as_mut() {
                        old.phase = Phase::Cancelled;
                        old.detail = Some(note.clone());
                        old.updated_at = now;
                    }
                }
            }
            s.note(now, note);
            Ok(())
        })
        .await
        .map(|_| Some(sentence))
        .or_else(|e| match e {
            StateError::Superseded => Ok(None),
            e => Err(e.to_string()),
        })
    }

    /// Whether some agent's container is not what its assignment says — it
    /// runs another image (the pre-pin `:latest` a host upgraded from main
    /// still runs, a roll that failed) or is down with no Stop recorded — so
    /// a catch-up onto the fleet's image is due. Asked at most every five
    /// minutes (it asks docker about every agent).
    async fn drifted(
        &self,
        state: &ChannelState,
        digests: &[(ChannelAgent, String)],
    ) -> Result<bool, String> {
        let now = self.now();
        let last = state
            .extra
            .get("driftCheckedAt")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if now - last < 5 * MINUTE && now >= last {
            return Ok(false);
        }
        self.patch(move |s| {
            s.extra
                .insert("driftCheckedAt".into(), serde_json::json!(now));
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?;
        let current = self.current(state);
        let mut ids: std::collections::HashMap<String, Option<String>> = Default::default();
        for (agent, d) in digests {
            if state.catchup_failed.get(&agent.slug) == Some(&current) {
                continue;
            }
            let Ok(snap) = self.ops.snapshot(agent).await else {
                continue;
            };
            if snap.person_stopped(agent) || snap.status == "removing" {
                continue;
            }
            if !snap.running {
                return Ok(true);
            }
            if !ids.contains_key(d) {
                let id = match self.record(d).await? {
                    Some(rec) => self.ops.image_id(&rec.image_ref()).await.ok(),
                    None => None,
                };
                ids.insert(d.clone(), id);
            }
            if ids[d].as_deref() != Some(snap.image.as_str()) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Advance the open rollout as far as its phase and the step's budget
    /// allow. A superseded write re-reads and goes on with the row's rollout.
    pub async fn advance(&self) -> Result<Vec<String>, String> {
        let started = tokio::time::Instant::now();
        let mut said = Vec::new();
        for _ in 0..64 {
            match self.advance_once(started, &mut said).await {
                Ok(true) => continue,
                Ok(false) => break,
                Err(StateError::Superseded) => {
                    tracing::info!("[hermes] the rollout changed under this step; re-reading");
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(said)
    }

    /// One move of the state machine. `Ok(true)`: go round again.
    async fn advance_once(
        &self,
        started: tokio::time::Instant,
        said: &mut Vec<String>,
    ) -> Result<bool, StateError> {
        let state = self.load().await?;
        let Some(r) = state.rollout.clone().filter(Rollout::open) else {
            return Ok(false);
        };
        let Some(record) = self.record(&r.digest).await.map_err(StateError::Store)? else {
            let id = r.id.clone();
            self.write(&id, |r, _| {
                r.phase = Phase::Failed;
                r.detail = Some("its image record is gone".into());
            })
            .await?;
            return Ok(false);
        };
        // Banned under it (an admin, or another path): its agents go back.
        if record.status == Status::Bad && r.kind != RolloutKind::Rollback {
            let why = record
                .reason
                .clone()
                .unwrap_or_else(|| "it was marked bad".into());
            said.push(self.fail_candidate(&r, &record, &why).await?);
            return Ok(true);
        }
        match r.phase {
            Phase::Preparing => self.preparing(&state, &r, &record, said).await,
            Phase::Waiting => self.waiting(&state, &r, &record).await,
            Phase::Canary => self.canary(&r, &record, said).await,
            Phase::Rolling => self.rolling(&state, &r, &record, started, said).await,
            Phase::Soaking => self.soaking(&r, &record, said).await,
            Phase::Done | Phase::Failed | Phase::Cancelled => Ok(false),
        }
    }

    // ── Preparing, waiting, the canary ──────────────────────────────────

    /// How long stable has named the rollout's digest: never old enough
    /// when it no longer does.
    fn stable_age(&self, state: &ChannelState, r: &Rollout, record: &ImageRecord) -> i64 {
        match &state.last_check {
            Some(c) if c.digest.as_deref() == Some(r.digest.as_str()) => {
                self.now() - c.since.unwrap_or(record.first_seen)
            }
            _ => 0,
        }
    }

    async fn preparing(
        &self,
        state: &ChannelState,
        r: &Rollout,
        record: &ImageRecord,
        said: &mut Vec<String>,
    ) -> Result<bool, StateError> {
        let now = self.now();
        let label = record.label();
        if r.retry_at.is_some_and(|at| at > now) {
            return Ok(false);
        }
        // A new release is not even downloaded before its minimum time on
        // stable: one Hermes pulls within it costs nothing here.
        if r.kind == RolloutKind::Upgrade && !r.forced {
            let age = self.stable_age(state, r, record);
            if age < self.tuning.min_stable_ms {
                self.detail(
                    &r.id,
                    &format!(
                        "{label} has been on stable for {} h; it is downloaded and rolled out once it has been there {} h (or on Roll now)",
                        age / HOUR,
                        self.tuning.min_stable_ms / HOUR
                    ),
                )
                .await?;
                return Ok(false);
            }
        }
        let room = crate::prepare::Room {
            urgent: r.kind == RolloutKind::Rollback,
            forced: r.forced,
        };
        match self.ops.prepare(record, room).await {
            Prepared::Ready => {
                let canary = watching(r, record) && r.canary_ok_at.is_none();
                self.write(&r.id, move |r, _| {
                    if canary {
                        r.phase = Phase::Waiting;
                    } else {
                        r.phase = Phase::Rolling;
                        r.rolling_since = r.rolling_since.or(Some(now));
                    }
                    r.retry_at = None;
                    r.infra_failures = 0;
                    if r.strike_on.as_deref() == Some("prepare") {
                        r.strikes = 0;
                        r.strike_on = None;
                    }
                    r.detail = Some(format!("{label} is on the host and inspected"));
                })
                .await?;
                Ok(true)
            }
            Prepared::Pulling(p) | Prepared::Waiting(p) => {
                self.detail(&r.id, &p).await?;
                Ok(false)
            }
            Prepared::NoSpace(why) => {
                self.alert_once(
                    crate::KIND_CHANNEL_STUCK,
                    &format!("stuck:{}", r.id),
                    &format!("The agent image update to {label} needs disk space"),
                    &format!(
                        "{why}. Nothing is downloaded until there is room: a full disk would stop the database."
                    ),
                )
                .await?;
                said.push(
                    self.failed(r, record, "prepare", None, Fault::Infra(why))
                        .await?,
                );
                Ok(false)
            }
            Prepared::Failed {
                why,
                definitive: true,
            } => {
                said.push(
                    self.failed(r, record, "prepare", None, Fault::Image(why))
                        .await?,
                );
                Ok(true)
            }
            Prepared::Failed { why, .. } => {
                said.push(
                    self.failed(r, record, "prepare", None, Fault::Infra(why))
                        .await?,
                );
                Ok(true)
            }
        }
    }

    async fn waiting(
        &self,
        state: &ChannelState,
        r: &Rollout,
        record: &ImageRecord,
    ) -> Result<bool, StateError> {
        let now = self.now();
        let label = record.label();
        if r.retry_at.is_some_and(|at| at > now) {
            return Ok(false);
        }
        // Held for an admin: a Hermes internal Talaria relies on changed.
        if watching(r, record) && record.held() {
            let text = format!(
                "{label} is held for an admin: {}. It rolls out once an admin accepts it on the Manage panel.",
                record.contract_problems().join("; ")
            );
            self.detail(&r.id, &text).await?;
            self.alert_once(
                crate::KIND_UPDATE_FAILED,
                &format!("held:{}", r.digest),
                &format!("Agent image {label} is held for review"),
                &text,
            )
            .await?;
            return Ok(false);
        }
        let (open, zone) = self.ops.window_open(&state.window).await;
        if !(r.forced || open) {
            self.detail(
                &r.id,
                &format!(
                    "{label} is ready; it rolls out in the maintenance window ({}–{} {zone}) or on Roll now",
                    state.window.start, state.window.end
                ),
            )
            .await?;
            return Ok(false);
        }
        let canary = watching(r, record) && r.canary_ok_at.is_none();
        self.write(&r.id, move |r, _| {
            r.retry_at = None;
            if canary {
                r.phase = Phase::Canary;
                r.detail = Some(format!("booting {label} as the canary"));
            } else {
                r.phase = Phase::Rolling;
                r.rolling_since = r.rolling_since.or(Some(now));
            }
        })
        .await?;
        Ok(true)
    }

    async fn canary(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        said: &mut Vec<String>,
    ) -> Result<bool, StateError> {
        let label = record.label();
        match self.ops.canary(record).await {
            Ok(passed) => {
                let now = self.now();
                let text = format!("{label}: {passed}");
                let digest = record.digest.clone();
                self.write_fx(&r.id, move |r, s, fx| {
                    fx.canary_ok(&digest, now);
                    r.phase = Phase::Rolling;
                    r.rolling_since = r.rolling_since.or(Some(now));
                    r.canary_ok_at = Some(now);
                    r.infra_failures = 0;
                    if r.strike_on.as_deref() == Some("canary") {
                        r.strikes = 0;
                        r.strike_on = None;
                    }
                    r.detail = Some(passed.clone());
                    s.note(now, text);
                })
                .await?;
                said.push(format!("{label}: the canary passed"));
                Ok(true)
            }
            Err(fault) => {
                said.push(self.failed(r, record, "canary", None, fault).await?);
                Ok(true)
            }
        }
    }

    // ── Rolling ─────────────────────────────────────────────────────────

    async fn rolling(
        &self,
        state: &ChannelState,
        r: &Rollout,
        record: &ImageRecord,
        started: tokio::time::Instant,
        said: &mut Vec<String>,
    ) -> Result<bool, StateError> {
        let now = self.now();
        let label = record.label();
        if r.retry_at.is_some_and(|at| at > now) {
            return Ok(false);
        }
        let agents = self.ops.agents().await.map_err(StateError::Store)?;
        // An agent this step's predecessor was moving when it was stopped
        // (an api restart mid-roll): settled before anything else.
        if let Some(slug) = r.agent.clone() {
            if let Some(agent) = agents.iter().find(|a| a.slug == slug) {
                return self.resume(r, record, agent, said).await;
            }
            self.write(&r.id, |r, _| r.agent = None).await?;
        }
        let watched = watching(r, record);
        if watched {
            // Every agent already moved is asked again first — all of them,
            // whatever one says.
            match self.recheck(r, record, &agents).await? {
                Recheck::Fine => {}
                Recheck::Relapse(slug, fault) => {
                    let agent = agents.iter().find(|a| a.slug == slug).cloned();
                    said.push(self.failed(r, record, &slug, agent.as_ref(), fault).await?);
                    return Ok(true);
                }
                Recheck::Unknown(why) => {
                    said.push(
                        self.failed(r, record, "recheck", None, Fault::Infra(why))
                            .await?,
                    );
                    return Ok(true);
                }
            }
        }
        let r = &self.reload(&r.id).await?;
        // The last agent moved soaks before the next one moves; once it has,
        // a strike against it is cleared.
        if watched && let Some(last) = r.watch.last() {
            let need = if r.watch.len() == 1 {
                self.tuning.first_agent_soak_ms
            } else {
                self.tuning.agent_soak_ms
            };
            if now < last.moved_at + need {
                self.detail(
                    &r.id,
                    &format!(
                        "watching {} on {label} for {} more min before the next agent moves",
                        last.slug,
                        (last.moved_at + need - now + MINUTE - 1) / MINUTE
                    ),
                )
                .await?;
                return Ok(false);
            }
            if r.strike_on.as_deref() == Some(last.slug.as_str()) {
                self.write(&r.id, |r, _| {
                    r.strikes = 0;
                    r.strike_on = None;
                })
                .await?;
            }
        }
        if let Some(why) = r.halted.clone() {
            self.detail(
                &r.id,
                &format!("stopped for an admin: {why} — Roll now goes on with the rest"),
            )
            .await?;
            return Ok(false);
        }
        let digests = self
            .effective(state, &agents)
            .await
            .map_err(StateError::Store)?;
        let target = match self.ops.image_id(&record.image_ref()).await {
            Ok(t) => t,
            Err(e) => {
                let text = format!("{label} is not on the host ({e}): downloading it again");
                self.write(&r.id, move |r, _| {
                    r.phase = Phase::Preparing;
                    r.detail = Some(text);
                })
                .await?;
                return Ok(true);
            }
        };
        let from_image = match r.from.as_deref() {
            Some(d) if r.kind == RolloutKind::Rollback => match self.record(d).await {
                Ok(Some(rec)) => self.ops.image_id(&rec.image_ref()).await.ok(),
                _ => None,
            },
            _ => None,
        };
        // Still to move: every agent not settled on the image — assigned it
        // AND its container running it (or stopped by a person) — so a roll
        // that failed, a put-back that failed, or an agent left down is
        // moved again, whatever the assignment says. For a rollback, only
        // the agents on the image it leaves (or meant for its target).
        let mut pending: Vec<&ChannelAgent> = Vec::new();
        for (a, d) in &digests {
            if r.deferred.contains(&a.slug)
                || (r.kind == RolloutKind::Catchup
                    && state.catchup_failed.get(&a.slug) == Some(&r.digest))
            {
                continue;
            }
            let snap = self.ops.snapshot(a).await.ok();
            let settled = d == &r.digest
                && snap
                    .as_ref()
                    .is_some_and(|s| s.person_stopped(a) || (s.running && s.image == target));
            if settled {
                continue;
            }
            let ours = match r.kind {
                RolloutKind::Rollback => {
                    r.from.as_deref() == Some(d.as_str())
                        || d == &r.digest
                        || snap
                            .as_ref()
                            .zip(from_image.as_ref())
                            .is_some_and(|(s, f)| s.running && &s.image == f)
                }
                _ => true,
            };
            if ours {
                pending.push(a);
            }
        }
        let (open, zone) = self.ops.window_open(&state.window).await;
        let gated = !(r.forced || r.kind == RolloutKind::Rollback) && !open;
        // Busy all window (or, for a forced rollout, for hours): left on the
        // image it runs, for a catch-up in a later window. A rollback never
        // leaves an agent on the image it is leaving.
        let busy_long = r.kind != RolloutKind::Rollback
            && (gated
                || (r.forced
                    && r.rolling_since
                        .is_some_and(|t| now - t > self.tuning.forced_defer_ms)));
        if busy_long {
            let skipped: Vec<(String, String, String)> = pending
                .iter()
                .filter_map(|a| {
                    r.skipped
                        .iter()
                        .find(|s| s.slug == a.slug)
                        .map(|s| (a.slug.clone(), a.display_name.clone(), s.why.clone()))
                })
                .collect();
            if !skipped.is_empty() {
                let list = skipped
                    .iter()
                    .map(|(_, n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                let s2 = skipped.clone();
                let long: Vec<(String, String, String, String)> = self
                    .write(&r.id, move |r, s| {
                        let mut long = Vec::new();
                        for (slug, name, why) in s2 {
                            r.deferred.push(slug.clone());
                            let n = s.deferrals.entry(slug.clone()).or_insert(0);
                            *n += 1;
                            if *n >= DEFER_ALERT_AFTER {
                                long.push((slug, name, why, n.to_string()));
                            }
                        }
                        s.note(
                            now,
                            format!("{list}: busy all window — left for a later window"),
                        );
                        long
                    })
                    .await?;
                for (slug, name, why, n) in long {
                    let unknown = why.starts_with("whether it is working could not be read");
                    let advice = if unknown {
                        "Its Hermes does not say whether it is working, so the channel will not restart it on its own: Roll now (or Pin) restarts it anyway."
                    } else {
                        "It moves the first night it is free; Roll now moves it as soon as it is. A schedule that keeps it busy every night keeps it behind."
                    };
                    // Said again every week for as long as it lasts.
                    self.alert_every(
                        crate::KIND_CHANNEL_STUCK,
                        &format!("left-behind:{slug}"),
                        &format!("{name} has been left behind on an older Hermes image"),
                        &format!(
                            "{name} was passed over through {n} maintenance windows in a row ({why}), so it does not run {label} like the rest of the fleet. {advice}"
                        ),
                        7 * DAY,
                    )
                    .await?;
                }
                pending.retain(|a| !skipped.iter().any(|(s, _, _)| s == &a.slug));
            }
        }
        if pending.is_empty() {
            if !watched {
                said.push(self.finish(r, record).await?);
                return Ok(true);
            }
            return self.begin_soak(r, &label).await.map(|_| true);
        }
        if gated {
            self.detail(
                &r.id,
                &format!(
                    "paused outside the maintenance window ({}–{} {zone}); {} moved so far",
                    state.window.start,
                    state.window.end,
                    r.moved.len()
                ),
            )
            .await?;
            return Ok(false);
        }
        for agent in pending {
            if started.elapsed() > self.tuning.step_budget {
                return Ok(false);
            }
            // The row and the image, again, before each agent.
            let r = &self.reload(&r.id).await?;
            match self.record(&r.digest).await.map_err(StateError::Store)? {
                Some(rec) if rec.status != Status::Bad || r.kind == RolloutKind::Rollback => {}
                _ => return Ok(true),
            }
            match self.move_agent(r, record, agent).await? {
                Move::Moved(watch, how) => {
                    tracing::info!("[hermes] {} ({}): {how}", agent.display_name, agent.slug);
                    let soaks = watched && watch.is_some();
                    self.moved(r, agent, watch, how).await?;
                    if soaks {
                        // It soaks before the next one moves.
                        return Ok(true);
                    }
                }
                Move::Skipped(why) => {
                    tracing::info!(
                        "[hermes] {} ({}) passed over for now: {why}",
                        agent.display_name,
                        agent.slug
                    );
                    let slug = agent.slug.clone();
                    let name = agent.display_name.clone();
                    self.write(&r.id, move |r, _| {
                        r.note_skip(&slug, &why, now);
                        r.agent = None;
                        r.detail = Some(format!(
                            "{name} passed over for now: {why}; it is asked again on the next step"
                        ));
                    })
                    .await?;
                }
                Move::Unprepared(why) => {
                    let text = format!("{label} is not on the host ({why}): downloading it again");
                    self.write(&r.id, move |r, _| {
                        r.phase = Phase::Preparing;
                        r.detail = Some(text);
                    })
                    .await?;
                    return Ok(true);
                }
                Move::Left(how) => {
                    self.left(r, agent, how).await?;
                    if r.kind == RolloutKind::Pin {
                        return Ok(false);
                    }
                }
                Move::Failed(fault) => {
                    said.push(
                        self.failed(r, record, &agent.slug, Some(agent), fault)
                            .await?,
                    );
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Record an agent left unhealthy on a known-good image: out of this
    /// rollout's way (it is where it should be), and — for a pin, an admin's
    /// choice that may not suit today's config — the pin stops there.
    async fn left(&self, r: &Rollout, agent: &ChannelAgent, how: String) -> Result<(), StateError> {
        let slug = agent.slug.clone();
        let halt = r.kind == RolloutKind::Pin;
        self.write(&r.id, move |r, _| {
            if !r.moved.contains(&slug) {
                r.moved.push(slug.clone());
            }
            if !r.deferred.contains(&slug) {
                r.deferred.push(slug);
            }
            r.agent = None;
            if halt {
                r.halted = Some(how.clone());
            }
            r.detail = Some(how);
        })
        .await
    }

    /// Record an agent that runs the rollout's image now.
    async fn moved(
        &self,
        r: &Rollout,
        agent: &ChannelAgent,
        watch: Option<Watched>,
        how: String,
    ) -> Result<(), StateError> {
        let slug = agent.slug.clone();
        let name = agent.display_name.clone();
        self.write(&r.id, move |r, s| {
            if !r.moved.contains(&slug) {
                r.moved.push(slug.clone());
            }
            r.skipped.retain(|k| k.slug != slug);
            r.watch.retain(|k| k.slug != slug);
            if let Some(w) = watch {
                r.watch.push(w);
            }
            r.agent = None;
            r.agent_faults.clear();
            r.infra_failures = 0;
            s.deferrals.remove(&slug);
            r.detail = Some(format!("{name}: {how}"));
        })
        .await
    }

    /// One agent onto the rollout's image.
    async fn move_agent(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        agent: &ChannelAgent,
    ) -> Result<Move, StateError> {
        let label = record.label();
        let target = match self.ops.image_id(&record.image_ref()).await {
            Ok(t) => t,
            Err(e) => return Ok(Move::Unprepared(e)),
        };
        let snap = match self.ops.snapshot(agent).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Move::Skipped(format!(
                    "docker would not describe its container ({e})"
                )));
            }
        };
        let (id, digest) = (agent.id.clone(), r.digest.clone());
        if snap.status == "removing" {
            return Ok(Move::Skipped("docker is removing its container".into()));
        }
        // Stopped by a person (Talaria recorded their Stop): assigned, and
        // it starts on the image when a person starts it.
        if snap.person_stopped(agent) {
            self.write_fx(&r.id, move |_, _, fx| fx.assign(&id, &digest))
                .await?;
            self.ops.render(agent).await;
            return Ok(Move::Moved(
                None,
                format!("stopped by a person; it starts on {label} next time"),
            ));
        }
        // Down without being stopped (crash-looping, a recreate left half
        // done) is recreated whatever it is doing; so is every agent a
        // rollback moves — it is leaving something worse.
        let down = snap.down_unwillingly(agent);
        if !(down || r.kind == RolloutKind::Rollback) {
            // An image this build never inspected (the pre-pin `:latest` a
            // host upgraded from main runs, an operator's) may predate the
            // fields Talaria reads; its answer is read for what it has.
            let lenient = !self
                .record(&snap.image)
                .await
                .ok()
                .flatten()
                .is_some_and(|rec| rec.contract_checked());
            match self.ops.busy(agent, lenient).await {
                Ok(Busy::Working(why)) => return Ok(Move::Skipped(why)),
                // Not on its own; an admin's Roll now or Pin (whose confirm
                // says so) restarts it anyway.
                Ok(Busy::Unknown(why)) if !r.forced => {
                    return Ok(Move::Skipped(format!(
                        "{why}, so it is not restarted now (Roll now restarts it anyway)"
                    )));
                }
                Ok(Busy::Unknown(why)) => tracing::info!(
                    "[hermes] {}: {why} — restarted anyway, on an admin's Roll now or Pin",
                    agent.display_name
                ),
                Ok(Busy::Idle) => {}
                Err(e) => {
                    return Ok(Move::Skipped(format!(
                        "whether it is in use could not be read ({e})"
                    )));
                }
            }
        }
        let known = if snap.running {
            self.ops.log_faults(agent).await
        } else {
            Vec::new()
        };
        // Its assignment and "this agent is in hand", in one guarded write:
        // a Roll back committed meanwhile refuses both.
        let slug = agent.slug.clone();
        let k = known.clone();
        self.write_fx(&r.id, move |r, _, fx| {
            r.agent = Some(slug);
            r.agent_faults = k;
            fx.assign(&id, &digest);
        })
        .await?;
        let rolled = self.ops.roll(agent, down).await;
        let checked = match rolled {
            Ok(()) => self.check_agent(agent, &target, None, &known).await,
            Err(f) => Err(f),
        };
        self.settle(r, record, agent, checked).await
    }

    /// What an agent the step recreated comes to. On an image already known
    /// good (a rollback, a pin to a good version), a failure is not the
    /// image's to answer for: the agent stays there — it is the best image
    /// there is — and an admin is told.
    async fn settle(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        agent: &ChannelAgent,
        checked: Result<Option<Watched>, Fault>,
    ) -> Result<Move, StateError> {
        let label = record.label();
        let watched = watching(r, record);
        match checked {
            Ok(Some(w)) => Ok(Move::Moved(
                watched.then_some(w),
                format!("rolled onto {label} and healthy"),
            )),
            Ok(None) => Ok(Move::Moved(
                None,
                format!("on {label}, and stopped by a person"),
            )),
            Err(Fault::Image(why)) if !watched && r.kind != RolloutKind::Catchup => {
                let more = if r.kind == RolloutKind::Pin {
                    " The pin stops here, before it moves anyone else onto it: Roll now goes on with the rest, or pin another version."
                } else {
                    ""
                };
                let text = format!(
                    "{} is on {label}, but {why}. It stays there — {label} is the known good — so look at its logs.{more}",
                    agent.display_name
                );
                self.alert_once(
                    crate::KIND_UPDATE_FAILED,
                    &format!("unhealthy:{}:{}", agent.slug, r.digest),
                    &format!("{} is not healthy on {label}", agent.display_name),
                    &text,
                )
                .await?;
                Ok(Move::Left(format!(
                    "{}: on {label}, but {why} — left there, on the known good",
                    agent.display_name
                )))
            }
            Err(f) => Ok(Move::Failed(f)),
        }
    }

    /// The agent the step before this one was moving when it stopped. Its
    /// assignment is the target already (written with it in hand), so it is
    /// finished here: checked if it came up on the image, recreated if not.
    async fn resume(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        agent: &ChannelAgent,
        said: &mut Vec<String>,
    ) -> Result<bool, StateError> {
        let label = record.label();
        let target = match self.ops.image_id(&record.image_ref()).await {
            Ok(t) => t,
            Err(e) => {
                // Kept in hand: it is finished once the image is back.
                let text = format!("{label} is not on the host ({e}): downloading it again");
                self.write(&r.id, move |r, _| {
                    r.phase = Phase::Preparing;
                    r.detail = Some(text);
                })
                .await?;
                return Ok(true);
            }
        };
        let snap = match self.ops.snapshot(agent).await {
            Ok(s) => s,
            Err(e) => {
                self.detail(
                    &r.id,
                    &format!(
                        "docker would not describe {}, whose move was interrupted ({e}); asked again on the next step",
                        agent.display_name
                    ),
                )
                .await?;
                return Ok(false);
            }
        };
        let known = r.agent_faults.clone();
        let checked = if snap.running && snap.image == target {
            self.check_agent(agent, &target, None, &known).await
        } else {
            // Never recreated, or left down by the interruption: the move
            // was this channel's, so it is finished now.
            match self.ops.roll(agent, !snap.running).await {
                Ok(()) => self.check_agent(agent, &target, None, &known).await,
                Err(f) => Err(f),
            }
        };
        match self.settle(r, record, agent, checked).await? {
            Move::Moved(watch, how) => {
                self.moved(r, agent, watch, format!("its interrupted move: {how}"))
                    .await?;
                Ok(true)
            }
            Move::Failed(fault) => {
                said.push(
                    self.failed(r, record, &agent.slug, Some(agent), fault)
                        .await?,
                );
                Ok(true)
            }
            Move::Left(how) => {
                self.left(r, agent, how).await?;
                Ok(true)
            }
            Move::Skipped(_) | Move::Unprepared(_) => Ok(false),
        }
    }

    /// Whether the container's process started in the last ten minutes.
    fn started_lately(&self, snap: &Snapshot) -> bool {
        chrono::DateTime::parse_from_rfc3339(&snap.started_at)
            .is_ok_and(|t| self.now() - t.timestamp_millis() < 10 * MINUTE)
    }

    fn watch_of(&self, agent: &ChannelAgent, snap: &Snapshot, known: Vec<String>) -> Watched {
        Watched {
            slug: agent.slug.clone(),
            container: snap.container.clone(),
            restart_count: snap.restart_count,
            moved_at: self.now(),
            known_faults: known,
            started_at: snap.started_at.clone(),
            extra: Default::default(),
        }
    }

    /// Ask a moved agent everything: running, healthy, on the image, its
    /// restart count unchanged since `baseline`, its API answering the
    /// gateway, its drain in place, no fault in its log beyond `known`.
    /// `Ok(None)`: nothing to watch — stopped by a person, gone, or (asked
    /// again) moved off the image by something else.
    async fn check_agent(
        &self,
        agent: &ChannelAgent,
        target: &str,
        baseline: Option<&Watched>,
        known: &[String],
    ) -> Result<Option<Watched>, Fault> {
        let snap = self
            .ops
            .snapshot(agent)
            .await
            .map_err(|e| Fault::Infra(format!("docker would not describe it: {e}")))?;
        if !snap.exists {
            return Ok(None);
        }
        let restarted = baseline
            .filter(|b| b.container == snap.container)
            .map(|b| snap.restart_count - b.restart_count)
            .unwrap_or(0);
        if !snap.running {
            if snap.down_unwillingly(agent) || restarted > 0 {
                return Err(Fault::Image(
                    if snap.status == "restarting" || restarted > 0 {
                        format!(
                            "it keeps restarting ({} restarts since it came up)",
                            restarted.max(1)
                        )
                    } else {
                        format!("it is down ({}) with no Stop recorded", snap.status)
                    },
                ));
            }
            return Ok(None);
        }
        if snap.image != target {
            if baseline.is_some() {
                return Ok(None);
            }
            return Err(Fault::Infra(
                "its container does not run the rollout's image after its roll — HERMES_IMAGE or a chassis override is in the way".into(),
            ));
        }
        // Started again with its restart count unchanged: the daemon did it
        // (a reboot, a docker restart), not a crash — the host's, and it is
        // watched from its new start.
        if let Some(b) = baseline
            && b.container == snap.container
            && restarted == 0
            && !b.started_at.is_empty()
            && snap.started_at != b.started_at
        {
            return Ok(Some(Watched {
                started_at: snap.started_at.clone(),
                ..b.clone()
            }));
        }
        if restarted > 0 {
            return Err(Fault::Image(format!(
                "it restarted {restarted} time{} since it came up",
                if restarted == 1 { "" } else { "s" }
            )));
        }
        // Recreated since (an edit's apply rolled it): watch the new
        // container from here.
        if let Some(b) = baseline
            && b.container != snap.container
        {
            return Ok(Some(Watched {
                container: snap.container.clone(),
                restart_count: snap.restart_count,
                ..b.clone()
            }));
        }
        match snap.health.as_deref() {
            Some("unhealthy") => return Err(Fault::Image("its health check fails".into())),
            // Just (re)started by the daemon: still in its start period.
            Some("starting") if baseline.is_some() && self.started_lately(&snap) => {
                return Ok(baseline.cloned());
            }
            Some("starting") if baseline.is_some() => {
                return Err(Fault::Image("it stopped reporting healthy".into()));
            }
            _ => {}
        }
        self.ops.answers(agent).await?;
        let faults = self.ops.log_faults(agent).await;
        let known: Vec<String> = baseline
            .map(|b| b.known_faults.clone())
            .unwrap_or_else(|| known.to_vec());
        if let Some(new) = faults.iter().find(|f| !known.contains(f)) {
            return Err(Fault::Image(format!("its log says: {new}")));
        }
        Ok(Some(match baseline {
            Some(b) => b.clone(),
            None => self.watch_of(agent, &snap, known),
        }))
    }

    /// Ask every watched agent again, every one of them whatever another
    /// says: one gone or moved off the image leaves the watch, one recreated
    /// meanwhile is watched from its new container, and the verdict is the
    /// first relapse, else the first agent docker would not describe.
    async fn recheck(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        agents: &[ChannelAgent],
    ) -> Result<Recheck, StateError> {
        if r.watch.is_empty() {
            return Ok(Recheck::Fine);
        }
        let target = match self.ops.image_id(&record.image_ref()).await {
            Ok(t) => t,
            Err(e) => {
                return Ok(Recheck::Unknown(format!(
                    "docker would not describe {}: {e}",
                    record.label()
                )));
            }
        };
        let mut drop: Vec<String> = Vec::new();
        let mut moved_on: Vec<Watched> = Vec::new();
        let mut relapse = None;
        let mut unknown = None;
        for w in &r.watch {
            let Some(agent) = agents.iter().find(|a| a.slug == w.slug) else {
                drop.push(w.slug.clone());
                continue;
            };
            match self.check_agent(agent, &target, Some(w), &[]).await {
                Ok(Some(next)) if &next != w => moved_on.push(next),
                Ok(Some(_)) => {}
                Ok(None) => drop.push(w.slug.clone()),
                Err(Fault::Infra(e)) => {
                    unknown.get_or_insert(format!("{}: {e}", agent.display_name));
                }
                Err(fault) => {
                    relapse.get_or_insert((w.slug.clone(), fault));
                }
            }
        }
        if !drop.is_empty() || !moved_on.is_empty() {
            self.write(&r.id, move |r, _| {
                r.watch.retain(|w| !drop.contains(&w.slug));
                for n in moved_on {
                    if let Some(w) = r.watch.iter_mut().find(|w| w.slug == n.slug) {
                        *w = n;
                    }
                }
            })
            .await?;
        }
        Ok(match (relapse, unknown) {
            (Some((slug, fault)), _) => Recheck::Relapse(slug, fault),
            (None, Some(why)) => Recheck::Unknown(why),
            (None, None) => Recheck::Fine,
        })
    }

    async fn begin_soak(&self, r: &Rollout, label: &str) -> Result<(), StateError> {
        let now = self.now();
        let until = now + self.tuning.rollout_soak_ms;
        let n = r.watch.len();
        let label = label.to_string();
        self.write(&r.id, move |r, _| {
            r.phase = Phase::Soaking;
            r.soak_until = Some(until);
            r.detail = Some(format!(
                "every agent has moved; watching {n} on {label} for {} min before calling it good",
                (until - now) / MINUTE
            ));
        })
        .await
    }

    async fn soaking(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        said: &mut Vec<String>,
    ) -> Result<bool, StateError> {
        let now = self.now();
        if r.retry_at.is_some_and(|at| at > now) {
            return Ok(false);
        }
        let agents = self.ops.agents().await.map_err(StateError::Store)?;
        match self.recheck(r, record, &agents).await? {
            Recheck::Fine => {}
            Recheck::Relapse(slug, fault) => {
                let agent = agents.iter().find(|a| a.slug == slug).cloned();
                said.push(self.failed(r, record, &slug, agent.as_ref(), fault).await?);
                return Ok(true);
            }
            Recheck::Unknown(why) => {
                said.push(
                    self.failed(r, record, "recheck", None, Fault::Infra(why))
                        .await?,
                );
                return Ok(true);
            }
        }
        if r.soak_until.is_some_and(|t| now < t) {
            return Ok(false);
        }
        let r = self.reload(&r.id).await?;
        said.push(self.finish(&r, record).await?);
        Ok(true)
    }

    // ── Failure ─────────────────────────────────────────────────────────

    /// A failure on the rollout's image, of `item` (`canary`, `prepare`,
    /// `recheck`, or an agent's slug); `touched` is the agent the step had
    /// recreated, if any.
    async fn failed(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        item: &str,
        touched: Option<&ChannelAgent>,
        fault: Fault,
    ) -> Result<String, StateError> {
        let now = self.now();
        let label = record.label();
        let watched = watching(r, record);
        let stalled = self.ops.host_stalled();
        let mut masked = false;
        let fault = match fault {
            Fault::Image(why) if watched && stalled && r.masked < MASKED_MAX => {
                masked = true;
                Fault::Infra(format!(
                    "{why} (while the host's disk was saturated, so it is not counted against the image)"
                ))
            }
            Fault::Image(why) if watched && stalled => Fault::Image(format!(
                "{why} (the host's disk was saturated, but it excused {MASKED_MAX} failures of this rollout already, so this one counts)"
            )),
            f => f,
        };
        let who = touched
            .map(|a| format!("{}: ", a.display_name))
            .unwrap_or_default();
        match fault {
            Fault::Held(problems) => {
                if let Err(e) = images::note_contract(self.pg, &record.digest, &problems).await {
                    return Err(StateError::Store(e.to_string()));
                }
                let text = format!("{label} is held for an admin: {}", problems.join("; "));
                let t = text.clone();
                self.write(&r.id, move |r, s| {
                    r.phase = Phase::Waiting;
                    r.agent = None;
                    s.note(now, t.clone());
                    r.detail = Some(t);
                })
                .await?;
                Ok(text)
            }
            Fault::Infra(why) => {
                self.host_failed(r, record, touched.filter(|_| watched), &who, &why, masked)
                    .await
            }
            // A catch-up's image already runs the fleet: never banned by an
            // agent that fails on it, whether or not it has soaked here.
            Fault::Image(why) if r.kind == RolloutKind::Catchup && touched.is_some() => {
                let agent = touched.expect("checked");
                self.catchup_failed(r, record, agent, &why).await
            }
            Fault::Image(why) if !watched => {
                // A known-good image is never banned: asked again, and said.
                self.host_failed(
                    r,
                    record,
                    None,
                    &who,
                    &format!(
                        "{why} — {label} is known good here, so it is asked again, never banned"
                    ),
                    false,
                )
                .await
            }
            Fault::Image(why) => {
                // The same thing twice is an answer; so are three failures
                // of anything (an image that fails something new each time).
                if (r.strikes >= 1 && r.strike_on.as_deref() == Some(item))
                    || r.image_failures + 1 >= 3
                {
                    return self.fail_candidate(r, record, &format!("{who}{why}")).await;
                }
                let back = touched.map(|a| (a.clone(), self.back_to(r, a)));
                let text = format!(
                    "{label}: {who}{why} — tried once more in {} min to confirm before it is marked bad{}",
                    self.tuning.retry_confirm_ms / MINUTE,
                    back.as_ref()
                        .map(|(a, _)| format!("; {} goes back meanwhile", a.display_name))
                        .unwrap_or_default()
                );
                let retry = now + self.tuning.retry_confirm_ms;
                let t = text.clone();
                let item = item.to_string();
                let b = back.clone();
                self.write_fx(&r.id, move |r, s, fx| {
                    r.strikes = 1;
                    r.image_failures += 1;
                    r.strike_on = Some(item);
                    r.retry_at = Some(retry);
                    r.agent = None;
                    if masked {
                        r.masked += 1;
                    }
                    if let Some((a, d)) = &b {
                        fx.assign(&a.id, d);
                        r.watch.retain(|w| w.slug != a.slug);
                    }
                    if r.phase == Phase::Canary {
                        r.phase = Phase::Waiting;
                    } else if r.phase == Phase::Soaking {
                        r.phase = Phase::Rolling;
                    }
                    s.note(now, t.clone());
                    r.detail = Some(t);
                })
                .await?;
                if let Some((a, d)) = &back {
                    self.put_back(a, d).await;
                }
                Ok(text)
            }
        }
    }

    /// The host failed: tried again after a wait that doubles each time,
    /// and the third in a row tells the admins. An agent the step had
    /// recreated on an image not yet known good goes back meanwhile.
    async fn host_failed(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        touched: Option<&ChannelAgent>,
        who: &str,
        why: &str,
        masked: bool,
    ) -> Result<String, StateError> {
        let now = self.now();
        let label = record.label();
        let n = r.infra_failures + 1;
        let wait = backoff(self.tuning.retry_infra_ms, n);
        let back = touched.map(|a| (a.clone(), self.back_to(r, a)));
        let verb = match r.kind {
            RolloutKind::Rollback => format!("could not be put back on {label}"),
            _ => format!("could not be moved onto {label}"),
        };
        let what = if who.is_empty() {
            format!("{label}: ")
        } else {
            format!("{who}{verb}: ")
        };
        let text = format!(
            "{what}{why} — tried again in {} min{}",
            wait / MINUTE,
            back.as_ref()
                .map(|(a, _)| format!("; {} goes back meanwhile", a.display_name))
                .unwrap_or_default()
        );
        let t = text.clone();
        let b = back.clone();
        self.write_fx(&r.id, move |r, s, fx| {
            r.infra_failures = n;
            if masked {
                r.masked += 1;
            }
            r.retry_at = Some(now + wait);
            r.agent = None;
            if let Some((a, d)) = &b {
                fx.assign(&a.id, d);
                r.watch.retain(|w| w.slug != a.slug);
            }
            if r.phase == Phase::Canary {
                r.phase = Phase::Waiting;
            }
            s.note(now, t.clone());
            r.detail = Some(t);
        })
        .await?;
        if let Some((a, d)) = &back {
            self.put_back(a, d).await;
        }
        if n >= INFRA_ALERT_AFTER {
            let kind = match r.kind {
                RolloutKind::Rollback => "rollback",
                RolloutKind::Pin => "pin",
                RolloutKind::Catchup => "catch-up",
                RolloutKind::Upgrade => "update",
            };
            self.alert_once(
                crate::KIND_CHANNEL_STUCK,
                &format!("stuck:{}", r.id),
                &format!("The agent image {kind} to {label} keeps failing on the host"),
                &format!(
                    "{text}. It has failed {n} times in a row; it is tried again less and less often, at most every {} h.",
                    BACKOFF_CAP_MS / HOUR
                ),
            )
            .await?;
        }
        Ok(text)
    }

    /// A catch-up's image already runs the fleet: an agent that fails on it
    /// goes back to what it ran and is left for a person; nothing is banned.
    async fn catchup_failed(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        agent: &ChannelAgent,
        why: &str,
    ) -> Result<String, StateError> {
        let now = self.now();
        let label = record.label();
        let back = self.back_to(r, agent);
        // Something to go back to (the digest it ran): put back on it. An
        // agent that was on no image of record (the pre-pin `:latest`) stays
        // where the roll left it.
        let put_back = back != r.digest;
        let text = format!(
            "{}: failed on {label} ({why}) though the rest of the fleet runs it; {}. It is not moved onto {label} again until the fleet's image changes or an admin presses Roll now",
            agent.display_name,
            if put_back {
                "it is back on what it ran"
            } else {
                "it is left where it is"
            }
        );
        let t = text.clone();
        let (slug, id, d, digest) = (
            agent.slug.clone(),
            agent.id.clone(),
            back.clone(),
            r.digest.clone(),
        );
        self.write_fx(&r.id, move |r, s, fx| {
            if put_back {
                fx.assign(&id, &d);
            }
            // Remembered past this catch-up: the next one leaves it alone.
            s.catchup_failed.insert(slug.clone(), digest);
            r.deferred.push(slug);
            r.agent = None;
            s.note(now, t.clone());
            r.detail = Some(t);
        })
        .await?;
        if put_back {
            self.put_back(agent, &back).await;
        }
        // Said once: it stays left until something changes.
        self.alert_every(
            crate::KIND_UPDATE_FAILED,
            &format!("catchup:{}:{}", agent.slug, r.digest),
            &format!("{} failed on {label}", agent.display_name),
            &text,
            30 * DAY,
        )
        .await?;
        Ok(text)
    }

    /// Where a failed agent goes back to: the rollout's `from` (the fleet's
    /// known good), or for a catch-up the digest the agent ran before.
    fn back_to(&self, r: &Rollout, agent: &ChannelAgent) -> String {
        if r.kind == RolloutKind::Catchup
            && let Some(prev) = r.prev.get(&agent.slug)
        {
            return prev.clone();
        }
        r.from.clone().unwrap_or_else(|| self.compiled())
    }

    /// Recreate an agent the step had moved on what it is assigned now (the
    /// write that put it back committed first).
    async fn put_back(&self, agent: &ChannelAgent, digest: &str) {
        let label = self.label_of(digest).await;
        match self.ops.roll(agent, true).await {
            Ok(()) => tracing::info!("[hermes] {} is back on {label}", agent.display_name),
            Err(f) => tracing::warn!(
                "[hermes] {} could not be put back on {label} ({}) — see its logs",
                agent.display_name,
                f.text()
            ),
        }
    }

    /// The image failed twice: it is bad and never rolled out again, a
    /// rollback moves every agent on it back (recreated whatever they are
    /// doing; one a person stopped is only reassigned), and the admins hear
    /// it. The ban and the rollback are one write: an admin's action that
    /// replaced this rollout first stands, and then neither happens.
    pub async fn fail_candidate(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        why: &str,
    ) -> Result<String, StateError> {
        let now = self.now();
        let label = record.label();
        let back_to = r.from.clone().unwrap_or_else(|| self.compiled());
        let back_label = self.label_of(&back_to).await;
        let text = format!(
            "{label} was not rolled out: {why}. It is marked bad and will not be tried again; every agent on it goes back to {back_label}"
        );
        tracing::warn!("[hermes] {text}");
        let back = Rollout {
            id: images::new_rollout_id(now),
            digest: back_to,
            from: Some(r.digest.clone()),
            kind: RolloutKind::Rollback,
            phase: Phase::Preparing,
            started_at: now,
            updated_at: now,
            forced: true,
            detail: Some(text.clone()),
            ..Default::default()
        };
        let unpin = r.kind == RolloutKind::Pin;
        let (id, digest, reason) = (r.id.clone(), r.digest.clone(), why.to_string());
        let note = text.clone();
        self.fenced().await?;
        self.patch_fx(move |s, fx| {
            if s.rollout.as_ref().is_none_or(|x| x.id != id || !x.open()) {
                return Err(StateError::Superseded);
            }
            fx.mark_bad(&digest, &reason);
            s.rollout = Some(back);
            if unpin {
                s.pinned = None;
            }
            s.note(now, note);
            Ok(())
        })
        .await?;
        self.ops
            .notify(
                crate::KIND_UPDATE_FAILED,
                &format!("{}:failed", r.digest),
                &format!("Agent image update to {label} failed"),
                &text,
            )
            .await;
        Ok(text)
    }

    /// Every agent runs the rollout's image (and, for one not known good, it
    /// soaked): it is good and current. An agent it did not move (busy all
    /// window, hired meanwhile) is held on the image it runs, for a catch-up
    /// later.
    async fn finish(&self, r: &Rollout, record: &ImageRecord) -> Result<String, StateError> {
        let now = self.now();
        let label = record.label();
        let state = self.load().await?;
        let agents = self.ops.agents().await.map_err(StateError::Store)?;
        let digests = self
            .effective(&state, &agents)
            .await
            .map_err(StateError::Store)?;
        let assigned = images::assignments(self.pg)
            .await
            .map_err(|e| StateError::Store(e.to_string()))?;
        let stragglers: Vec<(String, String)> = digests
            .iter()
            .filter(|(a, _)| !assigned.contains_key(&a.id))
            .map(|(a, d)| (a.id.clone(), d.clone()))
            .collect();
        // Good only once an agent ran it through the soak: an image nobody
        // ran (every agent stopped or passed over) stays a candidate, so the
        // next rollout to it still canaries and soaks it.
        let ran = !r.watch.is_empty();
        let becomes_good = watching(r, record) && ran;
        let unproven = watching(r, record) && !ran;
        let left = r.deferred.len();
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let text = match r.kind {
            RolloutKind::Rollback => format!("every agent is back on {label}"),
            RolloutKind::Catchup if r.moved.is_empty() && left > 0 => format!(
                "the {left} agent{} left behind {} still busy; {} on what {} run{}",
                plural(left),
                if left == 1 { "is" } else { "are" },
                if left == 1 { "it stays" } else { "they stay" },
                if left == 1 { "it" } else { "they" },
                if left == 1 { "s" } else { "" }
            ),
            RolloutKind::Catchup => format!("the agents left behind run {label}"),
            _ if unproven => format!(
                "{label} is current, but no agent ran it through the soak ({left} passed over), so it is not called good: the next rollout to it is watched again"
            ),
            _ if left > 0 => format!(
                "{label} is good; {left} busy agent{} moves in a later window",
                plural(left)
            ),
            _ => format!("every agent runs {label}"),
        };
        let note = text.clone();
        let kind = r.kind;
        let digest = r.digest.clone();
        self.write_fx(&r.id, move |r, s, fx| {
            for (agent, d) in &stragglers {
                fx.assign(agent, d);
            }
            if becomes_good {
                fx.mark_good(&digest);
            }
            let before = s.current.clone().unwrap_or_else(|| crate::compiled_pin().0);
            if before != digest {
                s.previous = match kind {
                    RolloutKind::Rollback => None,
                    _ => Some(before),
                };
            }
            // A catch-up failure is remembered against one image: once the
            // fleet's image is another, the agent is tried again.
            s.catchup_failed.retain(|_, d| d == &digest);
            s.current = Some(digest);
            r.phase = Phase::Done;
            r.agent = None;
            r.detail = Some(note.clone());
            s.note(now, note);
        })
        .await?;
        tracing::info!("[hermes] {text}");
        if becomes_good {
            self.say_withheld(&label).await;
        }
        Ok(text)
    }

    /// A newly good image strips env names it did not before: a per-agent
    /// secret stored under one is no longer loaded into that agent, so the
    /// admins hear which, once.
    async fn say_withheld(&self, label: &str) {
        let names = match images::stripped_env_of_good(self.pg).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("[hermes] the stripped env could not be read: {e}");
                return;
            }
        };
        let added = talaria_agent_secrets::set_image_stripped(names);
        if added.is_empty() {
            return;
        }
        let rows: Vec<(String, String)> = match sqlx::query_as(
            "select d.display_name, s.name from agent_secrets s \
             join agent_defs d on d.id = s.agent_id where s.name = any($1) \
             order by d.display_name, s.name",
        )
        .bind(&added)
        .fetch_all(self.pg)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[hermes] the withheld secrets could not be read: {e}");
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        let list = rows
            .iter()
            .map(|(agent, name)| format!("{agent}: {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = self
            .alert_once(
                crate::KIND_UPDATE_FAILED,
                &format!("withheld:{label}"),
                &format!("Hermes {label} keeps some stored secrets from agents"),
                &format!(
                    "Hermes {label} treats these names as its own provider settings, so Talaria no longer loads them into the agents' tools: {list}. Store them under another name to use them again."
                ),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_failures_wait_doubles_to_six_hours() {
        let first = 15 * MINUTE;
        assert_eq!(backoff(first, 1), 15 * MINUTE);
        assert_eq!(backoff(first, 2), 30 * MINUTE);
        assert_eq!(backoff(first, 4), 120 * MINUTE);
        assert_eq!(backoff(first, 6), BACKOFF_CAP_MS);
        assert_eq!(backoff(first, 500), BACKOFF_CAP_MS);
    }

    #[test]
    fn only_an_image_not_yet_good_here_is_watched_and_banned() {
        let rec = |status| ImageRecord {
            digest: "sha256:00".into(),
            repository: "r".into(),
            version: None,
            config_version: Some(52),
            status,
            reason: None,
            inspect: serde_json::json!({}),
            pulled_here: false,
            first_seen: 0,
            prepared_at: Some(1),
            good_since: None,
        };
        let r = |kind| Rollout {
            kind,
            ..Default::default()
        };
        assert!(watching(&r(RolloutKind::Upgrade), &rec(Status::Candidate)));
        assert!(watching(&r(RolloutKind::Pin), &rec(Status::Candidate)));
        assert!(!watching(&r(RolloutKind::Pin), &rec(Status::Good)));
        assert!(!watching(&r(RolloutKind::Catchup), &rec(Status::Good)));
        assert!(!watching(
            &r(RolloutKind::Rollback),
            &rec(Status::Candidate)
        ));
        assert!(!watching(&r(RolloutKind::Upgrade), &rec(Status::Good)));
    }

    #[test]
    fn the_minimum_age_is_clamped() {
        assert!(MIN_STABLE_HOURS_MAX as i64 * HOUR > 0, "no overflow");
    }
}
