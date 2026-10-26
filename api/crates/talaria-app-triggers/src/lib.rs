// App triggers — an app's code run with no request behind it: on a schedule
// (`cron`) or when a platform event happens (`event`), as `talaria.json`
// declares them (talaria-app-manifest `Trigger`).
//
// THE SPLIT, the one every app path keeps: app code is TypeScript and the app
// host runs it (docs/RUST-MIGRATION.md rule 10); this crate decides WHEN and
// keeps the RECORD. A run is a row in `app_trigger_runs` first, then a call
// to the host's `POST /api/app-host/<slug>/trigger/<id>` under the
// root-derived service credential, which runs the handler the app's
// `defineAppServer({ triggers })` names for that id — as the app, with no
// user — and answers how it went. The row is the record; the call is the
// delivery.
//
// WHEN:
//   cron    the one scheduler's job (`app-triggers`, every `TICK_MS`, leased
//           so one instance ticks per interval) finds each schedule's latest
//           due minute and claims it by moving `app_trigger_state.last_due_at`
//           forward — an instance fires a minute only if it moved it, so two
//           api processes never fire one minute twice. A trigger seen for the
//           first time is ARMED now (installing a nightly job at noon does not
//           fire the 03:00 it "missed"); minutes missed while the api was down
//           fire ONCE, for the latest. Read in UTC.
//   event   an engine emits a platform event (talaria-platform-events) where
//           it already announces the change; the sink installed at boot
//           queues a run for every trigger that names it and delivers it at
//           once. The tick re-delivers a run left queued (the instance died
//           between the row and the call).
//
// WHICH APPS: enabled, and their build ready. A disabled app or a failed
// build fires nothing, and its schedules are disarmed — turning it back on
// arms them afresh rather than catching up. A queued run whose app was turned
// off before it started is recorded `skipped`. A FAILING TRIGGER NEVER
// DISABLES THE APP: its run is recorded `failed` with the sentence, shown on
// the app's row in Manage → Apps (`trigger_report`), and the next one runs.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::Semaphore;

use talaria_agent_auth::{epoch_ms_to_iso, now_ms};
use talaria_app_manifest::{Trigger, TriggerOn, parse_manifest};
use talaria_platform_events::PlatformEvent;

/// How often the scheduler ticks. A schedule fires within this of its minute.
pub const TICK_MS: u64 = 30_000;
/// The longest one run may take. The host aborts the handler's `ctx.signal`
/// at it; a run that never reports back is recorded failed after it.
pub const RUN_DEADLINE: Duration = Duration::from_secs(10 * 60);
/// How long a run row is kept.
pub const KEEP_DAYS: i64 = 14;
/// Runs delivered at once by this process. A burst of events queues rows;
/// they go out as permits free up, and the tick picks up anything left.
const CONCURRENCY: usize = 4;
/// The newest runs per trigger the Apps row shows.
const REPORT_RUNS: usize = 5;
/// The app's own result line and an error sentence, as stored.
const DETAIL_MAX: usize = 500;
const ERROR_MAX: usize = 1_000;
/// How long a scan of the enabled apps' manifests is reused by the event
/// path: a burst of ticket writes reads the disk once.
const SCAN_TTL: Duration = Duration::from_secs(5);

static PERMITS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(CONCURRENCY));

/// What a delivery needs: the database, the service credential the host's
/// trigger route takes, and where the host listens.
#[derive(Clone)]
pub struct TriggerDeps {
    pub pg: PgPool,
    pub token: String,
    pub host: String,
}

impl TriggerDeps {
    /// `token` is `state.cfg.secret_root.internal_token()`; the host is
    /// talaria-apps' `app_host_url`.
    pub fn new(pg: PgPool, token: String) -> Self {
        Self {
            pg,
            token,
            host: talaria_apps::host::app_host_url(),
        }
    }
}

fn clamp(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ── Which apps' triggers are live ────────────────────────────────────────────

/// One enabled app's triggers.
#[derive(Debug, Clone)]
pub struct AppTriggers {
    pub slug: String,
    /// Its build is ready, so its triggers fire now. False while it builds:
    /// its schedules stay armed and fire on the first tick after.
    pub fires: bool,
    pub triggers: Vec<Trigger>,
}

/// The installed apps, and the enabled ones whose triggers are armed.
#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub installed: HashSet<String>,
    pub apps: Vec<AppTriggers>,
}

