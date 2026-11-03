// THE STATE MACHINE — one step of the channel, against any [`FleetOps`].
//
//   preparing → waiting → canary → rolling → soaking → done
//                  ↑         │         │          │
//                  └ retry ──┴─────────┴──────────┴──→ failed: a rollback
//
// THE ROW IS THE TRUTH, AND EVERY WRITE NAMES ITS ROLLOUT. Admin actions
// replace the rollout in the state row under its row lock at any moment (a
// Roll back or a Pin pressed mid-step). So the engine never acts on a
// snapshot for long: it re-reads the row and the image's status before each
// agent, and every write it makes into a rollout is refused once the row
// holds another rollout (or the rollout closed) — [`StateError::Superseded`].
// A step that is superseded stops what it was doing, re-reads, and carries on
// with whatever rollout the row holds now.
//
// NOTHING IS GOOD UNTIL IT HAS SOAKED. The canary stays up and is asked again
// for its soak (probe.rs). Each agent the rollout moves is watched — still
// running, healthy, its restart count unchanged since it came up, no new
// config fault in its log — before the next one moves (longer after the
// first), every moved agent is asked again before every move, and once the
// last has moved the whole rollout soaks before the image is called good.
//
// ONE FAILURE IS A QUESTION, TWO ARE AN ANSWER. A failure that is the
// host's (docker slow or refusing, the disk saturated) is tried again later
// and never bans. A failure that is the image's puts the agent back at once
// and is tried once more to confirm; only the second marks the image bad and
// moves every agent it reached back.

use std::time::Duration;

use sqlx::PgPool;

use talaria_hermes_images::{
    self as images, ChannelState, ImageRecord, Phase, Rollout, RolloutKind, StateError, Status,
    Watched,
};

use crate::ops::{ChannelAgent, Fault, FleetOps, Snapshot, on_channel};
use crate::prepare::Prepared;

/// The numbers the state machine runs on. [`Tuning::from_env`] is the real
/// one; the tests shrink them.
#[derive(Debug, Clone)]
pub struct Tuning {
    /// A digest must have been on stable this long (since this instance
    /// first saw it) before an Upgrade leaves Waiting on its own.
    pub min_stable_ms: i64,
    /// The first agent moved is watched this long before the next moves.
    pub first_agent_soak_ms: i64,
    /// Every later agent is watched this long before the next moves.
    pub agent_soak_ms: i64,
    /// Once every agent has moved, the whole rollout is watched this long.
    pub rollout_soak_ms: i64,
    /// After a first image failure, the retry that confirms it waits this.
    pub retry_confirm_ms: i64,
    /// After a failure that was the host's, the retry waits this.
    pub retry_infra_ms: i64,
    /// A forced rollout (Roll now) leaves an agent that stayed busy this
    /// long since it began rolling for a later window.
    pub forced_defer_ms: i64,
    /// How long one step keeps moving agents.
    pub step_budget: Duration,
}

const MINUTE: i64 = 60 * 1000;
const HOUR: i64 = 60 * MINUTE;

