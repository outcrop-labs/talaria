// THE APP LAYER, AS THE API SEES IT: the activity harnesses enabled apps ship
// in `apps/<slug>/harnesses/*.ts`, registered beside the builtins so the
// model-fitness matrix gets a column for each and the eval sweep scores their
// fixtures.
//
// THE API NEVER LOADS ONE. An app harness is customer code — its `render`
// builds the messages, its fixtures' `check` grades the reply — and app
// modules are not port surface (docs/RUST-MIGRATION.md rule 10). The app host
// loads and checks them (ui/src/server/app-harnesses.ts) and answers two
// loopback-only routes this crate calls with the internal credential:
//
//   GET  /api/app-host/harnesses                          the harnesses, as data
//   POST /api/app-host/<slug>/harnesses/<id>/cases/<name>  replay one fixture
//
// So an app harness here is a `HarnessDefinition` that carries the METADATA
// the registry's readers use (id, label, job, requires, floor, guard, output
// kind, fixture names and bands) and stubs where the code would be: nothing in
// the api renders one or runs its checks, and a stub that is reached says so.
// The defs are interned — one leak per distinct definition, the same
// `&'static` lifetime every builtin has — so a fitness page polling every few
// seconds does not leak a copy per poll.
//
// THE APP HOST BEING DOWN IS NOT AN OUTAGE. The fitness plane keeps its
// builtins and logs why the app layer is missing; the matrix is a working
// product with forty columns and an honest warning, never an error page.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use talaria_harness::define::{
    CheckResult, EvalBand, EvalCase, GuardDecl, HarnessDefinition, OnFailure, Output, RoleFloor,
    Widen,
};
use talaria_harness::transport::ToolPolicy;
use talaria_harness_defs::registry::{
    HarnessSource, RegisteredHarness, builtin_activity_harnesses, merge_app_layer,
};
use talaria_harness_model::ModelSpec;
use talaria_harness_schema::Schema;

// ── The wire ─────────────────────────────────────────────────────────────────

/// One app harness as the app host lists it (`AppHarnessWire` in
/// ui/src/server/app-harnesses.ts). Data only; fields the api does not read
/// (the model spec, which binds nothing here) are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppHarnessWire {
    pub id: String,
    pub label: String,
    pub job: String,
    /// `app:<slug>` — must agree with `app`.
    pub source: String,
    pub app: String,
    #[serde(default)]
    pub app_name: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub requires: Vec<String>,
    pub floor: WireFloor,
    pub output_kind: String,
    #[serde(default)]
    pub repair: u32,
    #[serde(default)]
    pub verifies: bool,
    #[serde(default = "tools_none")]
    pub tools: String,
    #[serde(default)]
    pub guard: Option<WireGuard>,
    #[serde(default)]
    pub widen: Option<WireWiden>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub evals: Vec<WireEval>,
}

fn tools_none() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireFloor {
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub refuse_below: bool,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireGuard {
    #[serde(default)]
    pub rules: Option<Vec<String>>,
    #[serde(default)]
    pub redact: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireWiden {
    pub requires: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireEval {
    pub name: String,
    pub band: EvalBand,
}

/// A harness file the app host skipped, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireProblem {
    pub app: String,
    pub file: String,
    #[serde(default)]
    pub id: Option<String>,
    pub reason: String,
}

/// `GET /api/app-host/harnesses`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppHarnessListing {
    #[serde(default)]
    pub harnesses: Vec<AppHarnessWire>,
    #[serde(default)]
    pub problems: Vec<WireProblem>,
}

/// One fixture, replayed by the app host against a pinned model (`CaseReplay`
/// in ui/src/server/app-harnesses.ts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCaseReply {
    pub harness: String,
    pub case: String,
    pub band: EvalBand,
    pub model: String,
    #[serde(default)]
    pub value: Value,
    /// `pass`, `fail`, `gap`, or None when there was no value to grade.
    #[serde(default)]
    pub verdict: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    pub schema_valid: bool,
    #[serde(default)]
    pub repairs: i64,
    #[serde(default)]
    pub answered: bool,
    #[serde(default)]
    pub refused: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub latency_ms: i64,
    #[serde(default)]
    pub findings: i64,
    #[serde(default)]
    pub raw: Option<String>,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub prompt_tokens: i64,
    #[serde(default)]
    pub completion_tokens: i64,
    #[serde(default)]
    pub estimated: bool,
}