/// An installed app's triggers, from its `talaria.json` — None when the
/// manifest fails the rules (the app is listed as broken, and runs nothing).
pub fn manifest_triggers(slug: &str) -> Option<Vec<Trigger>> {
    let bytes = std::fs::read(talaria_users::apps_dir().join(slug).join("talaria.json")).ok()?;
    let m = parse_manifest(&bytes, talaria_app_manifest::SDK_VERSION).ok()?;
    if m.slug.as_deref().is_some_and(|declared| declared != slug) {
        return None;
    }
    Some(m.triggers)
}

/// Read the installed apps from disk and the enabled set from the database.
pub async fn scan(pg: &PgPool) -> Scan {
    let installed: HashSet<String> = talaria_users::discovered_apps()
        .into_iter()
        .map(|a| a.slug)
        .collect();
    let mut apps = Vec::new();
    for slug in talaria_apps::enabled_app_slugs(pg).await {
        if !installed.contains(&slug) {
            continue;
        }
        let status = talaria_users::app_build_status(&slug).status;
        if status == "failed" {
            continue;
        }
        let Some(triggers) = manifest_triggers(&slug) else {
            continue;
        };
        if triggers.is_empty() {
            continue;
        }
        apps.push(AppTriggers {
            slug,
            fires: status == "ready",
            triggers,
        });
    }
    let scan = Scan { installed, apps };
    *SCAN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((Instant::now(), scan.clone()));
    scan
}

static SCAN: LazyLock<Mutex<Option<(Instant, Scan)>>> = LazyLock::new(|| Mutex::new(None));

async fn recent_scan(pg: &PgPool) -> Scan {
    {
        let held = SCAN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, scan)) = held.as_ref()
            && at.elapsed() < SCAN_TTL
        {
            return scan.clone();
        }
    }
    scan(pg).await
}

/// The triggers that name `event`, in apps that fire now: (slug, trigger id).
pub fn subscribers(scan: &Scan, event: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for app in scan.apps.iter().filter(|a| a.fires) {
        for t in &app.triggers {
            if matches!(&t.on, TriggerOn::Event(name) if name == event) {
                out.push((app.slug.clone(), t.id.clone()));
            }
        }
    }
    out
}

/// Why a run of this trigger may not start now, or None.
async fn not_firing(pg: &PgPool, slug: &str, trigger: &str) -> Option<String> {
    if !talaria_apps::enabled_app_slugs(pg)
        .await
        .iter()
        .any(|s| s == slug)
    {
        return Some(format!(
            "app \"{slug}\" was turned off before this run started"
        ));
    }
    let status = talaria_users::app_build_status(slug).status;
    if status != "ready" {
        return Some(format!(
            "app \"{slug}\" has no ready build (its build is {status})"
        ));
    }
    match manifest_triggers(slug) {
        Some(triggers) if triggers.iter().any(|t| t.id == trigger) => None,
        _ => Some(format!(
            "app \"{slug}\" no longer declares the trigger \"{trigger}\""
        )),
    }
}

// ── Runs ─────────────────────────────────────────────────────────────────────

/// Queue one run. None when the row was not written — a scheduled minute
/// already queued (the unique index), or the database refused (logged).
async fn queue_run(
    pg: &PgPool,
    slug: &str,
    trigger: &str,
    kind: &str,
    event: Option<&str>,
    payload: &Value,
    due_ms: Option<i64>,
) -> Option<String> {
    let res: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
        "insert into app_trigger_runs (app_slug, trigger_id, kind, event, payload, due_at) \
         values ($1, $2, $3, $4, $5, $6::timestamptz) \
         on conflict (app_slug, trigger_id, due_at) where due_at is not null do nothing \
         returning id::text",
    )
    .bind(slug)
    .bind(trigger)
    .bind(kind)
    .bind(event)
    .bind(payload)
    .bind(due_ms.map(epoch_ms_to_iso))
    .fetch_optional(pg)
    .await;
    match res {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("[app-triggers] {slug}/{trigger}: the run was not queued: {e}");
            None
        }
    }
}

