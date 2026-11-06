// THE STEP — one a minute, from the scheduler (never two at once: the job is
// leased fleet-wide). Every effect goes through [`Ops`], so the same step
// runs against the real instance and against a scripted one in the tests.
//
//   check     when due (every ~6 h, jittered; the first within 10 min of
//             boot), or on Check now: each harness's newest release, and the
//             document of every release the instance runs or pins, read
//             again each time — a release upstream deprecates or withdraws
//             after it was adopted is noticed, said to the admins, and moved
//             off. A release newer than what upstream now calls newest was
//             withdrawn, and is forgotten.
//   decide    plan::decide — one pair, or nothing.
//   re-read   the candidate's own document, just before its smoke test: one
//             deprecated or withdrawn while it soaked is never run.
//   verify    on ONE running workbench (the one whose host fails least):
//             installed beside what it runs by its own versioned paths (the
//             volume's shell `bun` untouched), started, a job's launcher laid
//             out for it and the provisioning preflight run through it. Long,
//             so it runs outside any lock, and its outcome is written in its
//             own short patch.
//   adopt     the pair passed: it is what every new job runs from now on, and
//             the pair before it is kept as the fallback; the lockfile its
//             install wrote is kept with it, so every later install of it is
//             frozen to the tree that was smoke-tested. A running job keeps
//             the install it started on until it ends. An admin's Pin or
//             Unpin made while it was being tested wins: it is not adopted.
//   hold      the candidate's own failure (not there to install, would not
//             start or not in time, its tarball not the documented one, the
//             preflight): held, the admins told, and jobs keep what they run.
//             Allow again lifts it.
//   back off  the host's or the network's failure: tried again in 15 min,
//             then twice as long each time to 6 h; the third in a row tells
//             the admins. Never held.
//   prewarm   with nothing to move, one other running workbench a step is
//             given the verified pair, so a job does not pay the install.
//             Each volume backs off on its own when its host fails it, and a
//             volume that will not take the pair is recorded (its jobs go
//             straight to the fallback) and said.
//
// Every write is a patch on a fresh read under the row's lock, so an admin's
// Pin or Allow again landing mid-step is never lost.

use futures_util::future::BoxFuture;
#[cfg(test)]
use talaria_workbench_harnesses::OMP_VERSION;
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
/// Every document a check reads, read at once, under this one bound.
pub const CHECK_BOUND_MS: u64 = 30_000;

/// A Developer Agent's workbench: its department's volume and container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workbench {
    pub department: String,
    pub agent: String,
}

/// A state change, applied to a fresh read under the row's lock.
pub type Patch<'a> = Box<dyn FnOnce(&mut State) + Send + 'a>;

/// What a smoke test is told besides the pair.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Smoke {
    /// A candidate (installed apart from what jobs resolve, and promoted by
    /// a provision once adopted), or the verified pair a prewarm installs
    /// the way a provision does.
    pub candidate: bool,
    /// Oh My Pi releases whose installs must survive it (current, fallback).
    pub keep: Vec<String>,
    /// npm's integrity for the Oh My Pi tarball.
    pub integrity: Option<String>,
}

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
    /// Install, start and preflight `v` on `w`'s volume; Ok carries the
    /// lockfile the install ran from or wrote.
    fn verify<'a>(
        &'a self,
        w: &'a Workbench,
        v: &'a Versions,
        smoke: &'a Smoke,
    ) -> BoxFuture<'a, Result<Option<String>, VerifyFailure>>;
    /// Keep pair `v`'s verified lockfile (and npm's integrity for its omp).
    fn store_lock<'a>(
        &'a self,
        v: &'a Versions,
        lock: &'a str,
        integrity: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// Drop every kept lockfile but `keep`'s pairs'.
    fn prune_locks<'a>(&'a self, keep: &'a [Versions]) -> BoxFuture<'a, ()>;
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
        .find(|l| !l.is_empty() && !l.starts_with("talaria-fault:"))
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

/// The backoff after `n` host failures in a row.
fn backoff(n: u32) -> i64 {
    let n = n.clamp(1, 12);
    (HOST_RETRY_MS << (n - 1).min(10)).min(HOST_RETRY_MAX_MS)
}

/// The Oh My Pi releases whose installs a smoke test keeps.
fn keep_of(s: &State) -> Vec<String> {
    let mut k = vec![s.running().omp];
    if let Some(p) = &s.previous
        && !k.contains(&p.omp)
    {
        k.push(p.omp.clone());
    }
    k
}

/// What the smoke test of `v` is told.
fn smoke_of(s: &State, v: &Versions) -> Smoke {
    Smoke {
        candidate: true,
        keep: keep_of(s),
        integrity: s
            .tool(Tool::Omp)
            .seen
            .get(&v.omp)
            .and_then(|x| x.integrity.clone()),
    }
}

