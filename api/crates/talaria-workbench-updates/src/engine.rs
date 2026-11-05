// THE STEP — one a minute, from the scheduler (never two at once: the job is
// leased fleet-wide). Every effect goes through [`Ops`], so the same step
// runs against the real instance and against a scripted one in the tests.
//
//   check     when due (every ~6 h, jittered; the first within 10 min of
//             boot), or on Check now: each harness's newest release, plus the
//             document of any release the instance runs or pins that it has
//             not read. A release newer than what upstream now calls newest
//             was withdrawn, and is forgotten.
//   decide    plan::decide — one pair, or nothing.
//   verify    on ONE running workbench (the first Developer Agent's volume):
//             install beside what it runs, start it, lay out a job's launcher
//             for it and run the provisioning preflight through that
//             launcher. Long, so it runs outside any lock, and its outcome is
//             written in its own short patch.
//   adopt     the pair passed: it is what every new job runs from now on, and
//             the pair before it is kept as the fallback. A job already
//             running keeps the install it started on until it ends.
//   hold      the candidate's own failure (would not install, start or pass
//             the preflight): held, the admins told, and jobs keep running
//             what they run. Allow again lifts it.
//   back off  the host's failure (docker, a timeout, the install lock): tried
//             again in 15 min, then twice as long each time to 6 h; the
//             third in a row tells the admins. Never held.
//   prewarm   with nothing to move, one other running workbench a step is
//             given the verified pair, so a job does not pay the install. A
//             volume that will not take it is recorded, the admins told, and
//             its jobs fall back to the previous pair.
//
// Every write is a patch on a fresh read under the row's lock, so an admin's
// Pin or Allow again landing mid-step is never lost.

use futures_util::future::BoxFuture;
use talaria_workbench_harnesses::Versions;
use talaria_workbench_mcp::devenv::VerifyFailure;

use crate::Tool;
use crate::plan::{self, Move, Tuning};
use crate::state::{Held, Pair, State, Volume};
use crate::upstream::Release;

/// How often upstream is checked, before jitter.
pub const CHECK_EVERY_MS: i64 = 6 * 60 * 60 * 1000;
/// Each check lands up to this far either side of its six hours.
pub const CHECK_JITTER_MS: i64 = 30 * 60 * 1000;
/// A fresh instance checks within this long of its first step.
pub const FIRST_CHECK_WITHIN_MS: i64 = 10 * 60 * 1000;
/// The first retry after a host failure; it doubles each time.
pub const HOST_RETRY_MS: i64 = 15 * 60 * 1000;
/// …to at most this.
pub const HOST_RETRY_MAX_MS: i64 = 6 * 60 * 60 * 1000;
/// A volume that would not take the verified pair is tried again after this.
pub const VOLUME_RETRY_MS: i64 = 6 * 60 * 60 * 1000;

/// A Developer Agent's workbench: its department's volume and container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workbench {
    pub department: String,
    pub agent: String,
}

/// A state change, applied to a fresh read under the row's lock.
pub type Patch<'a> = Box<dyn FnOnce(&mut State) + Send + 'a>;

/// Everything the step does to the world.
pub trait Ops: Send + Sync {
    fn now(&self) -> i64;
    /// A uniformly random offset in `lo..hi` ms.
    fn jitter(&self, lo: i64, hi: i64) -> i64;
    fn load(&self) -> BoxFuture<'_, Result<State, String>>;
    /// Apply `f` to the stored state and store it; the state as stored (its
    /// outbox holding the alerts `f` queued).
    fn patch<'a>(&'a self, f: Patch<'a>) -> BoxFuture<'a, Result<State, String>>;
    /// `tool`'s release `version` (`None`: the newest). `Ok(None)`: no such.
    fn fetch<'a>(
        &'a self,
        tool: Tool,
        version: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<Release>, String>>;
    /// The running Developer Agents' workbenches, oldest first.
    fn workbenches(&self) -> BoxFuture<'_, Vec<Workbench>>;
    /// Install, start and preflight `v` on `w`'s volume.
    fn verify<'a>(
        &'a self,
        w: &'a Workbench,
        v: &'a Versions,
    ) -> BoxFuture<'a, Result<(), VerifyFailure>>;
    /// Tell every admin.
    fn notify<'a>(&'a self, topic: &'a str, title: &'a str, body: &'a str) -> BoxFuture<'a, ()>;
}