/// A run, as the delivery claims it.
struct Claimed {
    slug: String,
    trigger: String,
    kind: String,
    event: Option<String>,
    payload: Value,
    due_ms: Option<i64>,
}

async fn claim_run(pg: &PgPool, id: &str) -> Result<Option<Claimed>, sqlx::Error> {
    let row: Option<(String, String, String, Option<String>, Value, Option<i64>)> = sqlx::query_as(
        "update app_trigger_runs \
               set state = 'running', started_at = now(), attempts = attempts + 1 \
             where id = $1::uuid and state = 'queued' \
             returning app_slug, trigger_id, kind, event, payload, \
               (extract(epoch from due_at) * 1000)::bigint",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(
        row.map(|(slug, trigger, kind, event, payload, due_ms)| Claimed {
            slug,
            trigger,
            kind,
            event,
            payload,
            due_ms,
        }),
    )
}

async fn finish_run(pg: &PgPool, id: &str, state: &str, detail: Option<&str>, error: Option<&str>) {
    let res = sqlx::query(
        "update app_trigger_runs set state = $2, finished_at = now(), detail = $3, error = $4 \
         where id = $1::uuid",
    )
    .bind(id)
    .bind(state)
    .bind(detail.map(|d| clamp(d, DETAIL_MAX)))
    .bind(error.map(|e| clamp(e, ERROR_MAX)))
    .execute(pg)
    .await;
    if let Err(e) = res {
        tracing::error!("[app-triggers] run {id}: its outcome ({state}) was not recorded: {e}");
    }
}

/// What the host's trigger route answered: the app's own result line, or why
/// the run failed.
pub fn outcome_of(status: u16, body: &Value) -> Result<Option<String>, String> {
    let said = body.get("error").and_then(Value::as_str);
    if !(200..300).contains(&status) {
        return Err(match said {
            Some(e) => format!("the app host answered {status}: {e}"),
            None => format!("the app host answered {status}"),
        });
    }
    if body.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(body
            .get("detail")
            .and_then(Value::as_str)
            .filter(|d| !d.trim().is_empty())
            .map(str::to_string));
    }
    Err(said
        .unwrap_or("the app host answered without saying how the run went")
        .to_string())
}

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        // Loopback, always: a proxy from the environment must not carry the
        // internal credential anywhere.
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
});

async fn call_host(deps: &TriggerDeps, id: &str, run: &Claimed) -> Result<Option<String>, String> {
    let body = json!({
        "runId": id,
        "kind": run.kind,
        "dueAt": run.due_ms.map(epoch_ms_to_iso),
        "event": run.event,
        "payload": run.payload,
        "deadlineMs": RUN_DEADLINE.as_millis() as u64,
    });
    let url = format!(
        "{}/api/app-host/{}/trigger/{}",
        deps.host, run.slug, run.trigger
    );
    let res = CLIENT
        .post(url)
        .header(talaria_config::INTERNAL_HEADER, &deps.token)
        .json(&body)
        .timeout(RUN_DEADLINE + Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("the app host did not answer: {e}"))?;
    let status = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    outcome_of(status, &parsed)
}

/// Deliver one queued run: claim it, check the app still fires, call the
/// host, record the outcome. A run another instance already claimed is left
/// to it.
pub async fn deliver(deps: &TriggerDeps, id: &str) {
    let Ok(_permit) = PERMITS.acquire().await else {
        return;
    };
    let run = match claim_run(&deps.pg, id).await {
        Ok(Some(run)) => run,
        Ok(None) => return,
        Err(e) => {
            tracing::error!("[app-triggers] run {id} could not be claimed: {e}");
            return;
        }
    };
    if let Some(why) = not_firing(&deps.pg, &run.slug, &run.trigger).await {
        finish_run(&deps.pg, id, "skipped", None, Some(&why)).await;
        return;
    }
    let started = Instant::now();
    match call_host(deps, id, &run).await {
        Ok(detail) => {
            tracing::info!(
                "[app-triggers] {}/{} ran in {}ms",
                run.slug,
                run.trigger,
                started.elapsed().as_millis()
            );
            finish_run(&deps.pg, id, "ok", detail.as_deref(), None).await;
        }
        Err(e) => {
            tracing::warn!("[app-triggers] {}/{} failed: {e}", run.slug, run.trigger);
            finish_run(&deps.pg, id, "failed", None, Some(&e)).await;
        }
    }
}