/// Check every harness's upstream and record what it said. Returns one line
/// per harness.
pub async fn check(ops: &dyn Ops) -> Result<Vec<String>, String> {
    let now = ops.now();
    let snapshot = ops.load().await?;
    let running = snapshot.running();
    let mut lines = Vec::new();
    // Every document at once, under one overall bound, so Check now answers
    // well inside a proxy's limit: each harness's newest, and what this
    // instance runs, falls back to and pins — read again on every check, so a
    // deprecation or a withdrawal after adoption is noticed.
    let wants: Vec<(Tool, Vec<String>)> = Tool::ALL
        .iter()
        .map(|&t| {
            let mut want: Vec<String> = vec![t.of(&running).to_string()];
            if let Some(p) = &snapshot.previous {
                want.push(t.of(&p.versions()).to_string());
            }
            want.extend(snapshot.tool(t).pinned.clone());
            want.sort();
            want.dedup();
            (t, want)
        })
        .collect();
    let all = futures_util::future::join_all(wants.iter().map(|(t, want)| async move {
        let (latest, docs) = futures_util::future::join(
            ops.fetch(*t, None),
            futures_util::future::join_all(want.iter().map(|v| ops.fetch(*t, Some(v)))),
        )
        .await;
        (latest, docs)
    }));
    let Ok(all) = tokio::time::timeout(std::time::Duration::from_millis(CHECK_BOUND_MS), all).await
    else {
        let why = format!("upstream did not answer within {} s", CHECK_BOUND_MS / 1000);
        let retry = now + HOST_RETRY_MS;
        let w = why.clone();
        write(
            ops,
            Box::new(move |s: &mut State| {
                for t in Tool::ALL {
                    s.tool_mut(t).last_check = Some(crate::state::LastCheck {
                        at: now,
                        latest: None,
                        error: Some(w.clone()),
                    });
                }
                s.next_check_at = Some(retry);
            }),
        )
        .await?;
        return Err(why);
    };
    // (tool, newest, documents re-read, versions upstream answered 404 for)
    let mut found: Vec<(Tool, Result<Release, String>, Vec<Release>, Vec<String>)> = Vec::new();
    for ((t, want), (latest, docs)) in wants.into_iter().zip(all) {
        let latest = match latest {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err("upstream has no release".to_string()),
            Err(e) => Err(e),
        };
        let mut extra = Vec::new();
        let mut gone = Vec::new();
        for (v, doc) in want.into_iter().zip(docs) {
            match doc {
                Ok(Some(r)) => extra.push(r),
                Ok(None) => gone.push(v),
                Err(_) => {}
            }
        }
        lines.push(match &latest {
            Ok(r) => format!("{}: newest is {}", t.label(), r.version),
            Err(e) => format!("{}: the check failed: {e}", t.label()),
        });
        found.push((t, latest, extra, gone));
    }
    let next = now + CHECK_EVERY_MS + ops.jitter(-CHECK_JITTER_MS, CHECK_JITTER_MS);
    write(
        ops,
        Box::new(move |s: &mut State| {
            for (t, latest, extra, gone) in found {
                for r in &extra {
                    s.saw(t, r, now);
                }
                // A 404 is a withdrawal only the second time in a row.
                for v in &gone {
                    s.missed(t, v, now);
                }
                match latest {
                    Ok(r) => {
                        // Upstream's newest went back: what it no longer
                        // names (and the instance does not run or pin) is
                        // withdrawn, and never adopted.
                        let running = s.running();
                        let keep = [
                            t.of(&running).to_string(),
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
                // What jobs run went bad upstream: the admins hear it once.
                let cur = t.of(&s.running()).to_string();
                if let Some(d) = s.tool(t).seen.get(&cur).and_then(|x| x.deprecated.clone()) {
                    let title = format!("{} {cur}, which workbench jobs run, was deprecated upstream", t.label());
                    let body = format!(
                        "{} {cur}: {d}.\n\nNew jobs move off it within a minute or two, to a newer release once one is ready, else to the previous verified pair, else to the compiled one. A job already running keeps it until it ends.",
                        t.label()
                    );
                    s.alert(now, &format!("deprecated:{}:{cur}", t.slug()), &title, &body);
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
    let mut wbs = ops.workbenches().await;
    // The healthiest host first: a volume whose host keeps failing is not
    // the one every move and every prewarm tries.
    wbs.sort_by_key(|w| {
        s.volumes
            .get(&w.department)
            .map(|v| v.host_failures)
            .unwrap_or(0)
    });
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
    let joined = |said: Vec<String>| {
        if said.is_empty() {
            None
        } else {
            Some(said.join("; "))
        }
    };
    if let Some(mv) = plan.mv {
        if s.retry_at.is_some_and(|at| at > now) {
            return Ok(joined(said));
        }
        // A release that timed out once is tried on another workbench when
        // there is one: two timeouts on two hosts say more than two on one.
        // Its strikes are kept under the culprit's release (a move of both
        // may have struck either), so every release the move installs counts.
        let struck: Vec<String> = Tool::ALL
            .iter()
            .filter_map(|t| s.tool(*t).strikes.get(t.of(&mv.to)).cloned())
            .flat_map(|k| k.departments)
            .collect();
        let Some(w) = wbs
            .iter()
            .find(|w| !struck.contains(&w.department))
            .or(wbs.first())
        else {
            return Ok(joined(said));
        };
        if let Some(line) = reread(ops, &mv, &s).await? {
            said.push(line);
            return Ok(joined(said));
        }
        said.push(verify_move(ops, &mv, w, &s).await?);
        return Ok(joined(said));
    }
    // Prewarm: one workbench a step that has not taken the verified pair,
    // and is not in its own backoff.
    if let Some(cur) = s.current.clone() {
        let pair = cur.versions();
        let due = wbs.iter().find(|w| match s.volumes.get(&w.department) {
            None => true,
            Some(v) if v.retry_at.is_some_and(|at| at > now) => false,
            Some(v)
                if v.bun.as_deref() == Some(&pair.bun)
                    && v.omp.as_deref() == Some(&pair.omp)
                    && v.refused.is_none() =>
            {
                false
            }
            Some(v) => v.error.is_none() || now - v.at >= VOLUME_RETRY_MS,
        });
        if let Some(w) = due {
            said.push(prewarm(ops, w, &pair, &s).await?);
        }
    }
    Ok(joined(said))
}

/// Read the move's releases again, right before they run: one upstream
/// deprecated or withdrew while it soaked is recorded and not run, and the
/// next step decides again. `Some(line)` when the move is off.
async fn reread(ops: &dyn Ops, mv: &Move, before: &State) -> Result<Option<String>, String> {
    let running = before.running();
    for t in Tool::ALL {
        let v = t.of(&mv.to).to_string();
        if v == t.of(&running) {
            continue;
        }
        match ops.fetch(t, Some(&v)).await {
            Ok(Some(r)) if r.deprecated.is_none() => {
                // Its integrity may have been read only now.
                let now = ops.now();
                write(ops, Box::new(move |s: &mut State| s.saw(t, &r, now))).await?;
            }
            Ok(None) => {
                // One 404 is a blip: this step does not smoke-test it, and
                // the next reads it again; the second in a row withdraws it.
                let now = ops.now();
                let s = write(
                    ops,
                    Box::new(move |s: &mut State| {
                        s.missed(t, &v, now);
                    }),
                )
                .await?;
                let v = t.of(&mv.to).to_string();
                return Ok(Some(if s.is_bad(t, &v) {
                    format!(
                        "{} {v} was not smoke-tested: upstream withdrew it (two 404s in a row)",
                        t.label()
                    )
                } else {
                    format!(
                        "{} {v} was not smoke-tested this step: upstream answered 404 for it once",
                        t.label()
                    )
                }));
            }
            Ok(found) => {
                let now = ops.now();
                let why = found
                    .as_ref()
                    .and_then(|r| r.deprecated.clone())
                    .unwrap_or_else(|| "withdrawn: upstream no longer has it".into());
                let line = format!("{} {v} was not smoke-tested: {why}", t.label());
                let text = line.clone();
                let rel = Release {
                    version: v.clone(),
                    requires_bun: found.and_then(|r| r.requires_bun),
                    deprecated: Some(why),
                    integrity: None,
                };
                write(
                    ops,
                    Box::new(move |s: &mut State| {
                        s.saw(t, &rel, now);
                        s.event(now, text);
                    }),
                )
                .await?;
                return Ok(Some(line));
            }
            Err(e) => {
                return host_failed(
                    ops,
                    &format!("{} {v}", t.label()),
                    "its registry",
                    &format!("its document could not be read again: {e}"),
                )
                .await
                .map(Some);
            }
        }
    }
    Ok(None)
}

/// Which release a failed move is held against: the move's own, except
/// when it moved both and the failure is one of bun's OWN steps — a line
/// that starts `bun <v> ` or `could not install bun <v> ` (after the
/// provision's `… for the harness: ` prefix, which the first line carries).
/// An Oh My Pi step names the bun it ran on mid-line (`Oh My Pi <v> on bun
/// <b> did not finish starting …`), and that is Oh My Pi's.
fn culprit(mv: &Move, from: &Versions, why: &str) -> (Tool, String) {
    let both = mv.to.bun != from.bun && mv.to.omp != from.omp;
    let own = format!("bun {} ", mv.to.bun);
    let install = format!("could not install bun {} ", mv.to.bun);
    let names_bun = why.lines().any(|l| {
        let l = l.trim();
        let l = l
            .split_once(" for the harness: ")
            .map_or(l, |(_, rest)| rest);
        l.starts_with(&own) || l.starts_with(&install)
    });
    if both && names_bun {
        (Tool::Bun, mv.to.bun.clone())
    } else {
        (mv.tool, mv.version.clone())
    }
}

async fn verify_move(
    ops: &dyn Ops,
    mv: &Move,
    w: &Workbench,
    before: &State,
) -> Result<String, String> {
    // The state as the re-read left it (integrity included).
    let before = ops.load().await.unwrap_or_else(|_| before.clone());
    let now = ops.now();
    let from = before.running();
    let to = mv.to.clone();
    let tool = mv.tool;
    let version = mv.version.clone();
    let dept = w.department.clone();
    let agent = w.agent.clone();
    let pins = (before.tool(Tool::Bun).pinned, before.tool(Tool::Omp).pinned);
    let smoke = smoke_of(&before, &mv.to);
    match ops.verify(w, &mv.to, &smoke).await {
        Ok(lock) => {
            if let Some(lock) = &lock {
                ops.store_lock(&to, lock, smoke.integrity.as_deref())
                    .await?;
            }
            let why = mv.why.clone();
            let line = format!(
                "adopted Oh My Pi {} on bun {} ({why}; smoke-tested on {agent}'s workbench)",
                to.omp, to.bun
            );
            let text = line.clone();
            let late = format!(
                "Oh My Pi {} on bun {} passed its smoke test, but a Pin or Unpin made meanwhile wins: not adopted",
                to.omp, to.bun
            );
            let to2 = to.clone();
            let s = write(
                ops,
                Box::new(move |s: &mut State| {
                    s.volumes.insert(
                        dept,
                        Volume {
                            bun: Some(to.bun.clone()),
                            omp: Some(to.omp.clone()),
                            at: now,
                            ..Default::default()
                        },
                    );
                    s.host_failures = 0;
                    s.retry_at = None;
                    // It passed: whatever timed out before was the host's.
                    for t in Tool::ALL {
                        s.tool_mut(t).strikes.remove(t.of(&to));
                    }
                    if (s.tool(Tool::Bun).pinned, s.tool(Tool::Omp).pinned) != pins {
                        s.event(now, late);
                        return;
                    }
                    let prev = s.current.clone().unwrap_or_else(|| Pair::of(&from, 0));
                    if prev.versions() != to {
                        s.previous = Some(prev);
                    }
                    s.current = Some(Pair::of(&to, now));
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
            if s.current.as_ref().map(Pair::versions).as_ref() == Some(&to2) {
                let mut keep = vec![Versions::bootstrap()];
                keep.extend(
                    s.current
                        .iter()
                        .chain(s.previous.iter())
                        .map(Pair::versions),
                );
                ops.prune_locks(&keep).await;
                Ok(line)
            } else {
                Ok(format!(
                    "{} on bun {}: a Pin or Unpin made meanwhile wins",
                    to2.omp, to2.bun
                ))
            }
        }
        Err(VerifyFailure::Candidate(why)) => {
            let (tool, version) = culprit(mv, &from, &why);
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
                    s.hold(
                        tool,
                        &version,
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
        Err(VerifyFailure::Timeout(why)) => {
            let (tool, version) = culprit(mv, &from, &why);
            let reason = tail(&why, 1500);
            let prior = before
                .tool(tool)
                .strikes
                .get(&version)
                .cloned()
                .unwrap_or_default();
            if prior.count >= 1 {
                // The second timeout in a row, on a separate attempt: held.
                let places = {
                    let mut d = prior.departments.clone();
                    if !d.contains(&dept) {
                        d.push(dept.clone());
                    }
                    d.join(", ")
                };
                let line = format!(
                    "held {} {version}: it timed out twice in a row (on {places}): {}",
                    tool.label(),
                    first_line(&reason)
                );
                let text = line.clone();
                let title = format!("Workbench harness update held: {} {version}", tool.label());
                let body = format!(
                    "{} {version} ran past its bound twice in a row, on separate tries (workbenches: {places}), so it was not adopted:\n\n{reason}\n\nEvery job keeps running Oh My Pi {} on bun {}, and so do new jobs. Allow it again on Manage → Workbench harnesses if the host was the slow one.",
                    tool.label(),
                    from.omp,
                    from.bun
                );
                let topic = format!("held:{}:{version}", tool.slug());
                write(
                    ops,
                    Box::new(move |s: &mut State| {
                        s.tool_mut(tool).strikes.remove(&version);
                        if s.tool(tool).forced.as_deref() == Some(version.as_str()) {
                            s.tool_mut(tool).forced = None;
                        }
                        s.host_failures = 0;
                        s.hold(
                            tool,
                            &version,
                            Held {
                                reason,
                                at: now,
                                department: Some(dept),
                            },
                        );
                        s.event(now, text);
                        s.alert(now, &topic, &title, &body);
                    }),
                )
                .await?;
                return Ok(line);
            }
            let line = format!(
                "{} {version} timed out on {agent}'s workbench, once: tried again in 15 min (on another workbench when there is one); held only if it times out again",
                tool.label()
            );
            let text = line.clone();
            write(
                ops,
                Box::new(move |s: &mut State| {
                    let k = s.tool_mut(tool).strikes.entry(version).or_default();
                    k.count = k.count.saturating_add(1);
                    if !k.departments.contains(&dept) {
                        k.departments.push(dept);
                    }
                    k.at = now;
                    k.why = reason;
                    s.retry_at = Some(now + HOST_RETRY_MS);
                    s.event(now, text);
                }),
            )
            .await?;
            Ok(line)
        }
        Err(VerifyFailure::Host(why)) => {
            host_failed(
                ops,
                &format!("{} {version}", tool.label()),
                &format!("{agent}'s workbench"),
                &why,
            )
            .await
        }
    }
}

/// A move the host (or the network) failed: the channel's own backoff.
async fn host_failed(ops: &dyn Ops, what: &str, place: &str, why: &str) -> Result<String, String> {
    let now = ops.now();
    let why = tail(why, 600);
    let line = format!(
        "could not smoke-test {what} on {place} (the host's or the network's, tried again later): {}",
        first_line(&why)
    );
    let text = line.clone();
    let body = format!(
        "Talaria could not smoke-test {what} on {place} three times in a row. The host or the network, not the release: {why}\n\nJobs keep running what they run. The channel keeps trying, less often each time (up to every 6 h)."
    );
    write(
        ops,
        Box::new(move |s: &mut State| {
            s.host_failures = s.host_failures.saturating_add(1);
            s.retry_at = Some(now + backoff(s.host_failures));
            s.event(now, text);
            if s.host_failures >= 3 {
                s.alert(now, "host", "Workbench harness updates cannot run", &body);
            }
        }),
    )
    .await?;
    Ok(line)
}

async fn prewarm(
    ops: &dyn Ops,
    w: &Workbench,
    pair: &Versions,
    s: &State,
) -> Result<String, String> {
    let now = ops.now();
    let dept = w.department.clone();
    let agent = w.agent.clone();
    let p = pair.clone();
    // A verified pair: installed the way a provision does (into the shared
    // mise dir), not apart like a candidate.
    let smoke = Smoke {
        candidate: false,
        ..smoke_of(s, pair)
    };
    match ops.verify(w, pair, &smoke).await {
        Ok(_) => {
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
                            ..Default::default()
                        },
                    );
                }),
            )
            .await?;
            Ok(line)
        }
        // A verified pair that times out here is this host being slow.
        Err(VerifyFailure::Host(why) | VerifyFailure::Timeout(why)) => {
            // This volume's own backoff: the channel's moves, and the other
            // volumes' prewarms, go on.
            let why = tail(&why, 600);
            let line = format!(
                "could not prewarm {agent}'s workbench (the host's or the network's, tried again later): {}",
                first_line(&why)
            );
            let text = line.clone();
            let topic = format!("volume-host:{dept}");
            let title = format!("{agent}'s workbench cannot be prewarmed");
            let body = format!(
                "Talaria could not put Oh My Pi {} on bun {} on {agent}'s workbench three times in a row. The host or the network, not the release: {why}\n\nIts jobs install it themselves, or run the previous pair. It is tried again, less often each time (up to every 6 h).",
                pair.omp, pair.bun
            );
            write(
                ops,
                Box::new(move |s: &mut State| {
                    let v = s.volumes.entry(dept).or_default();
                    v.host_failures = v.host_failures.saturating_add(1);
                    v.retry_at = Some(now + backoff(v.host_failures));
                    let n = v.host_failures;
                    s.event(now, text);
                    if n >= 3 {
                        s.alert(now, &topic, &title, &body);
                    }
                }),
            )
            .await?;
            Ok(line)
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
                            refused: Some(Pair::of(&p, now)),
                            host_failures: 0,
                            retry_at: None,
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
        smokes: Mutex<Vec<Smoke>>,
        locks: Mutex<BTreeMap<String, (String, Option<String>)>>,
        /// Runs once, mid-smoke-test (an admin acting meanwhile).
        delay: Mutex<Option<Box<dyn FnOnce(&Fake) + Send>>>,
        /// Upstream that takes a minute to answer each document.
        slow: std::sync::atomic::AtomicBool,
    }

    impl Fake {
        fn with(dev: &[&str]) -> Fake {
            let f = Fake {
                wbs: dev
                    .iter()
                    .map(|d| Workbench {
                        department: d.to_string(),
                        agent: format!("Dev {d}"),
                    })
                    .collect(),
                ..Default::default()
            };
            // The compiled pair is a real release upstream.
            f.release(Tool::Omp, OMP_VERSION, Some(">=1.4.4"), false);
            f.release(
                Tool::Bun,
                talaria_workbench_harnesses::HARNESS_BUN,
                None,
                false,
            );
            f
        }
        fn release(&self, t: Tool, v: &str, requires: Option<&str>, latest: bool) {
            self.upstream.lock().unwrap().insert(
                (t, v.into()),
                Release {
                    version: v.into(),
                    requires_bun: requires.map(Into::into),
                    deprecated: None,
                    integrity: Some(format!("sha512-{}", v.replace('.', ""))),
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
                if self.slow.load(std::sync::atomic::Ordering::Relaxed) {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
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
            smoke: &'a Smoke,
        ) -> BoxFuture<'a, Result<Option<String>, VerifyFailure>> {
            Box::pin(async move {
                self.smokes.lock().unwrap().push(smoke.clone());
                self.verified
                    .lock()
                    .unwrap()
                    .push((w.department.clone(), v.clone()));
                if let Some(d) = self.delay.lock().unwrap().take() {
                    d(self);
                }
                match self
                    .fail
                    .lock()
                    .unwrap()
                    .get(&(v.bun.clone(), v.omp.clone()))
                {
                    Some(f) => Err(f.clone()),
                    None => Ok(Some(format!("lock-for-{}", v.omp))),
                }
            })
        }
        fn store_lock<'a>(
            &'a self,
            v: &'a Versions,
            lock: &'a str,
            integrity: Option<&'a str>,
        ) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                self.locks.lock().unwrap().insert(
                    format!("{}+{}", v.bun, v.omp),
                    (lock.to_string(), integrity.map(str::to_string)),
                );
                Ok(())
            })
        }
        fn prune_locks<'a>(&'a self, keep: &'a [Versions]) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                let keep: Vec<String> = keep
                    .iter()
                    .map(|v| format!("{}+{}", v.bun, v.omp))
                    .collect();
                self.locks.lock().unwrap().retain(|k, _| keep.contains(k));
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
        let told = f.told.lock().unwrap().clone();
        assert_eq!(told.len(), 1, "{told:?}");
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

    #[tokio::test]
    async fn a_candidate_deprecated_or_withdrawn_while_it_soaked_is_never_run() {
        for withdrawn in [false, true] {
            let f = Fake::with(&["eng"]);
            f.release(Tool::Bun, "1.4.4", None, true);
            f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
            f.state.lock().unwrap().next_check_at = Some(0);
            f.at(1_000);
            check(&f).await.unwrap();
            // Upstream changes its mind before the smoke test.
            if withdrawn {
                f.upstream
                    .lock()
                    .unwrap()
                    .remove(&(Tool::Omp, "18.17.0".to_string()));
            } else {
                f.upstream
                    .lock()
                    .unwrap()
                    .get_mut(&(Tool::Omp, "18.17.0".to_string()))
                    .unwrap()
                    .deprecated = Some("broken".into());
            }
            f.state.lock().unwrap().next_check_at = Some(i64::MAX);
            settle(&f, 2).await;
            let ran = f
                .verified
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, v)| v.omp == "18.17.0")
                .count();
            assert_eq!(ran, 0, "withdrawn={withdrawn}");
            assert!(f.s().is_bad(Tool::Omp, "18.17.0"), "withdrawn={withdrawn}");
        }
    }

    #[tokio::test]
    async fn a_current_release_deprecated_after_adoption_is_said_and_moved_off() {
        let f = Fake::with(&["eng"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.16.5", Some(">=1.4.4"), false);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        {
            let mut s = f.state.lock().unwrap();
            s.current = Some(Pair::of(&pair("1.4.4", "18.17.0"), 0));
            s.previous = Some(Pair::of(&pair("1.4.4", "18.16.5"), 0));
            s.volumes.insert(
                "eng".into(),
                Volume {
                    bun: Some("1.4.4".into()),
                    omp: Some("18.17.0".into()),
                    ..Default::default()
                },
            );
            s.next_check_at = Some(0);
        }
        f.upstream
            .lock()
            .unwrap()
            .get_mut(&(Tool::Omp, "18.17.0".to_string()))
            .unwrap()
            .deprecated = Some("leaks the key".into());
        f.at(1_000);
        settle(&f, 2).await;
        let told = f.told.lock().unwrap().clone();
        assert!(
            told.iter()
                .any(|(topic, title, _)| topic == "deprecated:omp:18.17.0"
                    && title.contains("deprecated upstream")),
            "{told:?}"
        );
        assert_eq!(
            f.s().running(),
            pair("1.4.4", "18.16.5"),
            "back to the fallback"
        );
    }

    #[tokio::test]
    async fn a_volume_whose_host_fails_backs_off_alone_and_the_others_are_prewarmed() {
        let f = Fake::with(&["eng", "ops", "web"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        {
            let mut s = f.state.lock().unwrap();
            s.current = Some(Pair::of(&pair("1.4.4", "18.17.0"), 0));
            s.volumes.insert(
                "eng".into(),
                Volume {
                    bun: Some("1.4.4".into()),
                    omp: Some("18.17.0".into()),
                    ..Default::default()
                },
            );
            s.next_check_at = Some(i64::MAX);
        }
        // ops's host fails every prewarm; web is fine.
        *f.delay.lock().unwrap() = None;
        let fail_ops = |f: &Fake| {
            f.fail.lock().unwrap().insert(
                ("1.4.4".into(), "18.17.0".into()),
                VerifyFailure::Host("docker exec … timed out".into()),
            );
        };
        fail_ops(&f);
        f.at(1_000);
        settle(&f, 1).await;
        let v = f.s().volumes["ops"].clone();
        assert_eq!(v.host_failures, 1);
        assert!(v.retry_at.unwrap() > 1_000);
        assert_eq!(
            f.s().host_failures,
            0,
            "the channel's own backoff is untouched"
        );
        assert_eq!(f.s().retry_at, None);
        // The next step goes on to web, not ops again.
        f.fail.lock().unwrap().clear();
        settle(&f, 1).await;
        assert_eq!(f.verified.lock().unwrap().last().unwrap().0, "web");
        // Once its backoff is over, ops succeeds and its count resets.
        f.at(v.retry_at.unwrap());
        settle(&f, 1).await;
        let v = f.s().volumes["ops"].clone();
        assert_eq!((v.host_failures, v.retry_at), (0, None));
        assert_eq!(v.omp.as_deref(), Some("18.17.0"));
    }

    #[tokio::test]
    async fn a_pin_made_while_a_release_is_smoke_tested_wins() {
        let f = Fake::with(&["eng"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        f.state.lock().unwrap().next_check_at = Some(0);
        f.state.lock().unwrap().current = Some(Pair::of(&Versions::bootstrap(), 0));
        *f.delay.lock().unwrap() = Some(Box::new(|f: &Fake| {
            f.state.lock().unwrap().tool_mut(Tool::Omp).pinned = Some(OMP_VERSION.into());
        }));
        f.at(1_000);
        settle(&f, 1).await;
        assert_eq!(
            f.s().running(),
            Versions::bootstrap(),
            "the admin's pin wins over the move it raced"
        );
        assert!(
            f.s()
                .events
                .iter()
                .any(|e| e.text.contains("a Pin or Unpin made meanwhile wins"))
        );
    }

    #[tokio::test]
    async fn the_lockfile_and_integrity_go_with_the_pair_and_the_smoke_test_keeps_current_and_fallback()
     {
        let f = Fake::with(&["eng"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        {
            let mut s = f.state.lock().unwrap();
            s.current = Some(Pair::of(&pair("1.4.4", "18.16.0"), 0));
            s.previous = Some(Pair::of(&pair("1.4.4", "18.15.0"), 0));
            s.next_check_at = Some(0);
        }
        f.release(Tool::Omp, "18.15.0", Some(">=1.4.4"), false);
        f.at(1_000);
        settle(&f, 1).await;
        assert_eq!(f.s().running(), pair("1.4.4", "18.17.0"));
        let smoke = f.smokes.lock().unwrap()[0].clone();
        assert_eq!(
            smoke.keep,
            vec!["18.16.0".to_string(), "18.15.0".to_string()]
        );
        assert_eq!(smoke.integrity.as_deref(), Some("sha512-18170"));
        let locks = f.locks.lock().unwrap().clone();
        assert_eq!(
            locks["1.4.4+18.17.0"],
            (
                "lock-for-18.17.0".to_string(),
                Some("sha512-18170".to_string())
            )
        );
    }

    #[tokio::test]
    async fn a_timeout_is_a_strike_held_only_on_the_second_in_a_row_on_another_workbench() {
        let f = Fake::with(&["eng", "ops"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        f.state.lock().unwrap().next_check_at = Some(0);
        f.fail.lock().unwrap().insert(
            ("1.4.4".into(), "18.17.0".into()),
            VerifyFailure::Timeout(
                "could not provision bun 1.4.4 and Oh My Pi 18.17.0 for the harness: Oh My Pi 18.17.0 on bun 1.4.4 did not finish starting within 60 s\ntalaria-fault: timeout"
                    .into(),
            ),
        );
        f.at(1_000);
        settle(&f, 1).await;
        let s = f.s();
        assert!(
            s.tool(Tool::Omp).held.is_empty(),
            "never held on the first timeout"
        );
        assert_eq!(
            s.tool(Tool::Omp).strikes["18.17.0"].departments,
            vec!["eng".to_string()]
        );
        assert!(f.told.lock().unwrap().is_empty());
        assert_eq!(s.host_failures, 0, "a strike is not a host failure");
        let again = s.retry_at.unwrap();
        // The second try goes to the other workbench, and holds it.
        f.at(again);
        settle(&f, 1).await;
        let tried: Vec<String> = f
            .verified
            .lock()
            .unwrap()
            .iter()
            .map(|(d, _)| d.clone())
            .collect();
        assert_eq!(tried, vec!["eng".to_string(), "ops".to_string()]);
        let s = f.s();
        assert!(
            s.tool(Tool::Omp).held["18.17.0"]
                .reason
                .contains("did not finish starting")
        );
        assert!(s.tool(Tool::Omp).strikes.is_empty());
        let told = f.told.lock().unwrap().clone();
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].2.contains("twice in a row"), "{told:?}");
        // A strike that then passes is cleared, and adopted.
        let f = Fake::with(&["eng"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        f.state.lock().unwrap().next_check_at = Some(0);
        f.fail.lock().unwrap().insert(
            ("1.4.4".into(), "18.17.0".into()),
            VerifyFailure::Timeout("slow\ntalaria-fault: timeout".into()),
        );
        f.at(1_000);
        settle(&f, 1).await;
        f.fail.lock().unwrap().clear();
        f.at(f.s().retry_at.unwrap());
        settle(&f, 1).await;
        assert_eq!(f.s().running(), pair("1.4.4", "18.17.0"));
        assert!(f.s().tool(Tool::Omp).strikes.is_empty());
    }

    #[tokio::test]
    async fn one_404_on_what_jobs_run_is_a_blip_and_the_second_withdraws_it() {
        let f = Fake::with(&["eng"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        {
            let mut s = f.state.lock().unwrap();
            s.current = Some(Pair::of(&pair("1.4.4", "18.16.5"), 0));
            s.previous = Some(Pair::of(&Versions::bootstrap(), 0));
        }
        f.at(1_000);
        check(&f).await.unwrap();
        assert!(!f.s().is_bad(Tool::Omp, "18.16.5"), "one 404 is a blip");
        assert!(f.told.lock().unwrap().is_empty());
        check(&f).await.unwrap();
        assert!(
            f.s().is_bad(Tool::Omp, "18.16.5"),
            "two in a row withdraw it"
        );
        // Found again: it heals.
        f.release(Tool::Omp, "18.16.5", Some(">=1.4.4"), false);
        check(&f).await.unwrap();
        assert!(!f.s().is_bad(Tool::Omp, "18.16.5"));
    }

    #[tokio::test(start_paused = true)]
    async fn check_now_answers_within_its_bound_when_upstream_does_not() {
        let f = Fake::with(&["eng"]);
        f.release(Tool::Bun, "1.4.4", None, true);
        f.release(Tool::Omp, "18.17.0", Some(">=1.4.4"), true);
        f.slow.store(true, std::sync::atomic::Ordering::Relaxed);
        let t = tokio::time::Instant::now();
        let e = check(&f).await.unwrap_err();
        assert!(e.contains("did not answer within 30 s"), "{e}");
        assert!(
            t.elapsed() < std::time::Duration::from_secs(31),
            "{:?}",
            t.elapsed()
        );
        assert!(f.s().tool(Tool::Omp).last_check.unwrap().error.is_some());
        // Every document is read at once: answering takes one fetch's time.
        f.slow.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(check(&f).await.is_ok());
    }

    #[tokio::test]
    async fn a_move_of_both_holds_the_bun_only_for_buns_own_steps() {
        // The lines the runtime script prints, as they arrive through the
        // provision's `… for the harness: ` prefix (the first line) and on.
        let from = pair("1.3.14", "18.13.2");
        let mv = Move {
            to: pair("1.4.5", "18.18.0"),
            tool: Tool::Omp,
            version: "18.18.0".into(),
            why: "both together".into(),
        };
        let p = "could not provision bun 1.4.5 and Oh My Pi 18.18.0 for the harness: ";
        for (why, held) in [
            (format!("{p}Oh My Pi 18.18.0 on bun 1.4.5 did not finish starting within 60 s\ntalaria-fault: timeout"), Tool::Omp),
            (format!("{p}Oh My Pi 18.18.0 installed but does not start on bun 1.4.5: error: x\ntalaria-fault: start"), Tool::Omp),
            ("the preflight's start timed out: the launcher's `--version` did not finish within 60 s\ntalaria-fault: timeout".to_string(), Tool::Omp),
            (format!("{p}could not install bun 1.4.5 with mise (exit 124): …\nbun 1.4.5 did not finish installing within 180 s\ntalaria-fault: timeout"), Tool::Bun),
            (format!("{p}bun 1.4.5 installed but does not start: Illegal instruction\ntalaria-fault: start"), Tool::Bun),
            (format!("{p}bun 1.4.5 did not finish starting within 60 s\ntalaria-fault: timeout"), Tool::Bun),
        ] {
            assert_eq!(culprit(&mv, &from, &why).0, held, "{why}");
        }
    }

    #[tokio::test]
    async fn a_move_of_both_whose_omp_start_hangs_holds_the_omp_on_the_second_workbench() {
        let f = Fake::with(&["eng", "ops"]);
        f.release(Tool::Bun, "1.3.14", None, false);
        f.release(Tool::Bun, "1.4.5", None, true);
        f.release(Tool::Omp, "18.13.2", Some(">=1.3.14 <1.4.5"), false);
        f.release(Tool::Omp, "18.18.0", Some(">=1.4.5"), true);
        {
            let mut s = f.state.lock().unwrap();
            s.current = Some(Pair::of(&pair("1.3.14", "18.13.2"), 0));
            s.next_check_at = Some(0);
        }
        f.fail.lock().unwrap().insert(
            ("1.4.5".into(), "18.18.0".into()),
            VerifyFailure::Timeout(
                "could not provision bun 1.4.5 and Oh My Pi 18.18.0 for the harness: Oh My Pi 18.18.0 on bun 1.4.5 did not finish starting within 60 s\ntalaria-fault: timeout".into(),
            ),
        );
        f.at(1_000);
        settle(&f, 1).await;
        assert_eq!(
            f.verified.lock().unwrap()[0],
            ("eng".to_string(), pair("1.4.5", "18.18.0")),
            "both together"
        );
        assert!(
            f.s().tool(Tool::Omp).strikes.contains_key("18.18.0"),
            "struck against Oh My Pi"
        );
        assert!(f.s().tool(Tool::Bun).strikes.is_empty());
        f.at(f.s().retry_at.unwrap());
        settle(&f, 1).await;
        assert_eq!(
            f.verified.lock().unwrap()[1].0,
            "ops",
            "the second try is on the other workbench"
        );
        let s = f.s();
        assert!(s.tool(Tool::Omp).held.contains_key("18.18.0"));
        assert!(s.tool(Tool::Bun).held.is_empty(), "the bun is not blamed");
    }

    #[tokio::test]
    async fn a_move_of_both_holds_the_bun_when_the_new_bun_is_what_failed() {
        let from = pair("1.3.14", "18.13.2");
        let mv = Move {
            to: pair("1.4.4", "18.16.0"),
            tool: Tool::Omp,
            version: "18.16.0".into(),
            why: "both together".into(),
        };
        assert_eq!(
            culprit(
                &mv,
                &from,
                "bun 1.4.4 installed but does not start: Illegal instruction\ntalaria-fault: start"
            ),
            (Tool::Bun, "1.4.4".to_string())
        );
        assert_eq!(
            culprit(
                &mv,
                &from,
                "Oh My Pi 18.16.0 installed but does not start on bun 1.4.4: …\ntalaria-fault: start"
            ),
            (Tool::Omp, "18.16.0".to_string())
        );
    }
}