impl Tuning {
    /// The defaults, each overridable from the api's environment
    /// (`TALARIA_HERMES_MIN_STABLE_HOURS`, `TALARIA_HERMES_*_SOAK_SECS`).
    pub fn from_env() -> Tuning {
        let secs = |var: &str, default: i64| crate::env_secs(var, default as u64) as i64 * 1000;
        Tuning {
            min_stable_ms: crate::env_secs("TALARIA_HERMES_MIN_STABLE_HOURS", 48) as i64 * HOUR,
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

/// The engine over one state row.
pub struct Engine<'a> {
    pub pg: &'a PgPool,
    /// The `app_settings` key of the state (the channel's own is
    /// `talaria_hermes_images::STATE_KEY`; a test uses its own).
    pub key: &'a str,
    pub ops: &'a dyn FleetOps,
    pub tuning: &'a Tuning,
}

/// What one agent's turn came to.
enum Move {
    /// It runs the target now; its watch entry when it is running.
    Moved(Option<Watched>, String),
    /// Passed over for now, and why.
    Skipped(String),
    /// It failed, and whose fault that is.
    Failed(Fault),
}

/// A failure the soak found on an agent already moved.
struct Relapse {
    slug: String,
    fault: Fault,
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

    /// A write into the rollout named `id`: refused (`Superseded`) once the
    /// row holds another rollout, or this one closed.
    async fn write<T, F>(&self, id: &str, f: F) -> Result<T, StateError>
    where
        F: FnOnce(&mut Rollout, &mut ChannelState) -> T,
    {
        let now = self.now();
        self.patch(|s| {
            if s.rollout_if(id).is_none() {
                return Err(StateError::Superseded);
            }
            let mut r = s.rollout.take().expect("checked above");
            let out = f(&mut r, s);
            r.updated_at = now;
            s.rollout = Some(r);
            Ok(out)
        })
        .await
        .map(|(_, out)| out)
    }

    async fn detail(&self, id: &str, text: &str) -> Result<(), StateError> {
        let text = text.to_string();
        self.write(id, move |r, _| r.detail = Some(text)).await
    }

    /// The compiled pin's digest.
    fn compiled(&self) -> String {
        crate::compiled_pin().0
    }

    fn current(&self, state: &ChannelState) -> String {
        state.current.clone().unwrap_or_else(|| self.compiled())
    }

    /// Each agent's digest: its assignment, or the fleet's current one.
    async fn digests(
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
            match &found {
                Ok(f) => {
                    s.last_check = Some(images::CheckRecord {
                        at: now,
                        digest: Some(f.digest.clone()),
                        version: f.version.clone(),
                        error: None,
                    });
                    s.last_good_check_at = Some(now);
                    s.extra.remove("checksFailingSince");
                }
                Err(e) => {
                    // A failed check keeps what the last good one found, so
                    // a registry outage never opens or closes anything.
                    let keep = s.last_check.clone().unwrap_or_default();
                    s.last_check = Some(images::CheckRecord {
                        at: now,
                        digest: keep.digest,
                        version: keep.version,
                        error: Some(e.chars().take(500).collect()),
                    });
                    s.extra
                        .entry("checksFailingSince")
                        .or_insert_with(|| serde_json::json!(now));
                }
            }
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
        let mut due: Vec<(String, String, String)> = Vec::new();
        if let Some(since) = state
            .extra
            .get("checksFailingSince")
            .and_then(|v| v.as_i64())
            && now - since > 3 * 24 * HOUR
        {
            let error = state
                .last_check
                .as_ref()
                .and_then(|c| c.error.clone())
                .unwrap_or_default();
            due.push((
                "checks-failing".into(),
                "Agent image checks have failed for 3 days".into(),
                format!(
                    "The channel could not reach {} since {} days ago, so agents cannot follow Hermes stable: {error}",
                    crate::stable_ref(),
                    (now - since) / (24 * HOUR)
                ),
            ));
        }
        if let Some(r) = state.rollout.as_ref().filter(|r| r.open())
            && now - r.started_at > 7 * 24 * HOUR
        {
            due.push((
                format!("rollout-stuck:{}", r.id),
                "An agent image rollout has been open for a week".into(),
                format!(
                    "The rollout to {} started {} days ago and has not finished: {}",
                    images::label(None, &r.digest),
                    (now - r.started_at) / (24 * HOUR),
                    r.detail.clone().unwrap_or_default()
                ),
            ));
        }
        for (topic, title, body) in due {
            if state
                .alerted
                .get(&topic)
                .is_some_and(|at| now - at < 24 * HOUR)
            {
                continue;
            }
            self.ops
                .notify(crate::KIND_CHANNEL_STUCK, &topic, &title, &body)
                .await;
            let t = topic.clone();
            self.patch(move |s| {
                s.alerted.insert(t, now);
                Ok(())
            })
            .await?;
        }
        Ok(())
    }

    /// Open the rollout the channel calls for, when none is open (a waiting
    /// catch-up gives way to a new stable release): the pinned digest, a new
    /// stable digest that is not bad, or the fleet's own image for agents an
    /// earlier rollout left behind.
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
        if let Some(open) = state.rollout.as_ref().filter(|r| r.open()) {
            let yields = open.kind == RolloutKind::Catchup
                && open.agent.is_none()
                && state.pinned.is_none()
                && stable_ok;
            if !yields {
                return Ok(None);
            }
        }
        let agents = self.ops.agents().await?;
        let digests = self.digests(&state, &agents).await?;
        let (target, kind) = match &state.pinned {
            Some(p) if p != &current || digests.iter().any(|(_, d)| d != p) => {
                (p.clone(), RolloutKind::Pin)
            }
            Some(_) => return Ok(None),
            None if stable_ok => (stable.clone().expect("stable_ok"), RolloutKind::Upgrade),
            None if digests.iter().any(|(_, d)| d != &current) => {
                (current.clone(), RolloutKind::Catchup)
            }
            None => return Ok(None),
        };
        let Some(record) = self.record(&target).await? else {
            return Ok(None);
        };
        if record.status == Status::Bad {
            return Ok(None);
        }
        let now = self.now();
        let label = record.label();
        let replaced = state
            .rollout
            .as_ref()
            .filter(|r| r.open())
            .map(|r| r.id.clone());
        let prev = if kind == RolloutKind::Catchup {
            digests
                .iter()
                .filter(|(_, d)| d != &target)
                .map(|(a, d)| (a.slug.clone(), d.clone()))
                .collect()
        } else {
            Default::default()
        };
        let rollout = Rollout {
            prev,
            id: images::new_rollout_id(now),
            digest: target,
            from: Some(current),
            kind,
            phase: Phase::Preparing,
            started_at: now,
            updated_at: now,
            // A pin is an admin's action: it runs now.
            forced: kind == RolloutKind::Pin,
            ..Default::default()
        };
        let sentence = match kind {
            RolloutKind::Pin => format!("pinned to {label}: moving the fleet onto it"),
            RolloutKind::Catchup => {
                format!("moving the agents an earlier rollout left behind onto {label}")
            }
            _ => format!("{label} is new on stable: preparing it"),
        };
        let text = sentence.clone();
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
            s.rollout = Some(rollout);
            s.note(now, text);
            Ok(())
        })
        .await
        .map(|_| Some(sentence))
        .or_else(|e| match e {
            StateError::Superseded => Ok(None),
            e => Err(e.to_string()),
        })
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
        let now = self.now();
        let label = record.label();
        // Banned under it (an admin, or another path): its agents go back.
        if record.status == Status::Bad && r.kind != RolloutKind::Rollback {
            let why = record
                .reason
                .clone()
                .unwrap_or_else(|| "it was marked bad".into());
            said.push(self.fail_candidate(&r, &record, &why, None).await?);
            return Ok(true);
        }
        match r.phase {
            Phase::Preparing => match self.ops.prepare(&record).await {
                Prepared::Ready => {
                    let canary = crate::needs_canary(&r, &record);
                    self.write(&r.id, move |r, _| {
                        if canary {
                            r.phase = Phase::Waiting;
                        } else {
                            r.phase = Phase::Rolling;
                            r.rolling_since = Some(now);
                        }
                        r.detail = Some(format!("{label} is downloaded and inspected"));
                    })
                    .await?;
                    Ok(true)
                }
                Prepared::Pulling(p) | Prepared::Waiting(p) => {
                    self.detail(&r.id, &p).await?;
                    Ok(false)
                }
                Prepared::Failed {
                    why,
                    definitive: true,
                } => {
                    said.push(self.fail_candidate(&r, &record, &why, None).await?);
                    Ok(true)
                }
                Prepared::Failed { why, .. } => {
                    self.detail(&r.id, &format!("{why} — tried again on the next step"))
                        .await?;
                    Ok(false)
                }
            },
            Phase::Waiting => {
                if let Some(at) = r.retry_at.filter(|at| *at > now) {
                    let _ = at;
                    return Ok(false);
                }
                if r.kind == RolloutKind::Upgrade && !r.forced {
                    let age = now - record.first_seen;
                    if age < self.tuning.min_stable_ms {
                        let hours = |ms: i64| ms / HOUR;
                        self.detail(
                            &r.id,
                            &format!(
                                "{label} has been on stable for {} h; it rolls out once it has been there {} h (or on Roll now)",
                                hours(age),
                                hours(self.tuning.min_stable_ms)
                            ),
                        )
                        .await?;
                        return Ok(false);
                    }
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
                let canary = crate::needs_canary(&r, &record);
                self.write(&r.id, move |r, _| {
                    r.retry_at = None;
                    if canary {
                        r.phase = Phase::Canary;
                        r.detail = Some(format!("booting {label} as the canary"));
                    } else {
                        r.phase = Phase::Rolling;
                        r.rolling_since = Some(now);
                    }
                })
                .await?;
                Ok(true)
            }
            Phase::Canary => match self.ops.canary(&record).await {
                Ok(passed) => {
                    let _ = images::note_canary_ok(self.pg, &record.digest, now).await;
                    let text = format!("{label}: {passed}");
                    self.write(&r.id, |r, s| {
                        r.phase = Phase::Rolling;
                        r.rolling_since = Some(now);
                        r.canary_ok_at = Some(now);
                        r.detail = Some(passed.clone());
                        s.note(now, text);
                    })
                    .await?;
                    said.push(format!("{label}: the canary passed"));
                    Ok(true)
                }
                Err(fault) => {
                    said.push(self.failed(&r, &record, None, fault).await?);
                    Ok(true)
                }
            },
            Phase::Rolling => self.rolling(&state, &r, &record, started, said).await,
            Phase::Soaking => self.soaking(&r, &record, said).await,
            Phase::Done | Phase::Failed => Ok(false),
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
        // (an api restart mid-roll): verify it before anything else.
        if let Some(slug) = r.agent.clone() {
            if let Some(agent) = agents.iter().find(|a| a.slug == slug) {
                return self.resume(r, record, agent, said).await;
            }
            self.write(&r.id, |r, _| r.agent = None).await?;
        }
        // Every agent already moved is asked again first.
        match self.recheck(r, record, &agents).await? {
            Ok(()) => {}
            Err(Some(relapse)) => {
                let agent = agents.iter().find(|a| a.slug == relapse.slug).cloned();
                said.push(
                    self.failed(r, record, agent.as_ref(), relapse.fault)
                        .await?,
                );
                return Ok(true);
            }
            Err(None) => return Ok(false),
        }
        // The last agent moved soaks before the next one moves — for an
        // image not yet known good here. Going back to a known good image
        // (a rollback, a catch-up, a pin to a good version) does not wait:
        // its agents are leaving something worse.
        let soaks = crate::needs_canary(r, record);
        if soaks && let Some(last) = r.watch.last() {
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
        }
        let digests = self
            .digests(state, &agents)
            .await
            .map_err(StateError::Store)?;
        let mut pending: Vec<&ChannelAgent> = digests
            .iter()
            .map(|(a, _)| a)
            .filter(|a| !r.moved.contains(&a.slug) && !r.deferred.contains(&a.slug))
            .collect();
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
            let skipped: Vec<String> = pending
                .iter()
                .filter(|a| r.skipped.iter().any(|s| s.slug == a.slug))
                .map(|a| a.slug.clone())
                .collect();
            if !skipped.is_empty() {
                let list = skipped.join(", ");
                let s2 = skipped.clone();
                self.write(&r.id, move |r, s| {
                    r.deferred.extend(s2);
                    s.note(
                        now,
                        format!("{list}: busy all window — left for a later window"),
                    );
                })
                .await?;
                pending.retain(|a| !skipped.contains(&a.slug));
            }
        }
        if pending.is_empty() {
            if !soaks {
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
            let fresh = self.load().await?;
            if fresh
                .rollout
                .as_ref()
                .is_none_or(|x| x.id != r.id || !x.open())
            {
                return Err(StateError::Superseded);
            }
            match self.record(&r.digest).await.map_err(StateError::Store)? {
                Some(rec) if rec.status != Status::Bad || r.kind == RolloutKind::Rollback => {}
                _ => return Ok(true),
            }
            match self.move_agent(&fresh, r, record, agent).await? {
                Move::Moved(watch, how) => {
                    tracing::info!("[hermes] {} ({}): {how}", agent.display_name, agent.slug);
                    let slug = agent.slug.clone();
                    let name = agent.display_name.clone();
                    let watched = watch.is_some();
                    self.write(&r.id, move |r, _| {
                        r.moved.push(slug.clone());
                        r.skipped.retain(|k| k.slug != slug);
                        if let Some(w) = watch {
                            r.watch.retain(|k| k.slug != slug);
                            r.watch.push(w);
                        }
                        r.agent = None;
                        r.detail = Some(format!("{name}: {how}"));
                    })
                    .await?;
                    if watched && soaks {
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
                            "{name} passed over for now: {why}; it is tried again on the next step"
                        ));
                    })
                    .await?;
                }
                Move::Failed(fault) => {
                    said.push(self.failed(r, record, Some(agent), fault).await?);
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// One agent onto the rollout's image.
    async fn move_agent(
        &self,
        state: &ChannelState,
        r: &Rollout,
        record: &ImageRecord,
        agent: &ChannelAgent,
    ) -> Result<Move, StateError> {
        let label = record.label();
        let target = match self.ops.image_id(&record.image_ref()).await {
            Ok(t) => t,
            Err(e) => {
                return Ok(Move::Failed(Fault::Infra(format!(
                    "its image is not on the host: {e}"
                ))));
            }
        };
        let snap = match self.ops.snapshot(agent).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Move::Skipped(format!(
                    "docker would not describe its container ({e})"
                )));
            }
        };
        let assigned = images::assignments(self.pg)
            .await
            .map_err(|e| StateError::Store(e.to_string()))?
            .get(&agent.id)
            .cloned()
            .unwrap_or_else(|| self.current(state));
        let reached = r.reached.contains(&agent.slug);
        // Already there: no restart.
        if assigned == r.digest && snap.running && snap.image == target {
            if r.kind == RolloutKind::Rollback {
                return Ok(Move::Moved(
                    Some(self.watch_of(agent, &snap, Vec::new())),
                    format!("still on {label}"),
                ));
            }
            return Ok(match self.check_agent(agent, &target, None, &[]).await {
                Ok(w) => Move::Moved(w, format!("already on {label}")),
                Err(f) => Move::Failed(f),
            });
        }
        // Stopped by a person (and not left down by this channel's own bad
        // image): assigned, and it starts on the image next time.
        if !snap.running && !reached {
            images::assign(self.pg, &agent.id, &r.digest)
                .await
                .map_err(|e| StateError::Store(e.to_string()))?;
            self.ops.render(agent).await;
            return Ok(Move::Moved(
                None,
                format!("stopped; it starts on {label} next time"),
            ));
        }
        // An agent a failed image reached goes back whatever it is doing: it
        // is leaving something worse, and a crash-looping agent never reads
        // as idle.
        let urgent = reached && r.kind == RolloutKind::Rollback;
        if !urgent {
            match self.ops.busy(agent).await {
                Ok(Some(why)) => return Ok(Move::Skipped(why)),
                Ok(None) => {}
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
        images::assign(self.pg, &agent.id, &r.digest)
            .await
            .map_err(|e| StateError::Store(e.to_string()))?;
        let slug = agent.slug.clone();
        self.write(&r.id, move |r, _| r.agent = Some(slug)).await?;
        if let Err(fault) = self.ops.roll(agent, reached || !snap.running).await {
            return Ok(Move::Failed(fault));
        }
        Ok(match self.check_agent(agent, &target, None, &known).await {
            Ok(w) => Move::Moved(w, format!("rolled onto {label} and healthy")),
            Err(f) => Move::Failed(f),
        })
    }

    /// The agent the step before this one was moving when it stopped.
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
                self.detail(&r.id, &format!("its image is not on the host: {e}"))
                    .await?;
                return Ok(false);
            }
        };
        let snap = self.ops.snapshot(agent).await.map_err(StateError::Store)?;
        let checked = if snap.running && snap.image == target {
            self.check_agent(agent, &target, None, &[]).await
        } else if snap.running {
            // Still on the image it had: it was never recreated.
            let slug = agent.slug.clone();
            self.write(&r.id, |r, _| {
                r.agent = None;
                r.detail = Some(format!(
                    "{slug}'s interrupted move had not started; it moves next"
                ));
            })
            .await?;
            return Ok(true);
        } else {
            // Left down by the interruption: recreated now, on its
            // assignment (the target), then checked like any move.
            match self.ops.roll(agent, true).await {
                Ok(()) => self.check_agent(agent, &target, None, &[]).await,
                Err(f) => Err(f),
            }
        };
        match checked {
            Ok(watch) => {
                let slug = agent.slug.clone();
                self.write(&r.id, move |r, _| {
                    r.moved.push(slug.clone());
                    if let Some(w) = watch {
                        r.watch.retain(|k| k.slug != slug);
                        r.watch.push(w);
                    }
                    r.agent = None;
                    r.detail = Some(format!(
                        "{slug}'s interrupted move verified: it is on {label} and healthy"
                    ));
                })
                .await?;
                Ok(true)
            }
            Err(fault) => {
                said.push(self.failed(r, record, Some(agent), fault).await?);
                Ok(true)
            }
        }
    }

    fn watch_of(&self, agent: &ChannelAgent, snap: &Snapshot, known: Vec<String>) -> Watched {
        Watched {
            slug: agent.slug.clone(),
            container: snap.container.clone(),
            restart_count: snap.restart_count,
            moved_at: self.now(),
            known_faults: known,
        }
    }

    /// Ask a moved agent everything: running, healthy, on the image, its
    /// restart count unchanged since `baseline`, its API answering the
    /// gateway, its drain in place, no fault in its log beyond `known`.
    /// `Ok(None)`: it is stopped (by a person) or gone — nothing to watch.
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
            if snap.status == "restarting" || restarted > 0 {
                return Err(Fault::Image(format!(
                    "it keeps restarting ({} restarts since it came up)",
                    restarted.max(1)
                )));
            }
            // Exited and not restarted: a person stopped it.
            return Ok(None);
        }
        if snap.image != target {
            return Err(Fault::Infra(
                "its container does not run the rollout's image — HERMES_IMAGE or a chassis override is in the way".into(),
            ));
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
            Some("starting") if baseline.is_some() => {
                return Err(Fault::Image("it stopped reporting healthy".into()));
            }
            _ => {}
        }
        self.ops.answers(agent).await.map_err(Fault::Image)?;
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

    /// Ask every agent the rollout moved again. `Err(Some)`: one relapsed;
    /// `Err(None)`: docker would not answer, so not this step.
    async fn recheck(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        agents: &[ChannelAgent],
    ) -> Result<Result<(), Option<Relapse>>, StateError> {
        if r.watch.is_empty() {
            return Ok(Ok(()));
        }
        let target = match self.ops.image_id(&record.image_ref()).await {
            Ok(t) => t,
            Err(_) => return Ok(Err(None)),
        };
        let mut drop: Vec<String> = Vec::new();
        let mut moved_on: Vec<Watched> = Vec::new();
        for w in &r.watch {
            let Some(agent) = agents.iter().find(|a| a.slug == w.slug) else {
                drop.push(w.slug.clone());
                continue;
            };
            match self.check_agent(agent, &target, Some(w), &[]).await {
                Ok(Some(next)) if &next != w => moved_on.push(next),
                Ok(Some(_)) => {}
                Ok(None) => drop.push(w.slug.clone()),
                Err(Fault::Infra(_)) => return Ok(Err(None)),
                Err(fault) => {
                    return Ok(Err(Some(Relapse {
                        slug: w.slug.clone(),
                        fault,
                    })));
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
        Ok(Ok(()))
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
        let agents = self.ops.agents().await.map_err(StateError::Store)?;
        match self.recheck(r, record, &agents).await? {
            Ok(()) => {}
            Err(Some(relapse)) => {
                let agent = agents.iter().find(|a| a.slug == relapse.slug).cloned();
                said.push(
                    self.failed(r, record, agent.as_ref(), relapse.fault)
                        .await?,
                );
                return Ok(true);
            }
            Err(None) => return Ok(false),
        }
        let now = self.now();
        if r.soak_until.is_some_and(|t| now < t) {
            return Ok(false);
        }
        said.push(self.finish(r, record).await?);
        Ok(true)
    }

    // ── Failure ─────────────────────────────────────────────────────────

    /// A failure on the rollout's image: the host's is retried later; the
    /// image's puts the agent back, and is retried once to confirm before
    /// the image is marked bad.
    async fn failed(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        agent: Option<&ChannelAgent>,
        fault: Fault,
    ) -> Result<String, StateError> {
        let now = self.now();
        let label = record.label();
        let fault = match fault {
            Fault::Image(why) if self.ops.host_stalled() => Fault::Infra(format!(
                "{why} (while the host's disk was saturated, so it is not counted against the image)"
            )),
            f => f,
        };
        let who = agent
            .map(|a| format!("{}: ", a.display_name))
            .unwrap_or_default();
        // An agent that failed goes back now, whatever the verdict.
        let back_to = self.back_to(r, agent).await?;
        let put_back = match agent {
            Some(a) if r.kind != RolloutKind::Rollback => Some(self.put_back(a, &back_to).await),
            _ => None,
        };
        let slug = agent.map(|a| a.slug.clone());
        match (&fault, r.kind) {
            // Putting an agent BACK failed, or a host failure anywhere: a
            // person is told for a rollback, and it is all tried again.
            (_, RolloutKind::Rollback) | (Fault::Infra(_), _) => {
                let text = format!(
                    "{who}{}: {} — tried again in {} min",
                    if r.kind == RolloutKind::Rollback {
                        format!("could not be put back on {label}")
                    } else {
                        format!("could not be moved onto {label}")
                    },
                    fault.text(),
                    self.tuning.retry_infra_ms / MINUTE
                );
                if r.kind == RolloutKind::Rollback {
                    self.ops
                        .notify(
                            crate::KIND_UPDATE_FAILED,
                            &format!("{}:rollback", r.digest),
                            &format!("Agent image rollback to {label} is stuck"),
                            &text,
                        )
                        .await;
                }
                let retry = now + self.tuning.retry_infra_ms;
                let t = text.clone();
                let canary = r.phase == Phase::Canary;
                self.write(&r.id, move |r, s| {
                    r.retry_at = Some(retry);
                    r.agent = None;
                    if canary {
                        r.phase = Phase::Waiting;
                    }
                    s.note(now, t.clone());
                    r.detail = Some(t);
                })
                .await?;
                Ok(text)
            }
            // A catch-up's image already runs the fleet: the agent goes back
            // to what it ran and waits for a person; the image is not banned.
            (Fault::Image(why), RolloutKind::Catchup) => {
                let text = format!(
                    "{who}failed on {label} ({why}) though the rest of the fleet runs it; it is back on what it ran and left for a person"
                );
                self.ops
                    .notify(
                        crate::KIND_UPDATE_FAILED,
                        &format!("{}:catchup", r.digest),
                        &format!("An agent failed on {label}"),
                        &text,
                    )
                    .await;
                let t = text.clone();
                self.write(&r.id, move |r, s| {
                    if let Some(slug) = slug {
                        r.deferred.push(slug);
                    }
                    r.agent = None;
                    s.note(now, t.clone());
                    r.detail = Some(t);
                })
                .await?;
                Ok(text)
            }
            (Fault::Image(why), _) => {
                let why = format!("{who}{why}");
                if r.strikes == 0 {
                    let text = format!(
                        "{label}: {why}{} — tried once more in {} min to confirm before it is marked bad",
                        put_back
                            .as_deref()
                            .map(|p| format!(". {p}"))
                            .unwrap_or_default(),
                        self.tuning.retry_confirm_ms / MINUTE
                    );
                    let retry = now + self.tuning.retry_confirm_ms;
                    let t = text.clone();
                    let canary = r.phase == Phase::Canary;
                    self.write(&r.id, move |r, s| {
                        r.strikes += 1;
                        r.retry_at = Some(retry);
                        r.agent = None;
                        if let Some(slug) = slug {
                            r.moved.retain(|m| m != &slug);
                            r.watch.retain(|w| w.slug != slug);
                        }
                        if canary {
                            r.phase = Phase::Waiting;
                        } else if r.phase == Phase::Soaking {
                            r.phase = Phase::Rolling;
                        }
                        s.note(now, t.clone());
                        r.detail = Some(t);
                    })
                    .await?;
                    Ok(text)
                } else {
                    self.fail_candidate(r, record, &why, put_back.as_deref())
                        .await
                }
            }
        }
    }

    /// Where a failed agent goes back to: the rollout's `from` (the fleet's
    /// known good), or for a catch-up the digest the agent ran before.
    async fn back_to(
        &self,
        r: &Rollout,
        agent: Option<&ChannelAgent>,
    ) -> Result<String, StateError> {
        if r.kind == RolloutKind::Catchup
            && let Some(a) = agent
            && let Some(prev) = r.prev.get(&a.slug)
        {
            return Ok(prev.clone());
        }
        Ok(r.from.clone().unwrap_or_else(|| self.compiled()))
    }

    /// Put one agent back on `digest` and recreate it whatever its state.
    async fn put_back(&self, agent: &ChannelAgent, digest: &str) -> String {
        let label = match self.record(digest).await {
            Ok(Some(r)) => r.label(),
            _ => images::label(None, digest),
        };
        if let Err(e) = images::assign(self.pg, &agent.id, digest).await {
            return format!("{} could not be put back ({e})", agent.display_name);
        }
        match self.ops.roll(agent, true).await {
            Ok(()) => format!("{} is back on {label}", agent.display_name),
            Err(f) => format!(
                "{} could not be put back on {label} ({}) — see its logs",
                agent.display_name,
                f.text()
            ),
        }
    }

    /// The image failed twice: it is bad and never rolled out again, every
    /// agent it reached moves back (recreated whatever their state), and the
    /// admins hear it.
    pub async fn fail_candidate(
        &self,
        r: &Rollout,
        record: &ImageRecord,
        why: &str,
        put_back: Option<&str>,
    ) -> Result<String, StateError> {
        let now = self.now();
        let label = record.label();
        if let Err(e) = images::mark_bad(self.pg, &r.digest, why).await {
            tracing::warn!("[hermes] {label} could not be marked bad: {e}");
        }
        let back_to = r.from.clone().unwrap_or_else(|| self.compiled());
        let back_label = match self.record(&back_to).await {
            Ok(Some(b)) => b.label(),
            _ => images::label(None, &back_to),
        };
        let mut reached: Vec<String> = r.moved.clone();
        reached.extend(r.agent.clone());
        reached.sort();
        reached.dedup();
        let mut text = format!(
            "{label} was not rolled out: {why}. It is marked bad and will not be tried again; agents stay on {back_label}"
        );
        if let Some(p) = put_back {
            text.push_str(&format!(". {p}"));
        }
        if !reached.is_empty() {
            text.push_str(&format!(
                ". Moving the {} agent{} it reached back",
                reached.len(),
                if reached.len() == 1 { "" } else { "s" }
            ));
        }
        tracing::warn!("[hermes] {text}");
        self.ops
            .notify(
                crate::KIND_UPDATE_FAILED,
                &format!("{}:failed", r.digest),
                &format!("Agent image update to {label} failed"),
                &text,
            )
            .await;
        let back = Rollout {
            id: images::new_rollout_id(now),
            digest: back_to,
            from: Some(r.digest.clone()),
            kind: RolloutKind::Rollback,
            phase: Phase::Preparing,
            started_at: now,
            updated_at: now,
            forced: true,
            reached,
            detail: Some(text.clone()),
            ..Default::default()
        };
        let unpin = r.kind == RolloutKind::Pin;
        let id = r.id.clone();
        let note = text.clone();
        self.patch(move |s| {
            // Replaces this rollout only: if an admin already replaced it,
            // theirs stands.
            if s.rollout.as_ref().is_some_and(|x| x.id != id) {
                return Err(StateError::Superseded);
            }
            s.rollout = Some(back);
            if unpin {
                s.pinned = None;
            }
            s.note(now, note);
            Ok(())
        })
        .await?;
        Ok(text)
    }

    /// Every agent runs the rollout's image and it soaked: it is good and
    /// current. An agent it did not move (busy all window, hired meanwhile)
    /// is held on the image it runs, for a catch-up later.
    async fn finish(&self, r: &Rollout, record: &ImageRecord) -> Result<String, StateError> {
        let now = self.now();
        let label = record.label();
        let state = self.load().await?;
        let agents = self.ops.agents().await.map_err(StateError::Store)?;
        let digests = self
            .digests(&state, &agents)
            .await
            .map_err(StateError::Store)?;
        let assigned = images::assignments(self.pg)
            .await
            .map_err(|e| StateError::Store(e.to_string()))?;
        for (agent, digest) in &digests {
            if !r.moved.contains(&agent.slug) && !assigned.contains_key(&agent.id) {
                images::assign(self.pg, &agent.id, digest)
                    .await
                    .map_err(|e| StateError::Store(e.to_string()))?;
            }
        }
        if r.kind != RolloutKind::Rollback
            && let Err(e) = images::mark_good(self.pg, &r.digest).await
        {
            tracing::warn!("[hermes] {label} could not be marked good: {e}");
        }
        let left = r.deferred.len();
        let text = match r.kind {
            RolloutKind::Rollback => format!("every agent is back on {label}"),
            RolloutKind::Catchup => format!("the agents left behind run {label}"),
            _ if left > 0 => format!(
                "{label} is good; {left} busy agent{} moves in a later window",
                if left == 1 { "" } else { "s" }
            ),
            _ => format!("every agent runs {label}"),
        };
        let note = text.clone();
        let kind = r.kind;
        let digest = r.digest.clone();
        self.write(&r.id, move |r, s| {
            let before = s.current.clone().unwrap_or_else(|| crate::compiled_pin().0);
            if before != digest {
                s.previous = match kind {
                    RolloutKind::Rollback => None,
                    _ => Some(before),
                };
            }
            s.current = Some(digest);
            r.phase = Phase::Done;
            r.agent = None;
            r.detail = Some(note.clone());
            s.note(now, note);
        })
        .await?;
        tracing::info!("[hermes] {text}");
        Ok(text)
    }
}
