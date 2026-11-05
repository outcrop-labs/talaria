// THE WORKBENCH HARNESSES FOLLOW THEIR UPSTREAM RELEASES.
//
// Every harness Talaria installs into a Developer Agent's workbench is kept
// current on every instance, automatically, the way the agent image follows
// Hermes' stable channel (talaria-hermes-channel; docs/UPDATES.md):
//
//   Oh My Pi (omp)   the coding harness, `@oh-my-pi/pi-coding-agent` on npm
//   bun              the harness's OWN bun, which omp runs on (never the
//                    repo's: a repo's mise.toml decides that one)
//
// That is the whole list. mise is the volume's toolchain manager, installed
// once when missing; every repo's toolchain shims run through it and are
// shared by every job on the volume, so it cannot move under one job and not
// another. agent-browser follows the Hermes image's own lock, so the agent
// image channel moves it.
//
// Why: a job used to run `npx …@latest`, and omp's 28 Oct release raised its
// bun floor above the bun the harness had, so every job broke at start. #706
// pinned omp; this keeps the pin moving without a Talaria release, and never
// lets a job meet a release nobody has started first.
//
//   check    every ~6 h (jittered), upstream's newest release (engine.rs,
//            upstream.rs). Each release soaks `TALARIA_HARNESS_MIN_AGE_HOURS`
//            (default 24) from the first check that saw it.
//   decide   one pair a step (plan.rs): an Oh My Pi whose `engines.bun` is
//            above the harness's bun moves the bun first.
//   verify   on one running workbench: installed beside what it runs (#706's
//            versioned dir and atomic symlink swap), started, and the
//            provisioning preflight run through a job's own launcher.
//   adopt    the pair every new job runs (`current`), with the one before as
//            the fallback (`previous`). A running job keeps its install until
//            it ends; the runtime script's prune keeps what a job holds.
//   hold     a release that fails is held and the admins are told; jobs keep
//            what they run. Allow again lifts it.
//
// A JOB NEVER RESOLVES A VERSION. Provisioning installs what the channel
// verified, else the compiled bootstrap (`OMP_VERSION`, `HARNESS_BUN` in
// talaria-workbench-harnesses), and asks no registry
// (`talaria_workbench_harnesses::effective`).
//
// The admins' surface is `GET/POST /api/workbench/harnesses` and the panel
// Manage → Workbench harnesses: the current, candidate and held versions per
// harness; Check now, Update now, Pin, Unpin, Allow again and Reset.

pub mod engine;
pub mod ops;
pub mod plan;
pub mod semver;
pub mod state;
pub mod upstream;

use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_scheduler::{JobName, JobSpec};
use talaria_workbench_harnesses::{OMP_PACKAGE, Versions, plain_version};

pub use engine::{Ops, Workbench};
pub use ops::Live;
pub use plan::Tuning;
pub use state::State;

/// The notification kind (talaria-notify routes it to the admins'
/// workspace-health class, beside the agent image's).
pub const KIND: &str = "workbench_harness_update_failed";
/// Where the notification opens: the Manage overview carries the panel.
pub const HREF: &str = "/manage";
/// The scheduler's beat for the step.
pub const STEP_EVERY_MS: u64 = 60_000;
/// The soak's default, in hours.
pub const MIN_AGE_HOURS_DEFAULT: u64 = 24;
const LOG: &str = "[workbench-harness]";

/// A harness the channel keeps current.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tool {
    Bun,
    Omp,
}

impl Tool {
    /// The order the panel lists them: the coding harness, then its bun.
    pub const ALL: [Tool; 2] = [Tool::Omp, Tool::Bun];

    pub fn slug(self) -> &'static str {
        match self {
            Tool::Bun => "bun",
            Tool::Omp => "omp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Bun => "bun",
            Tool::Omp => "Oh My Pi",
        }
    }

    pub fn parse(s: &str) -> Option<Tool> {
        match s {
            "bun" => Some(Tool::Bun),
            "omp" => Some(Tool::Omp),
            _ => None,
        }
    }

    /// The compiled bootstrap fallback.
    pub fn bootstrap(self) -> &'static str {
        match self {
            Tool::Bun => talaria_workbench_harnesses::HARNESS_BUN,
            Tool::Omp => talaria_workbench_harnesses::OMP_VERSION,
        }
    }

    /// Where its releases come from, as the panel says it.
    pub fn source(self) -> String {
        match self {
            Tool::Bun => format!("GitHub releases ({})", upstream::bun_releases()),
            Tool::Omp => format!("npm ({OMP_PACKAGE} on {})", upstream::npm_registry()),
        }
    }

    pub fn of(self, v: &Versions) -> &str {
        match self {
            Tool::Bun => &v.bun,
            Tool::Omp => &v.omp,
        }
    }
}