/// Which fixture to replay, against which candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCaseRequest {
    pub app: String,
    pub harness: String,
    pub case: String,
    pub model: String,
}

// ── Registration ─────────────────────────────────────────────────────────────

/// What the stubs say if anything in the api ever reaches them. Nothing should:
/// the sweep routes an app harness's fixtures to the app host.
const HOST_ONLY: &str = "an app harness runs in the app host, not the api";

static STRS: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static DEFS: LazyLock<Mutex<HashMap<String, &'static HarnessDefinition>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// One leak per distinct string, for the `&'static str` every registry field
/// is.
fn intern(s: &str) -> &'static str {
    let mut set = STRS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(hit) = set.get(s) {
        return hit;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    set.insert(leaked);
    leaked
}

fn slug_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn id_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 120
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b':' | b'.' | b'_' | b'-'))
}

/// The api's own acceptance check, under the app host's: the few facts the
/// registry's readers would misread if they were wrong. The host has already
/// applied the platform's rules; this refuses only what the api cannot
/// represent.
fn wire_problem(w: &AppHarnessWire) -> Option<String> {
    if !slug_ok(&w.app) {
        return Some(format!("\"{}\" is not an app slug", w.app));
    }
    if w.source != format!("app:{}", w.app) {
        return Some(format!(
            "source \"{}\" does not name app \"{}\"",
            w.source, w.app
        ));
    }
    if !id_ok(&w.id) {
        return Some(format!("\"{}\" is not a harness id", w.id));
    }
    if w.label.trim().is_empty() || w.job.trim().is_empty() {
        return Some(format!("\"{}\" has no label or no job line", w.id));
    }
    if w.output_kind != "text" && w.output_kind != "json" {
        return Some(format!(
            "\"{}\" has output kind \"{}\"",
            w.id, w.output_kind
        ));
    }
    if w.tools != "none" && w.tools != "own" {
        return Some(format!("\"{}\" has tool policy \"{}\"", w.id, w.tools));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for e in &w.evals {
        if e.name.trim().is_empty() || !seen.insert(e.name.as_str()) {
            return Some(format!(
                "\"{}\" has an unnamed or repeated fixture name",
                w.id
            ));
        }
    }
    None
}

fn build_def(w: &AppHarnessWire) -> HarnessDefinition {
    let interned = |v: &[String]| v.iter().map(|s| intern(s)).collect::<Vec<&'static str>>();
    // Present only so `verifies` reads true for a harness whose contract has
    // the input-relational half; it never runs here.
    let verify = w.verifies.then(|| {
        Arc::new(
            |_: &Value,
             _: &Value,
             _: &talaria_harness::define::RenderContext|
             -> Result<Option<String>, String> { Err(HOST_ONLY.to_string()) },
        ) as talaria_harness::define::VerifyFn
    });
    let output = if w.output_kind == "json" {
        Output::Json {
            schema: Schema::Unknown,
            preprocess: None,
            repair: Some(w.repair),
            verify,
        }
    } else {
        Output::Text {
            clean: None,
            verify,
        }
    };
    let mut def = HarnessDefinition::new(
        intern(&w.id),
        intern(&w.label),
        intern(&w.job),
        // NO CHAIN, NO PIN, NO ROLE: the binding pass derives role and agent
        // columns from the spec, and an app harness belongs in its own column
        // only — a third-party fixture must not move a platform slot. ITS OWN
        // SLOT, though: `app:<slug>/<id>`, the model setting an admin assigns
        // on that column (`app_harness_models`), derived here from the slug
        // and the id exactly as the app host derives it.
        ModelSpec {
            pin: None,
            slot: Some(intern(&app_harness_slot(&w.app, &w.id))),
            role: None,
            chain: Some(&[]),
            user_id: None,
        },
        Arc::new(|_, _| Err(HOST_ONLY.to_string())),
        output,
        OnFailure::Null,
    );
    def.requires = interned(&w.requires);
    def.floor = RoleFloor {
        capabilities: interned(&w.floor.capabilities),
        refuse_below: w.floor.refuse_below,
        suppliable: Vec::new(),
        note: intern(&w.floor.note),
    };
    def.guard = w.guard.as_ref().map(|g| GuardDecl {
        rules: g.rules.as_ref().map(|r| interned(r)),
        redact: g.redact,
    });
    def.widen = w.widen.as_ref().map(|wd| Widen {
        requires: interned(&wd.requires),
        note: intern(&wd.note),
    });
    def.temperature = w.temperature;
    def.tools = Some(if w.tools == "own" {
        ToolPolicy::Own
    } else {
        ToolPolicy::None
    });
    def.evals = w
        .evals
        .iter()
        .map(|e| {
            EvalCase::new(
                intern(&e.name),
                Value::Null,
                Arc::new(|_, _| CheckResult::Gap(HOST_ONLY.to_string())),
            )
            .band(e.band)
        })
        .collect();
    def
}

/// The model-setting key of one app harness — `app:<slug>/<harnessId>`, the
/// slot an admin assigns a model to (`ModelSpec.slot`). The app host spells it
/// `appHarnessSlot` (ui/src/server/app-harness-models.ts); both build it from
/// the slug and the id, never from the app's own spec.
pub fn app_harness_slot(app: &str, harness: &str) -> String {
    format!("app:{app}/{harness}")
}

/// One app harness from the wire, as the registry holds it — or why not.
pub fn register(w: &AppHarnessWire) -> Result<RegisteredHarness, String> {
    if let Some(problem) = wire_problem(w) {
        return Err(format!("app \"{}\": {problem}", w.app));
    }
    let key = serde_json::to_string(w).map_err(|e| e.to_string())?;
    let def: &'static HarnessDefinition = {
        let mut defs = DEFS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match defs.get(&key) {
            Some(hit) => hit,
            None => {
                let leaked: &'static HarnessDefinition = Box::leak(Box::new(build_def(w)));
                defs.insert(key, leaked);
                leaked
            }
        }
    };
    Ok(RegisteredHarness {
        def,
        source: HarnessSource::App(intern(&w.app)),
    })
}

/// The app layer, registered: every harness the host listed that the api can
/// hold, in the host's order, and one sentence per harness skipped — by the
/// host (`problems`) or here.
#[derive(Clone, Default)]
pub struct AppLayer {
    pub harnesses: Vec<RegisteredHarness>,
    /// The app's display name per slug, for the fitness column's label.
    pub app_names: HashMap<String, String>,
    pub problems: Vec<String>,
}

pub fn app_layer_of(listing: &AppHarnessListing) -> AppLayer {
    let mut layer = AppLayer::default();
    for p in &listing.problems {
        layer.problems.push(format!(
            "app \"{}\": harnesses/{} skipped — {}",
            p.app, p.file, p.reason
        ));
    }
    for w in &listing.harnesses {
        match register(w) {
            Ok(h) => {
                if !w.app_name.trim().is_empty() {
                    layer.app_names.insert(w.app.clone(), w.app_name.clone());
                }
                layer.harnesses.push(h);
            }
            Err(e) => layer.problems.push(e),
        }
    }
    layer
}

/// The builtins, then the app layer. Refusals (an id a builtin or an earlier
/// app holds) are logged, never fatal.
pub fn merged_registry(layer: &AppLayer) -> Vec<RegisteredHarness> {
    let (merged, refused) = merge_app_layer(builtin_activity_harnesses(), &layer.harnesses);
    for r in refused {
        warn_once(r);
    }
    merged
}

// ── The app host ─────────────────────────────────────────────────────────────

/// Where this api reaches the app host: `TALARIA_APP_HOST_URL`, which the
/// production entry sets to the port it listens on, else loopback on `PORT`
/// (the dev app's port; `talaria dev` lifts it into the api's environment),
/// else the dev default.
pub fn app_host_url() -> String {
    if let Ok(url) = std::env::var("TALARIA_APP_HOST_URL") {
        let url = url.trim().trim_end_matches('/');
        if !url.is_empty() {
            return url.to_string();
        }
    }
    let port = std::env::var("PORT")
        .ok()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "5273".to_string());
    format!("http://127.0.0.1:{port}")
}

/// A path segment, percent-encoded: everything but the RFC 3986 unreserved
/// set. A harness id carries `:` and a fixture name is a sentence.
pub fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The case route for one fixture, relative to the app host.
pub fn case_path(req: &AppCaseRequest) -> String {
    format!(
        "/api/app-host/{}/harnesses/{}/cases/{}",
        encode_segment(&req.app),
        encode_segment(&req.harness),
        encode_segment(&req.case)
    )
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

/// How long the listing may take. It loads module files the host has already
/// imported, so this is generous.
const LIST_DEADLINE: Duration = Duration::from_secs(10);

/// A field's text from a listing entry that did not decode, for naming it.
fn text_of<'a>(entry: &'a Value, key: &str) -> Option<&'a str> {
    entry.get(key).and_then(Value::as_str)
}

/// THE LISTING, DECODED ONE ENTRY AT A TIME. A single entry this api cannot
/// read — a `repair` that is not a u32, a field of the wrong type — costs
/// that harness, reported as a problem on its app, and never the listing:
/// decoding the whole body as one `AppHarnessListing` failed on one bad entry
/// and left every app with no harnesses in the fitness plane. Only a body
/// that is not an object, or whose lists are not lists, is refused whole.
pub fn listing_of(body: &Value) -> Result<AppHarnessListing, String> {
    let Some(obj) = body.as_object() else {
        return Err("the app host's harness listing is not an object".into());
    };
    let list = |key: &str| -> Result<&[Value], String> {
        match obj.get(key) {
            None | Some(Value::Null) => Ok(&[]),
            Some(Value::Array(items)) => Ok(items.as_slice()),
            Some(_) => Err(format!(
                "the app host's harness listing has a \"{key}\" that is not a list"
            )),
        }
    };
    let mut listing = AppHarnessListing::default();
    for entry in list("problems")? {
        match serde_json::from_value::<WireProblem>(entry.clone()) {
            Ok(p) => listing.problems.push(p),
            Err(e) => warn_once(format!("a skipped-harness entry did not parse: {e}")),
        }
    }
    for entry in list("harnesses")? {
        match serde_json::from_value::<AppHarnessWire>(entry.clone()) {
            Ok(w) => listing.harnesses.push(w),
            Err(e) => {
                let app = text_of(entry, "app").unwrap_or("?").to_string();
                let file = text_of(entry, "file").unwrap_or("?").to_string();
                let id = text_of(entry, "id").map(str::to_string);
                let reason = format!("the api could not read its listing entry: {e}");
                warn_once(format!(
                    "app \"{app}\": harnesses/{file} skipped — {reason}"
                ));
                listing.problems.push(WireProblem {
                    app,
                    file,
                    id,
                    reason,
                });
            }
        }
    }
    Ok(listing)
}

/// Every warning this crate has logged, so one bad entry the api re-reads
/// every `LAYER_TTL` while the fitness matrix polls is logged once per
/// process, not every three seconds. Bounded: past `WARNED_CAP` distinct
/// sentences the set starts over rather than growing.
static WARNED_ONCE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
const WARNED_CAP: usize = 1000;

fn warn_once(message: String) {
    let mut seen = WARNED_ONCE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if seen.contains(&message) {
        return;
    }
    if seen.len() >= WARNED_CAP {
        seen.clear();
    }
    tracing::warn!("[app-harnesses] {message}");
    seen.insert(message);
}

/// Ask the app host what the enabled apps ship.
pub async fn fetch_listing(token: &str) -> Result<AppHarnessListing, String> {
    let res = CLIENT
        .get(format!("{}/api/app-host/harnesses", app_host_url()))
        .header(talaria_config::INTERNAL_HEADER, token)
        .timeout(LIST_DEADLINE)
        .send()
        .await
        .map_err(|e| format!("the app host did not answer ({}): {e}", app_host_url()))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(format!(
            "the app host answered {status}: {}",
            body.chars().take(300).collect::<String>()
        ));
    }
    let body = res
        .json::<Value>()
        .await
        .map_err(|e| format!("the app host's harness listing did not parse: {e}"))?;
    listing_of(&body)
}

/// THE ENABLED SET CHANGED: ask the app host to rewrite every enabled app's
/// harness report (`POST /api/app-host/harnesses`, the "N harnesses" and
/// "N skipped" lines in Manage → Apps). An app switched on that sorts first
/// takes a contested id from the app that held it, and one switched off
/// hands it back; a report written only by each app's own load probe kept
/// saying what was true then. The layer this process cached goes too, so
/// the fitness plane reads the new set on its next ask.
pub async fn refresh_app_reports(token: &str) -> Result<(), String> {
    *CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    let res = CLIENT
        .post(format!("{}/api/app-host/harnesses", app_host_url()))
        .header(talaria_config::INTERNAL_HEADER, token)
        .timeout(LIST_DEADLINE)
        .send()
        .await
        .map_err(|e| format!("the app host did not answer ({}): {e}", app_host_url()))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(format!(
            "the app host answered {status}: {}",
            body.chars().take(300).collect::<String>()
        ));
    }
    Ok(())
}

/// Replay one fixture through the app host, bounded by `deadline`.
pub async fn replay_case(
    token: &str,
    req: &AppCaseRequest,
    deadline: Duration,
) -> Result<AppCaseReply, String> {
    let res = CLIENT
        .post(format!("{}{}", app_host_url(), case_path(req)))
        .header(talaria_config::INTERNAL_HEADER, token)
        .json(&serde_json::json!({ "model": req.model }))
        .timeout(deadline)
        .send()
        .await
        .map_err(|e| format!("the app host did not answer: {e}"))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        let said = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| body.chars().take(300).collect());
        return Err(format!("the app host answered {status}: {said}"));
    }
    res.json::<AppCaseReply>()
        .await
        .map_err(|e| format!("the app host's replay did not parse: {e}"))
}

/// The listing, held this long: the matrix polls every few seconds and asks
/// for the registry each time, and an app's harness set changes only on a
/// rebuild or an enable.
const LAYER_TTL: Duration = Duration::from_secs(3);
/// A missing app layer is logged at most this often.
const WARN_EVERY: Duration = Duration::from_secs(60);

struct Cached {
    at: Instant,
    layer: Arc<AppLayer>,
}

static CACHE: LazyLock<Mutex<Option<Cached>>> = LazyLock::new(|| Mutex::new(None));
/// Every app display name the host has told this process, by slug — read
/// synchronously by the fitness plane when it labels an app's column.
static NAMES: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static WARNED: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// The app layer, fetched (or the copy fetched in the last `LAYER_TTL`).
pub async fn app_layer(token: &str) -> Result<Arc<AppLayer>, String> {
    {
        let cached = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(c) = cached.as_ref()
            && c.at.elapsed() < LAYER_TTL
        {
            return Ok(c.layer.clone());
        }
    }
    let listing = fetch_listing(token).await?;
    let layer = Arc::new(app_layer_of(&listing));
    NAMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend(layer.app_names.iter().map(|(k, v)| (k.clone(), v.clone())));
    for p in &layer.problems {
        tracing::debug!("[app-harnesses] {p}");
    }
    *CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Cached {
        at: Instant::now(),
        layer: layer.clone(),
    });
    Ok(layer)
}

/// THE REGISTRY THE FITNESS PLANE READS: the builtins, then every enabled
/// app's harnesses. The app host unreachable (an api running alone, a host
/// mid-restart) leaves the builtins — logged, once a minute at most.
pub async fn registry_with_apps(token: &str) -> Vec<RegisteredHarness> {
    match app_layer(token).await {
        Ok(layer) => merged_registry(&layer),
        Err(e) => {
            let mut warned = WARNED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if warned.is_none_or(|at| at.elapsed() >= WARN_EVERY) {
                tracing::warn!("[app-harnesses] the fitness registry has only the builtins: {e}");
                *warned = Some(Instant::now());
            }
            builtin_activity_harnesses().to_vec()
        }
    }
}

/// An app's display name, as the app host last listed it — None before the
/// host has been asked, or for an app it never named.
pub fn app_display_name(slug: &str) -> Option<String> {
    NAMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(slug)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(app: &str, id: &str) -> AppHarnessWire {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "label": "Support triage",
            "job": "Grades an inbound support message.",
            "source": format!("app:{app}"),
            "app": app,
            "appName": "Support",
            "file": "triage.ts",
            "requires": ["json", "instruction-following"],
            "floor": { "capabilities": ["json"], "refuseBelow": true, "note": "Needs JSON." },
            "outputKind": "json",
            "repair": 1,
            "verifies": false,
            "tools": "none",
            "guard": { "rules": ["secret_leak"], "redact": true },
            "widen": null,
            "temperature": 0.2,
            "model": { "pin": "titler", "role": null, "chain": null },
            "evals": [
                { "name": "an outage grades high", "band": "standard" },
                { "name": "a typo grades low", "band": "easy" }
            ]
        }))
        .expect("the host's wire shape parses")
    }

    #[test]
    fn registers_an_app_harness_as_metadata_with_its_source() {
        let h = register(&wire("support", "support:triage")).expect("registers");
        assert_eq!(h.def.id, "support:triage");
        assert_eq!(h.source, HarnessSource::App("support"));
        assert_eq!(h.app_slug(), Some("support"));
        assert!(h.def.output.is_json());
        assert_eq!(h.def.requires, ["json", "instruction-following"]);
        assert_eq!(h.def.floor.capabilities, ["json"]);
        assert!(h.def.floor.refuse_below);
        assert_eq!(
            h.eval_names().collect::<Vec<_>>(),
            ["an outage grades high", "a typo grades low"]
        );
        assert_eq!(h.band_of("a typo grades low"), EvalBand::Easy);
        assert_eq!(h.def.temperature, Some(0.2));
        assert_eq!(
            h.def.guard.as_ref().and_then(|g| g.rules.clone()),
            Some(vec!["secret_leak"])
        );
        // The pin the app declared binds nothing here: its column is its own.
        assert_eq!(h.def.model.pin, None);
        assert!(h.def.model.chain.is_some_and(|c| c.is_empty()));
        assert_eq!(talaria_harness_defs::registry::platform_agent_of(&h), None);
        // Its own model setting: the slot the app host stamps on every run,
        // derived from the slug and the id — the key the fitness column and
        // `app_harness_models` share.
        assert_eq!(h.def.model.slot, Some("app:support/support:triage"));
        assert_eq!(
            app_harness_slot("support", "support:triage"),
            "app:support/support:triage"
        );
    }

    #[test]
    fn never_runs_app_code_in_the_api() {
        let h = register(&wire("support", "support:triage")).expect("registers");
        let ctx = talaria_harness::define::RenderContext {
            widened: false,
            model: "m".into(),
        };
        assert!((h.def.render)(&Value::Null, &ctx).is_err());
        let check = &h.def.evals[0].check;
        let ctx = talaria_harness::define::CheckCtx {
            calls: Vec::new(),
            world: None,
            exhausted: false,
        };
        assert!(matches!(check(&Value::Null, &ctx), CheckResult::Gap(_)));
    }

    #[test]
    fn interns_one_definition_per_distinct_wire() {
        let a = register(&wire("support", "support:intern")).expect("registers");
        let b = register(&wire("support", "support:intern")).expect("registers");
        assert!(
            std::ptr::eq(a.def, b.def),
            "a repeat listing must not leak a second copy"
        );
        let mut changed = wire("support", "support:intern");
        changed.label = "Support triage, renamed".into();
        let c = register(&changed).expect("registers");
        assert!(!std::ptr::eq(a.def, c.def));
        assert_eq!(c.def.label, "Support triage, renamed");
    }

    #[test]
    fn refuses_what_the_api_cannot_hold() {
        let mut w = wire("support", "support:triage");
        w.source = "app:other".into();
        assert!(register(&w).is_err());
        let mut w = wire("support", "support:triage");
        w.id = "has space".into();
        assert!(register(&w).is_err());
        let mut w = wire("Not A Slug", "support:triage");
        w.source = "app:Not A Slug".into();
        assert!(register(&w).is_err());
        let mut w = wire("support", "support:triage");
        w.evals.push(w.evals[0].clone());
        assert!(register(&w).is_err());
        let mut w = wire("support", "support:triage");
        w.output_kind = "yaml".into();
        assert!(register(&w).is_err());
    }

    #[test]
    fn merges_the_listing_after_the_builtins_and_keeps_every_problem() {
        let listing = AppHarnessListing {
            harnesses: vec![
                wire("alpha", "alpha:sort"),
                wire("beta", "titler"),
                wire("zeta", "alpha:sort"),
            ],
            problems: vec![WireProblem {
                app: "beta".into(),
                file: "broken.ts".into(),
                id: Some("beta:broken".into()),
                reason: "guard rule \"nope\" does not exist".into(),
            }],
        };
        let layer = app_layer_of(&listing);
        assert_eq!(layer.harnesses.len(), 3);
        assert_eq!(
            layer.problems,
            ["app \"beta\": harnesses/broken.ts skipped — guard rule \"nope\" does not exist"]
        );
        assert_eq!(
            layer.app_names.get("alpha").map(String::as_str),
            Some("Support")
        );
        let merged = merged_registry(&layer);
        let builtins = builtin_activity_harnesses();
        assert_eq!(merged.len(), builtins.len() + 1);
        // The builtin titler stays the builtin; the second claim on alpha:sort
        // is refused; the first stays.
        let titler = merged
            .iter()
            .find(|h| h.def.id == "titler")
            .expect("titler");
        assert_eq!(titler.source, HarnessSource::Builtin);
        let last = merged.last().expect("an app harness");
        assert_eq!(
            (last.def.id, last.app_slug()),
            ("alpha:sort", Some("alpha"))
        );
    }

    #[test]
    fn one_entry_the_api_cannot_read_costs_that_harness_not_the_listing() {
        let good = serde_json::to_value(wire("alpha", "alpha:sort")).expect("wire");
        let mut overflow = serde_json::to_value(wire("beta", "beta:big")).expect("wire");
        overflow["repair"] = serde_json::json!(5_000_000_000_u64);
        overflow["file"] = serde_json::json!("big.ts");
        // What `Math.floor(NaN)` and `Infinity` serialise to.
        let mut nan = serde_json::to_value(wire("beta", "beta:nan")).expect("wire");
        nan["repair"] = Value::Null;
        nan["file"] = serde_json::json!("nan.ts");
        let mut no_floor = serde_json::to_value(wire("gamma", "gamma:x")).expect("wire");
        no_floor.as_object_mut().expect("object").remove("floor");
        let body = serde_json::json!({
            "harnesses": [overflow, good, nan, no_floor, 42],
            "problems": [
                { "app": "beta", "file": "broken.ts", "id": null, "reason": "it failed to compile" },
                { "not": "a problem" }
            ]
        });
        let listing = listing_of(&body).expect("the listing decodes");
        assert_eq!(
            listing
                .harnesses
                .iter()
                .map(|w| w.id.as_str())
                .collect::<Vec<_>>(),
            ["alpha:sort"]
        );
        let named = listing
            .problems
            .iter()
            .map(|p| (p.app.as_str(), p.file.as_str(), p.id.as_deref()))
            .collect::<Vec<_>>();
        assert_eq!(
            named,
            [
                ("beta", "broken.ts", None),
                ("beta", "big.ts", Some("beta:big")),
                ("beta", "nan.ts", Some("beta:nan")),
                ("gamma", "triage.ts", Some("gamma:x")),
                ("?", "?", None),
            ]
        );
        assert!(
            listing.problems[1]
                .reason
                .starts_with("the api could not read its listing entry")
        );
        // The good harness still registers beside the builtins.
        let merged = merged_registry(&app_layer_of(&listing));
        assert!(merged.iter().any(|h| h.def.id == "alpha:sort"));
        assert_eq!(merged.len(), builtin_activity_harnesses().len() + 1);
    }

    #[test]
    fn refuses_only_a_listing_that_is_not_one() {
        assert!(listing_of(&serde_json::json!([])).is_err());
        assert!(listing_of(&serde_json::json!({ "harnesses": {} })).is_err());
        assert_eq!(
            listing_of(&serde_json::json!({})).expect("empty"),
            AppHarnessListing::default()
        );
    }

    #[test]
    fn addresses_a_fixture_by_encoded_segments() {
        let path = case_path(&AppCaseRequest {
            app: "support".into(),
            harness: "support:triage".into(),
            case: "an outage / grades high".into(),
            model: "m".into(),
        });
        assert_eq!(
            path,
            "/api/app-host/support/harnesses/support%3Atriage/cases/an%20outage%20%2F%20grades%20high"
        );
    }

    #[test]
    fn reads_a_replay_the_host_answers() {
        let reply: AppCaseReply = serde_json::from_value(serde_json::json!({
            "harness": "support:triage",
            "case": "an outage grades high",
            "band": "standard",
            "model": "mock-small",
            "value": { "severity": "high" },
            "verdict": "pass",
            "reason": null,
            "schemaValid": true,
            "repairs": 0,
            "answered": true,
            "refused": false,
            "error": null,
            "latencyMs": 12,
            "findings": 0,
            "raw": "{\"severity\":\"high\"}",
            "prompt": "system: grade",
            "promptTokens": 30,
            "completionTokens": 8,
            "estimated": false
        }))
        .expect("the host's replay shape parses");
        assert_eq!(reply.verdict.as_deref(), Some("pass"));
        assert!(reply.schema_valid);
        assert_eq!(reply.prompt_tokens + reply.completion_tokens, 38);
    }
}