fn spawn_deliver(deps: &TriggerDeps, id: String) {
    let deps = deps.clone();
    tokio::spawn(async move { deliver(&deps, &id).await });
}

// ── Events ───────────────────────────────────────────────────────────────────

/// Queue and deliver a run for every trigger that names `event`.
pub async fn on_event(deps: &TriggerDeps, event: PlatformEvent) {
    let scan = recent_scan(&deps.pg).await;
    let payload = event.payload();
    for (slug, trigger) in subscribers(&scan, event.name()) {
        if let Some(id) = queue_run(
            &deps.pg,
            &slug,
            &trigger,
            "event",
            Some(event.name()),
            &payload,
            None,
        )
        .await
        {
            spawn_deliver(deps, id);
        }
    }
}

/// The sink talaria-jobs installs (`talaria_platform_events::set_sink`): each
/// event is handled detached, so the write that emitted it never waits.
pub fn event_sink(deps: TriggerDeps) -> talaria_platform_events::Sink {
    Arc::new(move |event| {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let deps = deps.clone();
        handle.spawn(async move { on_event(&deps, event).await });
    })
}

// ── The tick ─────────────────────────────────────────────────────────────────

/// One schedule as the tick reads it.
struct Armed {
    slug: String,
    trigger: String,
    text: String,
    schedule: talaria_app_manifest::Cron,
    fires: bool,
}

fn armed_schedules(scan: &Scan) -> Vec<Armed> {
    let mut out = Vec::new();
    for app in &scan.apps {
        for t in &app.triggers {
            if let TriggerOn::Cron { text, schedule } = &t.on {
                out.push(Armed {
                    slug: app.slug.clone(),
                    trigger: t.id.clone(),
                    text: text.clone(),
                    schedule: schedule.clone(),
                    fires: app.fires,
                });
            }
        }
    }
    out
}