/// The soak from the environment.
pub fn tuning() -> Tuning {
    let hours = std::env::var("TALARIA_HARNESS_MIN_AGE_HOURS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(MIN_AGE_HOURS_DEFAULT);
    Tuning {
        min_age_ms: (hours.min(24 * 365) * 60 * 60 * 1000) as i64,
    }
}

// ── The store ───────────────────────────────────────────────────────────────

/// Read the state row.
pub async fn load(pg: &PgPool) -> Result<State, String> {
    let stored: Option<Value> = sqlx::query_scalar("select value from app_settings where key = $1")
        .bind(talaria_workbench_harnesses::CHANNEL_STATE_KEY)
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("the workbench harness state could not be read: {e}"))?;
    state::parse_state(stored)
}

/// Apply `f` to the stored state under the row's lock.
pub async fn patch(pg: &PgPool, f: engine::Patch<'_>) -> Result<State, String> {
    let key = talaria_workbench_harnesses::CHANNEL_STATE_KEY;
    let db = |e: sqlx::Error| format!("the workbench harness state could not be written: {e}");
    let mut tx = pg.begin().await.map_err(db)?;
    sqlx::query(
        "insert into app_settings (key, value) values ($1, 'null'::jsonb) on conflict (key) do nothing",
    )
    .bind(key)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    let stored: Option<Value> =
        sqlx::query_scalar("select value from app_settings where key = $1 for update")
            .bind(key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
    let mut s = state::parse_state(stored)?;
    f(&mut s);
    let value = serde_json::to_value(&s).map_err(|e| e.to_string())?;
    sqlx::query("update app_settings set value = $2, updated_at = now() where key = $1")
        .bind(key)
        .bind(value)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(s)
}

// ── The step and the job ────────────────────────────────────────────────────

/// One step against the live instance.
pub async fn step(ops: &Live) -> Result<Option<String>, String> {
    engine::step(ops, &tuning()).await
}

/// The step on the scheduler's minute beat. Optional (not boot-required):
/// its absence leaves every job on what it runs, the bootstrap included.
pub fn job_spec(ops: Arc<Live>) -> JobSpec {
    JobSpec {
        name: JobName::WorkbenchHarnessChannel,
        every_ms: STEP_EVERY_MS,
        first_run_delay_ms: Some(120_000),
        // A smoke test can download bun and Oh My Pi (the runtime script's
        // own budget is 8 min) and then preflight; a prewarm another.
        max_run_ms: Some(30 * 60_000),
        per_instance: false,
        run: Arc::new(move || {
            let ops = ops.clone();
            Box::pin(async move {
                let out = step(&ops).await;
                if let Err(e) = &out {
                    tracing::warn!("{LOG} step: {e}");
                }
                out
            })
        }),
    }
}

/// Register the job.
pub fn register_job(ops: Arc<Live>) {
    talaria_scheduler::register_job(job_spec(ops));
}

// ── The admins' actions ─────────────────────────────────────────────────────

fn tool_of(slug: &str) -> Result<Tool, String> {
    Tool::parse(slug).ok_or_else(|| format!("unknown harness {slug:?} (omp or bun)"))
}

/// Check every harness's upstream now.
pub async fn check_now(ops: &dyn Ops) -> Result<String, String> {
    Ok(engine::check(ops).await?.join("; "))
}

/// Hold every job on `version` of `tool`, up or down, until Unpin. It must
/// exist upstream; one that never ran here is smoke-tested first.
pub async fn pin(ops: &dyn Ops, tool: &str, version: &str, by: &str) -> Result<String, String> {
    let t = tool_of(tool)?;
    if !plain_version(version) {
        return Err(format!("{version:?} is not a plain release (x.y.z)"));
    }
    let s = ops.load().await?;
    if let Some(h) = s.tool(t).held.get(version) {
        return Err(format!(
            "{} {version} is held ({}): allow it again first",
            t.label(),
            h.reason.lines().next().unwrap_or("")
        ));
    }
    let rel = ops
        .fetch(t, Some(version))
        .await?
        .ok_or_else(|| format!("upstream has no {} {version}", t.label()))?;
    let now = ops.now();
    let (by, v) = (by.to_string(), version.to_string());
    engine::write(
        ops,
        Box::new(move |s: &mut State| {
            s.saw(t, &rel, now);
            let ts = s.tool_mut(t);
            ts.pinned = Some(v.clone());
            ts.forced = None;
            s.event(now, format!("{by} pinned {} to {v}", t.label()));
        }),
    )
    .await?;
    Ok(format!(
        "{} is pinned to {version}; it is smoke-tested and adopted within a minute or two",
        t.label()
    ))
}

/// Follow upstream again.
pub async fn unpin(ops: &dyn Ops, tool: &str, by: &str) -> Result<String, String> {
    let t = tool_of(tool)?;
    let now = ops.now();
    let by = by.to_string();
    engine::write(
        ops,
        Box::new(move |s: &mut State| {
            s.tool_mut(t).pinned = None;
            s.event(
                now,
                format!("{by} unpinned {}: it follows upstream again", t.label()),
            );
        }),
    )
    .await?;
    Ok(format!("{} follows upstream again", t.label()))
}

/// Lift a hold: the release may be smoke-tested and adopted again.
pub async fn allow(ops: &dyn Ops, tool: &str, version: &str, by: &str) -> Result<String, String> {
    let t = tool_of(tool)?;
    let s = ops.load().await?;
    if !s.tool(t).held.contains_key(version) {
        return Err(format!("{} {version:.40} is not held", t.label()));
    }
    let now = ops.now();
    let (by, v) = (by.to_string(), version.to_string());
    engine::write(
        ops,
        Box::new(move |s: &mut State| {
            s.tool_mut(t).held.remove(&v);
            s.alerted.remove(&format!("held:{}:{v}", t.slug()));
            s.event(now, format!("{by} allowed {} {v} again", t.label()));
        }),
    )
    .await?;
    Ok(format!(
        "{} {version} may be adopted again: it is smoke-tested within a minute or two",
        t.label()
    ))
}

/// Adopt the newest release seen now, without its soak.
pub async fn update_now(ops: &dyn Ops, tool: &str, by: &str) -> Result<String, String> {
    let t = tool_of(tool)?;
    let s = ops.load().await?;
    let cur = t.of(&s.running()).to_string();
    let ts = s.tool(t);
    let newest = ts
        .seen
        .iter()
        .filter(|(v, seen)| {
            semver::newer(v, &cur) && !ts.held.contains_key(*v) && seen.deprecated.is_none()
        })
        .map(|(v, _)| v.clone())
        .max_by(|a, b| semver::cmp(a, b))
        .ok_or_else(|| {
            format!(
                "no {} release newer than {cur} has been seen: Check now first",
                t.label()
            )
        })?;
    let now = ops.now();
    let (by, v) = (by.to_string(), newest.clone());
    engine::write(
        ops,
        Box::new(move |s: &mut State| {
            s.tool_mut(t).forced = Some(v.clone());
            s.retry_at = None;
            s.event(
                now,
                format!("{by} asked for {} {v} now, without its soak", t.label()),
            );
        }),
    )
    .await?;
    Ok(format!(
        "{} {newest} is smoke-tested and adopted within a minute or two",
        t.label()
    ))
}

/// Set an unreadable state row aside (kept, under a new key) and start a
/// fresh one. Jobs run the bootstrap until the channel verifies again.
pub async fn reset(pg: &PgPool, by: &str) -> Result<String, String> {
    let key = talaria_workbench_harnesses::CHANNEL_STATE_KEY;
    let archive = format!("{key}:archived:{}", talaria_agent_auth::now_ms());
    let db = |e: sqlx::Error| e.to_string();
    let mut tx = pg.begin().await.map_err(db)?;
    let stored: Option<Value> =
        sqlx::query_scalar("select value from app_settings where key = $1 for update")
            .bind(key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
    if state::parse_state(stored.clone()).is_ok() {
        return Err("the state is readable: nothing to reset".into());
    }
    sqlx::query("insert into app_settings (key, value) values ($1, $2)")
        .bind(&archive)
        .bind(stored.unwrap_or(Value::Null))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let mut fresh = State::default();
    fresh.event(
        talaria_agent_auth::now_ms(),
        format!("{by} reset the unreadable state (kept as app_settings '{archive}')"),
    );
    sqlx::query("update app_settings set value = $2, updated_at = now() where key = $1")
        .bind(key)
        .bind(serde_json::to_value(&fresh).map_err(|e| e.to_string())?)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(format!(
        "the unreadable state is set aside as app_settings '{archive}'; jobs run the bootstrap until a release is verified again"
    ))
}

// ── What the panel shows ────────────────────────────────────────────────────

/// The channel as the admins' panel reads it.
pub fn status_of(s: &State, now: i64, tuning: &Tuning) -> Value {
    let running = s.running();
    let verified = s.current.is_some();
    let plan = plan::decide(s, now, tuning);
    let tools: Vec<Value> = Tool::ALL
        .iter()
        .map(|&t| {
            let ts = s.tool(t);
            let cur = t.of(&running).to_string();
            let mut held: Vec<Value> = ts
                .held
                .iter()
                .map(|(v, h)| json!({ "version": v, "reason": h.reason, "at": h.at, "department": h.department }))
                .collect();
            held.sort_by(|a, b| b["at"].as_i64().cmp(&a["at"].as_i64()));
            let mut seen: Vec<(&String, &state::Seen)> = ts.seen.iter().collect();
            seen.sort_by(|a, b| semver::cmp(b.0, a.0));
            let candidate = seen
                .iter()
                .find(|(v, x)| semver::newer(v, &cur) && !ts.held.contains_key(*v) && x.deprecated.is_none())
                .map(|(v, x)| {
                    json!({
                        "version": v,
                        "firstSeenAt": x.first_seen_at,
                        "readyAt": x.first_seen_at.saturating_add(tuning.min_age_ms),
                        "requiresBun": x.requires_bun,
                        "forced": ts.forced.as_deref() == Some(v.as_str()),
                    })
                });
            let moving = plan.mv.as_ref().filter(|m| m.tool == t).map(|m| json!({
                "version": m.version, "bun": m.to.bun, "omp": m.to.omp, "why": m.why,
            }));
            json!({
                "tool": t.slug(),
                "label": t.label(),
                "source": t.source(),
                "current": cur,
                "currentSource": if verified { "verified" } else { "bootstrap" },
                "bootstrap": t.bootstrap(),
                "previous": s.previous.as_ref().map(|p| t.of(&p.versions()).to_string()),
                "pinned": ts.pinned,
                "candidate": candidate,
                "next": moving,
                "held": held,
                "detail": plan.details.get(t.slug()).cloned().flatten().or(ts.detail.clone()),
                "lastCheck": ts.last_check.as_ref().map(|c| json!({ "at": c.at, "latest": c.latest, "error": c.error })),
                "versions": seen.iter().take(12).map(|(v, x)| json!({
                    "version": v, "requiresBun": x.requires_bun, "deprecated": x.deprecated,
                    "held": ts.held.contains_key(*v),
                })).collect::<Vec<_>>(),
                "requiresBun": if t == Tool::Omp { plan::requires(s, &cur).flatten() } else { None },
            })
        })
        .collect();
    let volumes: Vec<Value> = s
        .volumes
        .iter()
        .map(|(d, v)| json!({ "department": d, "bun": v.bun, "omp": v.omp, "at": v.at, "error": v.error }))
        .collect();
    let mut events = s.events.clone();
    events.reverse();
    json!({
        "tools": tools,
        "current": s.current.as_ref().map(|p| json!({ "bun": p.bun, "omp": p.omp, "at": p.at })),
        "previous": s.previous.as_ref().map(|p| json!({ "bun": p.bun, "omp": p.omp, "at": p.at })),
        "nextCheckAt": s.next_check_at,
        "retryAt": s.retry_at.filter(|at| *at > now),
        "minAgeHours": tuning.min_age_ms / (60 * 60 * 1000),
        "volumes": volumes,
        "events": events.into_iter().map(|e| json!({ "at": e.at, "text": e.text })).collect::<Vec<_>>(),
    })
}

/// [`status_of`] for the live instance, with each volume's agent named.
pub async fn status(pg: &PgPool) -> Result<Value, String> {
    let s = load(pg).await?;
    let mut v = status_of(&s, talaria_agent_auth::now_ms(), &tuning());
    let names: Vec<(String, String)> = sqlx::query_as(
        "select department, min(display_name) from agent_defs where developer and managed group by department",
    )
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    if let Some(vols) = v.get_mut("volumes").and_then(Value::as_array_mut) {
        for vol in vols {
            let d = vol["department"].as_str().unwrap_or("").to_string();
            if let Some((_, n)) = names.iter().find(|(x, _)| *x == d) {
                vol["agent"] = json!(n);
            }
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_names_current_candidate_and_held_per_harness() {
        let mut s = State::default();
        s.saw(
            Tool::Omp,
            &upstream::Release {
                version: "18.17.0".into(),
                requires_bun: Some(">=1.4.4".into()),
                deprecated: None,
            },
            1_000,
        );
        s.tool_mut(Tool::Omp).held.insert(
            "18.16.5".into(),
            state::Held {
                reason: "does not start".into(),
                at: 5,
                department: Some("eng".into()),
            },
        );
        let v = status_of(&s, 2_000, &Tuning { min_age_ms: 10_000 });
        let omp = &v["tools"][0];
        assert_eq!(omp["tool"], json!("omp"));
        assert_eq!(
            omp["current"],
            json!(talaria_workbench_harnesses::OMP_VERSION)
        );
        assert_eq!(omp["currentSource"], json!("bootstrap"));
        assert_eq!(omp["candidate"]["version"], json!("18.17.0"));
        assert_eq!(omp["candidate"]["readyAt"], json!(11_000));
        assert_eq!(omp["held"][0]["version"], json!("18.16.5"));
        assert!(omp["detail"].as_str().unwrap().contains("soaking"));
        assert_eq!(v["tools"][1]["tool"], json!("bun"));
    }
}