/// Patch, then send whatever alerts the patch queued.
pub async fn write<'a>(ops: &'a dyn Ops, f: Patch<'a>) -> Result<State, String> {
    let mut s = ops.patch(f).await?;
    for a in std::mem::take(&mut s.outbox) {
        ops.notify(&a.topic, &a.title, &a.body).await;
    }
    Ok(s)
}

fn first_line(s: &str) -> &str {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

fn tail(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let skip = t.chars().count() - max;
    format!("…{}", t.chars().skip(skip).collect::<String>())
}

/// Check every harness's upstream and record what it said. Returns one line
/// per harness.
pub async fn check(ops: &dyn Ops) -> Result<Vec<String>, String> {
    let now = ops.now();
    let snapshot = ops.load().await?;
    let running = snapshot.running();
    let mut lines = Vec::new();
    let mut found: Vec<(Tool, Result<Release, String>, Vec<Release>)> = Vec::new();
    for t in Tool::ALL {
        let latest = match ops.fetch(t, None).await {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err("upstream has no release".to_string()),
            Err(e) => Err(e),
        };
        // The documents of what this instance runs and pins, when it has not
        // read them: their declared floors are what a move is checked against.
        let ts = snapshot.tool(t);
        let mut extra = Vec::new();
        let mut want: Vec<String> = vec![match t {
            Tool::Bun => running.bun.clone(),
            Tool::Omp => running.omp.clone(),
        }];
        want.extend(ts.pinned.clone());
        for v in want {
            if ts.seen.contains_key(&v) || latest.as_ref().is_ok_and(|r| r.version == v) {
                continue;
            }
            if let Ok(Some(r)) = ops.fetch(t, Some(&v)).await {
                extra.push(r);
            }
        }
        lines.push(match &latest {
            Ok(r) => format!("{}: newest is {}", t.label(), r.version),
            Err(e) => format!("{}: the check failed: {e}", t.label()),
        });
        found.push((t, latest, extra));
    }
    let next = now + CHECK_EVERY_MS + ops.jitter(-CHECK_JITTER_MS, CHECK_JITTER_MS);
    write(
        ops,
        Box::new(move |s: &mut State| {
            for (t, latest, extra) in found {
                for r in &extra {
                    s.saw(t, r, now);
                }
                match latest {
                    Ok(r) => {
                        // Upstream's newest went back: what it no longer
                        // names (and the instance does not run or pin) is
                        // withdrawn, and never adopted.
                        let running = s.running();
                        let keep = [
                            match t {
                                Tool::Bun => running.bun.clone(),
                                Tool::Omp => running.omp.clone(),
                            },
                            s.tool(t).pinned.unwrap_or_default(),
                        ];
                        let ts = s.tool_mut(t);
                        ts.seen.retain(|v, _| {
                            !crate::semver::newer(v, &r.version) || keep.contains(v)
                        });
                        s.saw(t, &r, now);
                        s.tool_mut(t).last_check = Some(crate::state::LastCheck {
                            at: now,
                            latest: Some(r.version.clone()),
                            error: None,
                        });
                    }
                    Err(e) => {
                        s.tool_mut(t).last_check = Some(crate::state::LastCheck {
                            at: now,
                            latest: None,
                            error: Some(tail(&e, 300)),
                        });
                    }
                }
            }
            s.next_check_at = Some(next);
        }),
    )
    .await?;
    Ok(lines)
}

/// One step. `Ok(Some(line))` when it did something worth a log line.
pub async fn step(ops: &dyn Ops, tuning: &Tuning) -> Result<Option<String>, String> {
    let now = ops.now();
    let s = ops.load().await?;
    let mut said: Vec<String> = Vec::new();
    match s.next_check_at {
        None => {
            let first = now + ops.jitter(0, FIRST_CHECK_WITHIN_MS);
            write(
                ops,
                Box::new(move |s: &mut State| s.next_check_at = Some(first)),
            )
            .await?;
            return Ok(None);
        }
        Some(at) if at <= now => said.extend(check(ops).await?),
        Some(_) => {}
    }
    let s = ops.load().await?;
    let mut plan = plan::decide(&s, now, tuning);
    let wbs = ops.workbenches().await;
    if let Some(mv) = &plan.mv
        && wbs.is_empty()
    {
        plan.details.insert(
            mv.tool.slug().to_string(),
            Some(format!(
                "{} {} is ready; it is adopted once a Developer Agent is running to smoke-test it on",
                mv.tool.label(),
                mv.version
            )),
        );
    }
    if Tool::ALL
        .iter()
        .any(|t| s.tool(*t).detail != plan.details.get(t.slug()).cloned().flatten())
    {
        let details = plan.details.clone();
        write(
            ops,
            Box::new(move |s: &mut State| {
                for t in Tool::ALL {
                    s.tool_mut(t).detail = details.get(t.slug()).cloned().flatten();
                }
            }),
        )
        .await?;
    }
    if s.retry_at.is_some_and(|at| at > now) {
        return Ok(if said.is_empty() {
            None
        } else {
            Some(said.join("; "))
        });
    }
    if let Some(mv) = plan.mv {
        let Some(w) = wbs.first() else {
            return Ok(None);
        };
        said.push(verify_move(ops, &mv, w, &s).await?);
        return Ok(Some(said.join("; ")));
    }
    // Prewarm: one workbench a step that has not taken the verified pair.
    if let Some(cur) = s.current.clone() {
        let pair = cur.versions();
        let due = wbs.iter().find(|w| match s.volumes.get(&w.department) {
            None => true,
            Some(v)
                if v.bun.as_deref() == Some(&pair.bun) && v.omp.as_deref() == Some(&pair.omp) =>
            {
                false
            }
            Some(v) => v.error.is_none() || now - v.at >= VOLUME_RETRY_MS,
        });
        if let Some(w) = due {
            said.push(prewarm(ops, w, &pair).await?);
        }
    }
    Ok(if said.is_empty() {
        None
    } else {
        Some(said.join("; "))
    })
}

async fn verify_move(
    ops: &dyn Ops,
    mv: &Move,
    w: &Workbench,
    before: &State,
) -> Result<String, String> {
    let now = ops.now();
    let from = before.running();
    let to = mv.to.clone();
    let tool = mv.tool;
    let version = mv.version.clone();
    let dept = w.department.clone();
    let agent = w.agent.clone();
    match ops.verify(w, &mv.to).await {
        Ok(()) => {
            let why = mv.why.clone();
            let line = format!(
                "adopted Oh My Pi {} on bun {} ({why}; smoke-tested on {agent}'s workbench)",
                to.omp, to.bun
            );
            let text = line.clone();
            write(
                ops,
                Box::new(move |s: &mut State| {
                    let prev = s.current.clone().unwrap_or_else(|| Pair::of(&from, 0));
                    if prev.versions() != to {
                        s.previous = Some(prev);
                    }
                    s.current = Some(Pair::of(&to, now));
                    s.volumes.insert(
                        dept,
                        Volume {
                            bun: Some(to.bun.clone()),
                            omp: Some(to.omp.clone()),
                            at: now,
                            error: None,
                        },
                    );
                    s.host_failures = 0;
                    s.retry_at = None;
                    for t in Tool::ALL {
                        let ts = s.tool_mut(t);
                        if ts.forced.as_deref() == Some(version.as_str()) {
                            ts.forced = None;
                        }
                    }
                    s.tool_mut(tool).detail = None;
                    s.event(now, text);
                }),
            )
            .await?;
            Ok(line)
        }
        Err(VerifyFailure::Candidate(why)) => {
            let reason = tail(&why, 1500);
            let line = format!(
                "held {} {version}: it failed its smoke test on {agent}'s workbench: {}",
                tool.label(),
                first_line(&reason)
            );
            let text = line.clone();
            let title = format!("Workbench harness update held: {} {version}", tool.label());
            let body = format!(
                "{} {version} failed its smoke test on {agent}'s workbench, so it was not adopted:\n\n{reason}\n\nEvery job keeps running Oh My Pi {} on bun {}, and so do new jobs. Allow it again on Manage → Workbench harnesses once the cause is fixed, or pin another version.",
                tool.label(),
                from.omp,
                from.bun
            );
            let topic = format!("held:{}:{version}", tool.slug());
            write(
                ops,
                Box::new(move |s: &mut State| {
                    s.tool_mut(tool).held.insert(
                        version.clone(),
                        Held {
                            reason,
                            at: now,
                            department: Some(dept),
                        },
                    );
                    if s.tool(tool).forced.as_deref() == Some(version.as_str()) {
                        s.tool_mut(tool).forced = None;
                    }
                    s.host_failures = 0;
                    s.event(now, text);
                    s.alert(now, &topic, &title, &body);
                }),
            )
            .await?;
            Ok(line)
        }
        Err(VerifyFailure::Host(why)) => {
            host_failed(ops, &format!("{} {version}", tool.label()), &agent, &why).await
        }
    }
}

async fn host_failed(ops: &dyn Ops, what: &str, agent: &str, why: &str) -> Result<String, String> {
    let now = ops.now();
    let why = tail(why, 600);
    let line = format!(
        "could not smoke-test {what} on {agent}'s workbench (the host's, tried again later): {}",
        first_line(&why)
    );
    let text = line.clone();
    let body = format!(
        "Talaria could not smoke-test {what} on {agent}'s workbench three times in a row. The host, not the release: {why}\n\nJobs keep running what they run. The channel keeps trying, less often each time (up to every 6 h)."
    );
    write(
        ops,
        Box::new(move |s: &mut State| {
            s.host_failures = s.host_failures.saturating_add(1);
            let n = s.host_failures.min(12);
            let wait = (HOST_RETRY_MS << (n - 1).min(10)).min(HOST_RETRY_MAX_MS);
            s.retry_at = Some(now + wait);
            s.event(now, text);
            if s.host_failures >= 3 {
                s.alert(now, "host", "Workbench harness updates cannot run", &body);
            }
        }),
    )
    .await?;
    Ok(line)
}

async fn prewarm(ops: &dyn Ops, w: &Workbench, pair: &Versions) -> Result<String, String> {
    let now = ops.now();
    let dept = w.department.clone();
    let agent = w.agent.clone();
    let p = pair.clone();
    match ops.verify(w, pair).await {
        Ok(()) => {
            let line = format!(
                "{agent}'s workbench has Oh My Pi {} on bun {}",
                pair.omp, pair.bun
            );
            write(
                ops,
                Box::new(move |s: &mut State| {
                    s.volumes.insert(
                        dept,
                        Volume {
                            bun: Some(p.bun),
                            omp: Some(p.omp),
                            at: now,
                            error: None,
                        },
                    );
                }),
            )
            .await?;
            Ok(line)
        }
        Err(VerifyFailure::Host(why)) => {
            host_failed(
                ops,
                &format!("Oh My Pi {} on bun {}", pair.omp, pair.bun),
                &agent,
                &why,
            )
            .await
        }
        Err(VerifyFailure::Candidate(why)) => {
            let reason = tail(&why, 1500);
            let line = format!(
                "{agent}'s workbench would not take Oh My Pi {} on bun {}: {}",
                pair.omp,
                pair.bun,
                first_line(&reason)
            );
            let text = line.clone();
            let topic = format!("volume:{dept}:{}+{}", pair.bun, pair.omp);
            let title = format!("{agent}'s workbench would not take Oh My Pi {}", pair.omp);
            let body = format!(
                "Oh My Pi {} on bun {} passed its smoke test on another workbench but not on {agent}'s:\n\n{reason}\n\nThat agent's jobs run the previous verified pair until it does. It is tried again every 6 h.",
                pair.omp, pair.bun
            );
            write(
                ops,
                Box::new(move |s: &mut State| {
                    let prior = s.volumes.get(&dept).cloned().unwrap_or_default();
                    s.volumes.insert(
                        dept,
                        Volume {
                            error: Some(reason),
                            at: now,
                            ..prior
                        },
                    );
                    s.event(now, text);
                    s.alert(now, &topic, &title, &body);
                }),
            )
            .await?;
            Ok(line)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A scripted instance: upstream's answers, the workbenches, and what
    /// each pair does when it is smoke-tested.
    #[derive(Default)]
    struct Fake {
        now: Mutex<i64>,
        state: Mutex<State>,
        upstream: Mutex<BTreeMap<(Tool, String), Release>>,
        latest: Mutex<BTreeMap<Tool, String>>,
        wbs: Vec<Workbench>,
        fail: Mutex<BTreeMap<(String, String), VerifyFailure>>,
        verified: Mutex<Vec<(String, Versions)>>,
        told: Mutex<Vec<(String, String, String)>>,
    }

    impl Fake {
        fn with(dev: &[&str]) -> Fake {
            Fake {
                wbs: dev
                    .iter()
                    .map(|d| Workbench {
                        department: d.to_string(),
                        agent: format!("Dev {d}"),
                    })
                    .collect(),
                ..Default::default()
            }
        }
        fn release(&self, t: Tool, v: &str, requires: Option<&str>, latest: bool) {
            self.upstream.lock().unwrap().insert(
                (t, v.into()),
                Release {
                    version: v.into(),
                    requires_bun: requires.map(Into::into),
                    deprecated: None,
                },
            );
            if latest {
                self.latest.lock().unwrap().insert(t, v.into());
            }
        }
        fn at(&self, ms: i64) {
            *self.now.lock().unwrap() = ms;
        }
        fn s(&self) -> State {
            self.state.lock().unwrap().clone()
        }
    }

    impl Ops for Fake {
        fn now(&self) -> i64 {
            *self.now.lock().unwrap()
        }
        fn jitter(&self, lo: i64, _hi: i64) -> i64 {
            lo
        }
        fn load(&self) -> BoxFuture<'_, Result<State, String>> {
            Box::pin(async move { Ok(self.s()) })
        }
        fn patch<'a>(&'a self, f: Patch<'a>) -> BoxFuture<'a, Result<State, String>> {
            Box::pin(async move {
                let mut s = self.state.lock().unwrap();
                f(&mut s);
                let out = s.clone();
                s.outbox.clear();
                Ok(out)
            })
        }
        fn fetch<'a>(
            &'a self,
            tool: Tool,
            version: Option<&'a str>,
        ) -> BoxFuture<'a, Result<Option<Release>, String>> {
            Box::pin(async move {
                let v = match version {
                    Some(v) => v.to_string(),
                    None => match self.latest.lock().unwrap().get(&tool) {
                        Some(v) => v.clone(),
                        None => return Err("unreachable".into()),
                    },
                };
                Ok(self.upstream.lock().unwrap().get(&(tool, v)).cloned())
            })
        }
        fn workbenches(&self) -> BoxFuture<'_, Vec<Workbench>> {
            Box::pin(async move { self.wbs.clone() })
        }
        fn verify<'a>(
            &'a self,
            w: &'a Workbench,
            v: &'a Versions,
        ) -> BoxFuture<'a, Result<(), VerifyFailure>> {
            Box::pin(async move {
                self.verified
                    .lock()
                    .unwrap()
                    .push((w.department.clone(), v.clone()));
                match self
                    .fail
                    .lock()
                    .unwrap()
                    .get(&(v.bun.clone(), v.omp.clone()))
                {
                    Some(f) => Err(f.clone()),
                    None => Ok(()),
                }
            })
        }
        fn notify<'a>(
            &'a self,
            topic: &'a str,
            title: &'a str,
            body: &'a str,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.told
                    .lock()
                    .unwrap()
                    .push((topic.into(), title.into(), body.into()));
            })
        }
    }

    const NOW: Tuning = Tuning { min_age_ms: 0 };

    fn pair(bun: &str, omp: &str) -> Versions {
        Versions {
            bun: bun.into(),
            omp: omp.into(),
        }
    }

    /// Steps until nothing more happens (at most `n`).
    async fn settle(f: &Fake, n: usize) {
        for _ in 0..n {
            step(f, &NOW).await.unwrap();
        }
    }

    #[tokio::test]
    async fn an_instance_on_an_older_omp_moves_to_the_newer_verified_one() {
        let f = Fake::with(&["eng"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.16.0", Some(">=1.4.4"), false);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        // First step schedules the first check; the next one runs it.
        f.at(1_000);
        step(&f, &NOW).await.unwrap();
        assert!(f.s().next_check_at.is_some() && f.s().current.is_none());
        settle(&f, 2).await;
        let s = f.s();
        assert_eq!(s.running(), pair("1.4.4", "18.17.0"));
        assert_eq!(
            s.previous.as_ref().map(Pair::versions),
            Some(Versions::bootstrap()),
            "the bootstrap pair is the fallback"
        );
        assert_eq!(
            f.verified.lock().unwrap()[0],
            ("eng".into(), pair("1.4.4", "18.17.0"))
        );
        // What provisioning reads is the verified pair.
        let stored = serde_json::to_value(&s).unwrap();
        let e = talaria_workbench_harnesses::effective_from(Some(&stored));
        assert_eq!((e.target, e.source), (pair("1.4.4", "18.17.0"), "verified"));
        assert!(f.told.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_floor_above_the_harness_bun_updates_the_bun_first() {
        let f = Fake::with(&["eng"]);
        f.state.lock().unwrap().current = Some(Pair::of(&pair("1.3.14", "18.13.2"), 0));
        f.release(Tool::Bun, "1.3.14", None, false);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.13.2", Some(">=1.3.14"), false);
        f.release(Tool::Omp, "18.16.0", Some(">=1.4.4"), true);
        f.state.lock().unwrap().next_check_at = Some(0);
        f.at(1_000);
        settle(&f, 3).await;
        let v = f.verified.lock().unwrap().clone();
        assert_eq!(
            v[..2],
            [
                ("eng".to_string(), pair("1.4.4", "18.13.2")),
                ("eng".to_string(), pair("1.4.4", "18.16.0"))
            ],
            "the bun moves first, smoke-tested with the running Oh My Pi; then Oh My Pi"
        );
        assert_eq!(f.s().running(), pair("1.4.4", "18.16.0"));
        assert_eq!(f.s().previous.unwrap().versions(), pair("1.4.4", "18.13.2"));
    }

    #[tokio::test]
    async fn a_candidate_that_fails_its_smoke_test_is_held_and_the_admins_are_told() {
        let f = Fake::with(&["eng"]);
        f.state.lock().unwrap().current = Some(Pair::of(&pair("1.3.14", "18.13.2"), 0));
        f.release(Tool::Bun, "1.3.14", None, true);
        f.release(Tool::Omp, "18.13.2", Some(">=1.3.14"), false);
        // Declares a floor its own start contradicts (the 28 Oct shape).
        f.release(Tool::Omp, "18.16.0", Some(">=1.3.14"), true);
        f.fail.lock().unwrap().insert(
            ("1.3.14".into(), "18.16.0".into()),
            VerifyFailure::Candidate(
                "Oh My Pi 18.16.0 installed but does not start on bun 1.3.14".into(),
            ),
        );
        f.state.lock().unwrap().next_check_at = Some(0);
        f.at(1_000);
        settle(&f, 3).await;
        let s = f.s();
        assert_eq!(
            s.running(),
            pair("1.3.14", "18.13.2"),
            "jobs keep what they run"
        );
        let held = &s.tool(Tool::Omp).held["18.16.0"];
        assert!(held.reason.contains("does not start on bun 1.3.14"));
        let told = f.told.lock().unwrap().clone();
        assert_eq!(told.len(), 1, "told once, not every step: {told:?}");
        assert_eq!(told[0].0, "held:omp:18.16.0");
        assert!(told[0].1.contains("held: Oh My Pi 18.16.0"));
        assert!(
            told[0]
                .2
                .contains("keeps running Oh My Pi 18.13.2 on bun 1.3.14")
        );
        let tries = |f: &Fake| {
            f.verified
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, v)| v.omp == "18.16.0")
                .count()
        };
        assert_eq!(tries(&f), 1, "a held release is not tried again");
        // Allow again: it is tried once more.
        f.state.lock().unwrap().tool_mut(Tool::Omp).held.clear();
        f.fail.lock().unwrap().clear();
        settle(&f, 1).await;
        assert_eq!(f.s().running(), pair("1.3.14", "18.16.0"));
    }

    #[tokio::test]
    async fn a_host_failure_backs_off_and_never_holds() {
        let f = Fake::with(&["eng"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        f.fail.lock().unwrap().insert(
            ("1.4.4".into(), "18.17.0".into()),
            VerifyFailure::Host("docker exec … timed out".into()),
        );
        f.state.lock().unwrap().next_check_at = Some(i64::MAX);
        f.state.lock().unwrap().tools.clear();
        // Seed what a check would have seen.
        {
            let mut s = f.state.lock().unwrap();
            let r = f.upstream.lock().unwrap()[&(Tool::Omp, "18.17.0".to_string())].clone();
            s.saw(Tool::Omp, &r, 0);
        }
        let mut t = 1_000;
        for _ in 0..3 {
            f.at(t);
            step(&f, &NOW).await.unwrap();
            t = f.s().retry_at.unwrap();
        }
        let s = f.s();
        assert_eq!(s.host_failures, 3);
        assert!(
            s.tool(Tool::Omp).held.is_empty(),
            "never held for the host's failure"
        );
        assert_eq!(
            f.told.lock().unwrap().len(),
            1,
            "the third in a row is said"
        );
        // Inside the backoff, nothing is tried.
        let n = f.verified.lock().unwrap().len();
        f.at(t - 1);
        step(&f, &NOW).await.unwrap();
        assert_eq!(f.verified.lock().unwrap().len(), n);
    }

    #[tokio::test]
    async fn with_no_workbench_running_nothing_is_adopted_unverified() {
        let f = Fake::with(&[]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        f.state.lock().unwrap().next_check_at = Some(0);
        f.at(1_000);
        settle(&f, 2).await;
        assert_eq!(f.s().current, None);
        assert!(
            f.s()
                .tool(Tool::Omp)
                .detail
                .unwrap()
                .contains("once a Developer Agent is running")
        );
    }

    #[tokio::test]
    async fn other_workbenches_are_prewarmed_and_one_that_will_not_take_it_is_said() {
        let f = Fake::with(&["eng", "ops"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        f.state.lock().unwrap().next_check_at = Some(0);
        f.at(1_000);
        settle(&f, 2).await;
        assert_eq!(f.s().running(), pair("1.4.4", "18.17.0"));
        settle(&f, 1).await;
        assert_eq!(
            f.verified.lock().unwrap().last().unwrap(),
            &("ops".to_string(), pair("1.4.4", "18.17.0"))
        );
        assert!(f.s().volumes["ops"].error.is_none());
        // A volume that will not take it: recorded and said, once.
        f.state.lock().unwrap().volumes.remove("ops");
        f.fail.lock().unwrap().insert(
            ("1.4.4".into(), "18.17.0".into()),
            VerifyFailure::Candidate(
                "could not install Oh My Pi 18.17.0 (bun install, above)".into(),
            ),
        );
        settle(&f, 3).await;
        assert!(f.s().volumes["ops"].error.is_some());
        assert_eq!(f.told.lock().unwrap().len(), 1);
        assert_eq!(
            f.s().running(),
            pair("1.4.4", "18.17.0"),
            "the instance stays on it"
        );
    }

    #[tokio::test]
    async fn a_release_upstream_withdraws_is_forgotten_before_it_is_adopted() {
        let f = Fake::with(&["eng"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        f.state.lock().unwrap().next_check_at = Some(0);
        f.at(1_000);
        check(&f).await.unwrap();
        assert!(f.s().tool(Tool::Omp).seen.contains_key("18.17.0"));
        f.release(Tool::Omp, "18.16.0", Some(">=1.4.4"), true);
        check(&f).await.unwrap();
        assert!(!f.s().tool(Tool::Omp).seen.contains_key("18.17.0"));
    }
}