/// One tick: arm and disarm schedules, fire the due ones, re-deliver what was
/// left queued, close what never reported back, prune. The line is the
/// scheduler's log sentence.
pub async fn tick(deps: &TriggerDeps) -> Result<Option<String>, String> {
    let pg = &deps.pg;
    let now = now_ms();
    let scan = scan(pg).await;
    let armed = armed_schedules(&scan);

    // Arm and disarm. A row whose trigger is gone, or whose schedule changed,
    // is dropped; the ones that remain keep their arming.
    let rows: Vec<(String, String, String, i64, Option<i64>)> = sqlx::query_as(
        "select app_slug, trigger_id, cron, \
           (extract(epoch from armed_at) * 1000)::bigint, \
           (extract(epoch from last_due_at) * 1000)::bigint \
         from app_trigger_state",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("reading the armed schedules: {e}"))?;
    let mut state: HashMap<(String, String), (i64, Option<i64>)> = HashMap::new();
    for (slug, trigger, text, armed_ms, last_ms) in rows {
        let live = armed
            .iter()
            .any(|a| a.slug == slug && a.trigger == trigger && a.text == text);
        if live {
            state.insert((slug, trigger), (armed_ms, last_ms));
        } else {
            sqlx::query("delete from app_trigger_state where app_slug = $1 and trigger_id = $2")
                .bind(&slug)
                .bind(&trigger)
                .execute(pg)
                .await
                .map_err(|e| format!("disarming {slug}/{trigger}: {e}"))?;
        }
    }

    let mut fired = Vec::new();
    for a in &armed {
        let key = (a.slug.clone(), a.trigger.clone());
        let Some(&(armed_ms, last_ms)) = state.get(&key) else {
            sqlx::query(
                "insert into app_trigger_state (app_slug, trigger_id, cron, armed_at) \
                 values ($1, $2, $3, $4::timestamptz) \
                 on conflict (app_slug, trigger_id) do nothing",
            )
            .bind(&a.slug)
            .bind(&a.trigger)
            .bind(&a.text)
            .bind(epoch_ms_to_iso(now))
            .execute(pg)
            .await
            .map_err(|e| format!("arming {}/{}: {e}", a.slug, a.trigger))?;
            continue;
        };
        if !a.fires {
            continue;
        }
        let after = last_ms.map_or(armed_ms, |l| l.max(armed_ms));
        let Some(due) = a.schedule.latest_due(after, now) else {
            continue;
        };
        // THE CLAIM: only the instance that moves last_due_at forward fires.
        let claimed = sqlx::query(
            "update app_trigger_state set last_due_at = $3::timestamptz \
             where app_slug = $1 and trigger_id = $2 \
               and (last_due_at is null or last_due_at < $3::timestamptz)",
        )
        .bind(&a.slug)
        .bind(&a.trigger)
        .bind(epoch_ms_to_iso(due))
        .execute(pg)
        .await
        .map_err(|e| format!("claiming {}/{}: {e}", a.slug, a.trigger))?
        .rows_affected();
        if claimed == 0 {
            continue;
        }
        if let Some(id) =
            queue_run(pg, &a.slug, &a.trigger, "cron", None, &json!({}), Some(due)).await
        {
            spawn_deliver(deps, id);
            fired.push(format!("{}/{}", a.slug, a.trigger));
        }
    }

    // A run left queued past a minute: the instance that wrote it died before
    // delivering. The claim keeps a second delivery from running it twice.
    let stale: Vec<(String,)> = sqlx::query_as(
        "select id::text from app_trigger_runs \
         where state = 'queued' and queued_at < now() - interval '1 minute' \
         order by queued_at limit 100",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("reading queued runs: {e}"))?;
    for (id,) in &stale {
        spawn_deliver(deps, id.clone());
    }

    // A run still `running` well past the deadline never reported back: the
    // host restarted under it, or the api did mid-call.
    let lost = sqlx::query(
        "update app_trigger_runs set state = 'failed', finished_at = now(), \
           error = 'the run never reported back (the app host or the api restarted while it ran)' \
         where state = 'running' and started_at < now() - make_interval(secs => $1)",
    )
    .bind((RUN_DEADLINE.as_secs() + 120) as f64)
    .execute(pg)
    .await
    .map_err(|e| format!("closing lost runs: {e}"))?
    .rows_affected();

    let installed: Vec<String> = scan.installed.iter().cloned().collect();
    sqlx::query(
        "delete from app_trigger_runs \
         where queued_at < now() - make_interval(days => $1) or not (app_slug = any($2::text[]))",
    )
    .bind(KEEP_DAYS as i32)
    .bind(&installed)
    .execute(pg)
    .await
    .map_err(|e| format!("pruning runs: {e}"))?;

    let mut said = Vec::new();
    if !fired.is_empty() {
        said.push(format!("fired {}", fired.join(", ")));
    }
    if !stale.is_empty() {
        said.push(format!("re-delivered {} queued", stale.len()));
    }
    if lost > 0 {
        said.push(format!("closed {lost} lost"));
    }
    Ok((!said.is_empty()).then(|| said.join("; ")))
}

/// The scheduler's job: `tick` every `TICK_MS`, leased.
pub fn job_spec(deps: TriggerDeps) -> talaria_scheduler::JobSpec {
    talaria_scheduler::JobSpec {
        name: talaria_scheduler::JobName::AppTriggers,
        every_ms: TICK_MS,
        first_run_delay_ms: Some(20_000),
        max_run_ms: Some(60_000),
        // Leased: the input is the database every instance shares.
        per_instance: false,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move { tick(&deps).await })
        }),
    }
}

// ── What Manage → Apps shows ─────────────────────────────────────────────────

/// Each declared trigger of `slug`, with when a schedule next fires and its
/// newest runs: `[{ id, cron | event, nextAt?, runs: [{ state, dueAt?,
/// event?, queuedAt, startedAt?, finishedAt?, detail?, error? }] }]`. Empty
/// when the app declares none (or its manifest is refused).
pub async fn trigger_report(pg: &PgPool, slug: &str) -> Value {
    let Some(triggers) = manifest_triggers(slug) else {
        return json!([]);
    };
    if triggers.is_empty() {
        return json!([]);
    }
    type RunRow = (
        String,
        String,
        Option<String>,
        Option<i64>,
        i64,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<RunRow> = sqlx::query_as(
        "select trigger_id, state, event, \
           (extract(epoch from due_at) * 1000)::bigint, \
           (extract(epoch from queued_at) * 1000)::bigint, \
           (extract(epoch from started_at) * 1000)::bigint, \
           (extract(epoch from finished_at) * 1000)::bigint, \
           detail, error \
         from app_trigger_runs where app_slug = $1 \
         order by queued_at desc limit 200",
    )
    .bind(slug)
    .fetch_all(pg)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("[app-triggers] {slug}: the runs could not be read: {e}");
        Vec::new()
    });
    let now = now_ms();
    triggers
        .iter()
        .map(|t| {
            let runs: Vec<Value> = rows
                .iter()
                .filter(|r| r.0 == t.id)
                .take(REPORT_RUNS)
                .map(
                    |(_, state, event, due, queued, started, finished, detail, error)| {
                        json!({
                            "state": state,
                            "event": event,
                            "dueAt": due.map(epoch_ms_to_iso),
                            "queuedAt": epoch_ms_to_iso(*queued),
                            "startedAt": started.map(epoch_ms_to_iso),
                            "finishedAt": finished.map(epoch_ms_to_iso),
                            "detail": detail,
                            "error": error,
                        })
                    },
                )
                .collect();
            match &t.on {
                TriggerOn::Cron { text, schedule } => json!({
                    "id": t.id,
                    "cron": text,
                    "nextAt": schedule.next_after(now, 400).map(epoch_ms_to_iso),
                    "runs": runs,
                }),
                TriggerOn::Event(name) => json!({ "id": t.id, "event": name, "runs": runs }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_app_manifest::parse_cron;

    fn app(slug: &str, fires: bool, triggers: Vec<Trigger>) -> AppTriggers {
        AppTriggers {
            slug: slug.into(),
            fires,
            triggers,
        }
    }

    fn event(id: &str, name: &str) -> Trigger {
        Trigger {
            id: id.into(),
            on: TriggerOn::Event(name.into()),
        }
    }

    fn cron(id: &str, text: &str) -> Trigger {
        Trigger {
            id: id.into(),
            on: TriggerOn::Cron {
                text: text.into(),
                schedule: parse_cron(text).unwrap(),
            },
        }
    }

    #[test]
    fn an_event_reaches_the_triggers_that_name_it_in_apps_that_fire() {
        let scan = Scan {
            installed: HashSet::new(),
            apps: vec![
                app(
                    "crm",
                    true,
                    vec![
                        event("on-new", "ticket.created"),
                        event("on-move", "ticket.moved"),
                        cron("nightly", "0 3 * * *"),
                    ],
                ),
                app("notes", true, vec![event("also-new", "ticket.created")]),
                // Building: armed, but nothing fires until it is ready.
                app("draft", false, vec![event("new", "ticket.created")]),
            ],
        };
        assert_eq!(
            subscribers(&scan, "ticket.created"),
            vec![
                ("crm".to_string(), "on-new".to_string()),
                ("notes".to_string(), "also-new".to_string())
            ]
        );
        assert_eq!(
            subscribers(&scan, "ticket.moved"),
            vec![("crm".to_string(), "on-move".to_string())]
        );
        let schedules = armed_schedules(&scan);
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].text, "0 3 * * *");
    }

    #[test]
    fn the_hosts_answer_is_read_into_an_outcome() {
        assert_eq!(outcome_of(200, &json!({"ok": true})), Ok(None));
        assert_eq!(
            outcome_of(200, &json!({"ok": true, "detail": "3 leads scored"})),
            Ok(Some("3 leads scored".into()))
        );
        assert_eq!(
            outcome_of(200, &json!({"ok": false, "error": "boom"})),
            Err("boom".into())
        );
        assert_eq!(
            outcome_of(
                404,
                &json!({"error": "this app has no handler for the trigger \"x\""})
            ),
            Err("the app host answered 404: this app has no handler for the trigger \"x\"".into())
        );
        assert_eq!(
            outcome_of(502, &Value::Null),
            Err("the app host answered 502".into())
        );
        assert!(
            outcome_of(200, &json!({}))
                .unwrap_err()
                .contains("without saying")
        );
    }

    #[test]
    fn stored_text_is_clamped_on_a_char_boundary() {
        assert_eq!(clamp("short", 10), "short");
        let long = "é".repeat(20);
        let c = clamp(&long, 10);
        assert_eq!(c.chars().count(), 10);
        assert!(c.ends_with('…'));
    }
}
