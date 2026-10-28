// Agent harness — the agent-definition WRITE core: identity alphabets,
// versioned configs, and the create path's row plumbing. The endpoint CRUD
// half lives in gateway/registry.rs, alongside the relay's read of it.
//
// A CONFIG IS DATA, NOT A CONTRACT: every signature here moves the version's
// `config` jsonb as a `serde_json::Value`, the way fleet.rs already reads it.
// A typed struct would silently DROP unknown keys on round-trip — a merge
// keeps whatever a future field adds — and the three things
// the write path does with a config (build it, compare it, store it) all
// want the tree, not a schema.
//
// serde_json's map key order is alphabetical, not insertion. Nothing here
// can observe it —
// `canonical` sorts keys before comparing, and the config tree feeds a YAML
// emitter whose consumers parse mappings order-insensitively.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgPool;
use std::sync::OnceLock;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_gateway::registry::{LlmEndpoint, list_endpoints};

/// Agent slug / department alphabets. `create_agent`
/// has always enforced these; they live HERE because every path that accepts
/// an agent identity needs the same bar — a slug reaches an .env line
/// (`HERMES_KEY_<SLUG>` — a `\n` injects a line), path joins (a `../`
/// traverses), and a regex interpolation.
pub fn slug_ok(slug: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9]{1,30}$").unwrap())
        .is_match(slug)
}

pub fn dept_ok(department: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9-]{1,40}$").unwrap())
        .is_match(department)
}

/// The Hermes toolsets Talaria governs — the switches Hermes' own `hermes
/// tools` offers (`CONFIGURABLE_TOOLSETS` in hermes_cli/tools_config.py), in
/// alphabetical order, minus the four no Talaria agent is ever wired to:
/// `stt` (a provider setting with its own `stt.enabled` switch, not a model
/// toolset) and the platform tools `discord`, `discord_admin` and `yuanbao`.
/// `scripts/hermes-toolsets.json` is the checked-in snapshot of Hermes'
/// list; a test below holds this constant to it and the chassis smoke holds
/// the snapshot to the live image, so the day Hermes grows a switch both say
/// so.
///
/// What is NOT here, and why it must not be: the composites (`safe`,
/// `debugging`, `search`), the `coding` posture, the `hermes-*` bundles and
/// the GUI client surfaces (`project`, `desktop_ui`, `onboarding`,
/// `catalog`). Hermes disables tool by tool, so unticking a composite strips
/// its MEMBERS — `safe` took web search, vision and image generation with
/// it while their own rows stayed ticked — a posture or a bundle strips only
/// its non-core delta (nothing, for `coding`), and a client surface never
/// reaches an API-server session at all. A switch that does something other
/// than what its row says is worse than no switch. Names that used to be
/// here are [`RETIRED_TOOLSETS`].
///
/// A name outside this list is a typo or a Hermes version nobody has read
/// yet, and either way it reaches `agent.disabled_toolsets` in the rendered
/// config.yaml, where Hermes IGNORES it in silence: the operator would see a
/// toolset they believe is closed and an agent that still has it. The write
/// path refuses the stranger instead — the same bar, for the same reason, as
/// the alphabets above.
pub const GOVERNED_TOOLSETS: &[&str] = &[
    "browser",
    "clarify",
    "code_execution",
    "computer_use",
    "connections",
    "context_engine",
    "cronjob",
    "delegation",
    "file",
    "image_gen",
    "kanban",
    "memory",
    "session_search",
    "skills",
    "terminal",
    "todo",
    "tts",
    "video",
    "video_gen",
    "vision",
    "web",
    "x_search",
];

/// The names [`GOVERNED_TOOLSETS`] carried before it was cut to Hermes' own
/// switches, each with the governed toolsets that close at least what it
/// closed in Hermes (`toolsets.py`): `debugging` is terminal + process with
/// web and file folded in, `safe` folds in web, vision and image_gen, and
/// `search` is web_search alone — `web` closes that and web_extract beside
/// it, the stricter reading, because a list that said "closed" must not come
/// back open. `coding` (a posture), `project` and `desktop_ui` (GUI client
/// surfaces) closed nothing on an API-server agent, so they map to nothing.
///
/// A stored def, an org floor, the floor's env twin, or an older editor tab
/// can still spell one of these; [`toolset_names`] expands it rather than
/// refusing what used to be a valid list or, worse, dropping a closed name.
pub const RETIRED_TOOLSETS: &[(&str, &[&str])] = &[
    ("coding", &[]),
    ("debugging", &["terminal", "web", "file"]),
    ("desktop_ui", &[]),
    ("project", &[]),
    ("safe", &["web", "vision", "image_gen"]),
    ("search", &["web"]),
];

/// One raw name as the governed names it stands for: itself when governed,
/// its expansion when retired ([`RETIRED_TOOLSETS`]), `None` for a stranger.
/// Trims; the caller owns dedupe and order.
pub fn toolset_names(raw: &str) -> Option<Vec<&'static str>> {
    let name = raw.trim();
    if let Some(governed) = GOVERNED_TOOLSETS.iter().find(|g| **g == name) {
        return Some(vec![*governed]);
    }
    RETIRED_TOOLSETS
        .iter()
        .find(|(old, _)| *old == name)
        .map(|(_, members)| members.to_vec())
}

/// The two Talaria takes off every agent's table whatever a def says, because
/// Talaria already does both jobs and only its versions reach a person: the
/// MCP todo tool lands the chips someone can see and act on, and the MCP
/// clarify tool puts a question in front of them. The renderer merges these
/// ahead of an agent's own list (talaria-fleet-render's
/// `fleet_disabled_toolsets`), so storing them per agent would be noise —
/// they are the floor, not a choice.
pub const MANDATORY_DISABLED: &[&str] = &["todo", "clarify"];

/// The CLOSED DEFAULT for a new agent: what a fresh def's
/// `disabled_toolsets` starts as. A hired agent gets no shell, no code
/// execution, no file tool and no self-scheduling until a manager opens them
/// one by one — the container env carries the agent's own credential, the
/// Hermes API key and the fleet gateway key, and an agent that can `env |
/// curl` on a prompt-injected page needed exactly none of these to do the
/// job it was hired for. `browser`, `delegation` and `memory` stay on: they
/// are what an ordinary agent works with. A Developer Agent is the
/// exception ([`DEVELOPER_TOOLSETS`]): the switch opens the three the
/// sandbox is for, and turning it off closes them again.
pub const DEFAULT_DISABLED_FOR_NEW: &[&str] = &["terminal", "code_execution", "file", "cronjob"];

/// What the Developer Agent switch opens and closes — the toolsets a dev
/// sandbox cannot do without. `cronjob` is NOT among them: a developer still
/// does not schedule itself unless a manager says so.
pub const DEVELOPER_TOOLSETS: &[&str] = &["terminal", "file", "code_execution"];

/// Closed by default on an agent that serves MANY people (no
/// `owner_user_id`): with pinned sessions every conversation of every user
/// is a Hermes session on the agent's state volume, and `session_search`
/// would let the agent recall Alice's thread while answering Bob. A personal
/// assistant works for one person, so its recall is that person's own.
pub const SHARED_AGENT_DISABLED: &[&str] = &["session_search"];

/// The org-level floor: an `app_settings` row (a JSON array of governed
/// names) every render merges beside [`MANDATORY_DISABLED`], so an operator
/// can close a toolset for the whole fleet without visiting each agent. The
/// admin settings PUT writes it (`mandatoryDisabledToolsets`, governed names
/// only, the pair implied and never stored) and the admin Security panel
/// edits it; the env var below is the read-only twin for an install that
/// would rather pin it in the process environment. Both are read; the union
/// is the floor.
pub const ORG_FLOOR_SETTING: &str = "agents.mandatory_disabled_toolsets";
pub const ORG_FLOOR_ENV: &str = "TALARIA_MANDATORY_DISABLED_TOOLSETS";

/// The disabled list a brand-new def starts with: the closed default, plus
/// `session_search` unless the agent is somebody's personal assistant.
pub fn default_disabled_for_new(personal: bool) -> Vec<String> {
    let mut out: Vec<String> = DEFAULT_DISABLED_FOR_NEW
        .iter()
        .map(|n| n.to_string())
        .collect();
    if !personal {
        out.extend(SHARED_AGENT_DISABLED.iter().map(|n| n.to_string()));
    }
    out
}

/// The whole floor, pure: [`MANDATORY_DISABLED`], then the stored org list,
/// then the env list — governed names only (a stranger is logged and
/// dropped, never rendered for Hermes to ignore in silence), deduped,
/// floor-first so the rendered config reads the same way every render.
pub fn merge_org_floor(stored: &Value, env: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = MANDATORY_DISABLED.iter().map(|n| n.to_string()).collect();
    let stored_names: Vec<String> = stored
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let env_names: Vec<String> = env
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for raw in stored_names.into_iter().chain(env_names) {
        // A retired name expands to what it closed — dropping it as a
        // stranger would REOPEN those toolsets fleet-wide.
        let Some(names) = toolset_names(&raw) else {
            tracing::warn!(
                "[agent-defs] org toolset floor names \"{}\", which is not a governed toolset — ignored",
                raw.trim()
            );
            continue;
        };
        for name in names {
            if !out.iter().any(|kept| kept == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The floor the renderer merges ahead of every agent's own list:
/// [`MANDATORY_DISABLED`] plus the org floor ([`ORG_FLOOR_SETTING`] and
/// [`ORG_FLOOR_ENV`]). Never fails — an unreadable setting is the two-name
/// floor a fresh install has.
pub async fn mandatory_disabled(pg: &PgPool) -> Vec<String> {
    let stored = talaria_gateway::settings::get_setting(pg, ORG_FLOOR_SETTING, Value::Null).await;
    let env = std::env::var(ORG_FLOOR_ENV).ok();
    merge_org_floor(&stored, env.as_deref())
}

/// Take names OFF an agent's disabled list (open the toolsets). Returns
/// whether the stored list changed. The jsonb is rewritten whole so the
/// column keeps its wire shape (a deduped array of governed names).
pub async fn open_toolsets(pg: &PgPool, id: &str, names: &[&str]) -> Result<bool, sqlx::Error> {
    let current = disabled_toolsets_of(pg, id).await?;
    let next: Vec<String> = current
        .iter()
        .filter(|n| !names.contains(&n.as_str()))
        .cloned()
        .collect();
    if next == current {
        return Ok(false);
    }
    set_disabled_toolsets(pg, id, &next).await
}

/// Put names ON an agent's disabled list (close the toolsets). Returns
/// whether the stored list changed.
pub async fn close_toolsets(pg: &PgPool, id: &str, names: &[&str]) -> Result<bool, sqlx::Error> {
    let current = disabled_toolsets_of(pg, id).await?;
    let mut next = current.clone();
    for name in names {
        if !next.iter().any(|n| n == name) {
            next.push(name.to_string());
        }
    }
    if next == current {
        return Ok(false);
    }
    set_disabled_toolsets(pg, id, &next).await
}

/// The Developer Agent switch's toolset half: on opens
/// [`DEVELOPER_TOOLSETS`], off closes them again. Returns whether the list
/// changed, for the audit row.
pub async fn apply_developer_toolsets(
    pg: &PgPool,
    id: &str,
    on: bool,
) -> Result<bool, sqlx::Error> {
    if on {
        open_toolsets(pg, id, DEVELOPER_TOOLSETS).await
    } else {
        close_toolsets(pg, id, DEVELOPER_TOOLSETS).await
    }
}

async fn disabled_toolsets_of(pg: &PgPool, id: &str) -> Result<Vec<String>, sqlx::Error> {
    let row: Option<(sqlx::types::Json<Vec<String>>,)> =
        sqlx::query_as("select disabled_toolsets from agent_defs where id = $1::uuid")
            .bind(id)
            .fetch_optional(pg)
            .await?;
    Ok(row.map(|(j,)| j.0).unwrap_or_default())
}

/// The longest list a caller may hand us. Nothing can legitimately exceed
/// GOVERNED_TOOLSETS' own length once duplicates are gone, so this is a bound
/// on the WALK, not on the answer: a request cannot make this validate a
/// thousand names before refusing them.
const MAX_TOOLSETS: usize = 40;

/// Clean and bless a disabled-toolsets list (fleet.defs.$id PATCH): trim each
/// name, expand a retired one into what it closed ([`RETIRED_TOOLSETS`]),
/// drop repeats keeping the first, and refuse anything else outside
/// [`GOVERNED_TOOLSETS`] — naming the first stranger, because an operator who
/// mistyped one of eight names needs to know WHICH.
pub fn validate_toolsets(names: &[String]) -> Result<Vec<String>, String> {
    if names.len() > MAX_TOOLSETS {
        return Err(format!("at most {MAX_TOOLSETS} toolsets"));
    }
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for raw in names {
        let Some(expanded) = toolset_names(raw) else {
            return Err(format!("unknown toolset \"{}\"", raw.trim()));
        };
        for name in expanded {
            if !out.iter().any(|kept| kept == name) {
                out.push(name.to_string());
            }
        }
    }
    Ok(out)
}

/// A model target: which endpoint serves it and the upstream model id there.
/// `contextLength`/`effort` are ABSENT when
/// unset — skip_serializing_if, not null — so the jsonb round-trips only the
/// keys that were set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTarget {
    pub endpoint: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// One alias target — a ModelTarget with the tier's name on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasTarget {
    pub name: String,
    #[serde(flatten)]
    pub target: ModelTarget,
}

/// One nullable member of an edit: absent from the request (leave whatever is
/// stored), null (drop the key), or a value (write it). The three-way
/// distinction is the whole point — "the caller said nothing" and "the caller
/// said none" are different instructions, and a plain `Option` can only carry
/// one of them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Knob<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

/// The compaction knobs an editor turns on an agent's Hermes compression
/// block (`raw.compression`): the ratio of the context window at which Hermes
/// starts folding old turns away, and the absolute token ceiling that can trip
/// it first. EVERY OTHER KEY IN THE BLOCK IS THE DEF AUTHOR'S — `enabled`,
/// `protect_last_n`, `min_tail_user_messages`, `progress_notices` stay exactly
/// as written, because the renderer emits the block verbatim and a form that
/// rewrote it whole would silently revert hand-authored config.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompressionEdits {
    pub threshold: Knob<f64>,
    pub threshold_tokens: Knob<i64>,
}

/// The structured edit a caller applies onto a previous config.
#[derive(Debug, Clone)]
pub struct ConfigEdits {
    pub main: ModelTarget,
    pub aliases: Vec<AliasTarget>,
    pub fallbacks: Vec<ModelTarget>,
    /// The compaction knobs. Default is both `Keep`: a caller that says
    /// nothing about compression leaves the stored block untouched.
    pub compression: CompressionEdits,
}

/// The current compaction knobs off a stored config, for the editor that
/// turns them: `{ threshold, thresholdTokens }`, or null when the def carries
/// no `raw.compression` block at all. An absent block means Hermes' own
/// defaults are in force (0.50 of the window, 256K tokens) and the form is
/// what says so — this does not invent numbers the def never wrote. A member
/// that is stored as something other than a number reads as null for the same
/// reason: the wire type is a promise.
pub fn compression_wire(config: &Value) -> Value {
    let Some(block) = config
        .get("raw")
        .and_then(|raw| raw.get("compression"))
        .and_then(Value::as_object)
    else {
        return Value::Null;
    };
    let num = |key: &str| {
        block
            .get(key)
            .and_then(Value::as_f64)
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number)
    };
    serde_json::json!({
        "threshold": num("threshold"),
        "thresholdTokens": block.get("threshold_tokens").and_then(Value::as_i64).map_or(Value::Null, |n| serde_json::json!(n)),
    })
}

/// A version row, the columns the create path reads (timestamps nothing
/// here touches are left out).
#[derive(Debug, Clone)]
pub struct AgentVersionRow {
    pub id: String,
    pub agent_id: String,
    pub version: i32,
    pub soul: String,
    pub config: Value,
    pub note: Option<String>,
    pub created_by: Option<String>,
    /// ISO via epoch-ms.
    pub created_at: String,
}

/// A def row, the columns the create path hands around (the run's HiredDef
/// and the renderer's RenderTarget grow out of this). `managed` is carried
/// because create sets it in a second statement and the caller must see the
/// set value, not the insert default.
#[derive(Debug, Clone)]
pub struct AgentDefRow {
    pub id: String,
    pub slug: String,
    pub department: String,
    pub model: String,
    pub display_name: String,
    pub role: Option<String>,
    pub source: String,
    pub managed: bool,
    pub current_version: i32,
}

/// The def row as SQL hands it back — one alias so every reader maps the
/// same nine columns through the same constructor.
type DefTuple = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    bool,
    i32,
);

fn def_row(
    (id, slug, department, model, display_name, role, source, managed, current_version): DefTuple,
) -> AgentDefRow {
    AgentDefRow {
        id,
        slug,
        department,
        model,
        display_name,
        role,
        source,
        managed,
        current_version,
    }
}

// Literal SQL, not format!: the column list appears in each statement so the
// query is auditable as written (sqlx's dynamic-SQL lint exists for this).
pub async fn agent_def_by_slug(
    pg: &PgPool,
    slug: &str,
) -> Result<Option<AgentDefRow>, sqlx::Error> {
    let row: Option<DefTuple> = sqlx::query_as(
        "select id::text, slug, department, model, display_name, role, source, managed, current_version \
         from agent_defs where slug = $1",
    )
    .bind(slug)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(def_row))
}

pub async fn agent_def_by_id(pg: &PgPool, id: &str) -> Result<Option<AgentDefRow>, sqlx::Error> {
    let row: Option<DefTuple> = sqlx::query_as(
        "select id::text, slug, department, model, display_name, role, source, managed, current_version \
         from agent_defs where id = $1::uuid",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(def_row))
}

/// Every def with its LATEST version inline, the harness
/// registry's read. The def objects ride raw Values keyed in the SELECT's
/// order (jsonb passthroughs keep the stored order; the iso helper prints
/// timestamps as ISO strings). Twenty columns — past
/// sqlx's tuple arity, so the row lands in a FromRow struct.
#[derive(sqlx::FromRow)]
struct DefListRow {
    id: String,
    slug: String,
    department: String,
    model: String,
    display_name: String,
    role: Option<String>,
    email_alias: Option<String>,
    owner_user_id: Option<String>,
    enabled: bool,
    managed: bool,
    source: String,
    developer: bool,
    max_children: i32,
    pinned_sessions: bool,
    /// The jsonb list rides out as the Value it is stored as — a passthrough,
    /// like `config` above it: the column is written only through
    /// `set_disabled_toolsets`, so its shape is already the wire's.
    disabled_toolsets: Value,
    ticket_template_id: Option<String>,
    plan_template_id: Option<String>,
    current_version: i32,
    created_ms: i64,
    updated_ms: i64,
}

pub async fn list_agent_defs_wire(pg: &PgPool) -> Result<Vec<Value>, sqlx::Error> {
    let defs: Vec<DefListRow> = sqlx::query_as(
        "select id::text, slug, department, model, display_name, role, email_alias::text, \
           owner_user_id::text, enabled, managed, source, developer, max_children, \
           pinned_sessions, disabled_toolsets, ticket_template_id::text, plan_template_id::text, \
           current_version, (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
           (trunc(extract(epoch from updated_at) * 1000))::bigint as updated_ms \
         from agent_defs order by slug asc",
    )
    .fetch_all(pg)
    .await?;
    // distinct on (agent_id) … version desc — one row per agent, the newest.
    let versions: Vec<(
        String,
        String,
        i32,
        String,
        Value,
        Option<String>,
        Option<String>,
        i64,
    )> = sqlx::query_as(
        "select distinct on (agent_id) id::text, agent_id::text, version, soul, config, note, \
               created_by, (trunc(extract(epoch from created_at) * 1000))::bigint \
             from agent_versions order by agent_id, version desc",
    )
    .fetch_all(pg)
    .await?;
    let latest_of: std::collections::HashMap<String, Value> = versions
        .into_iter()
        .map(
            |(id, agent_id, version, soul, config, note, created_by, created_ms)| {
                let wire = serde_json::json!({
                    "id": id,
                    "agentId": agent_id,
                    "version": version,
                    "soul": soul,
                    "config": config,
                    "note": note,
                    "createdBy": created_by,
                    "createdAt": epoch_ms_to_iso(created_ms),
                });
                (agent_id, wire)
            },
        )
        .collect();
    let shared = shared_departments(pg).await?;
    Ok(defs
        .into_iter()
        .map(|d| {
            let latest = latest_of.get(&d.id).cloned().unwrap_or(Value::Null);
            serde_json::json!({
                "id": d.id,
                "slug": d.slug,
                "department": d.department,
                "model": d.model,
                "displayName": d.display_name,
                "role": d.role,
                "emailAlias": d.email_alias,
                "ownerUserId": d.owner_user_id,
                "enabled": d.enabled,
                "managed": d.managed,
                "source": d.source,
                "developer": d.developer,
                "maxChildren": d.max_children,
                "pinnedSessions": d.pinned_sessions,
                "disabledToolsets": d.disabled_toolsets,
                "ticketTemplateId": d.ticket_template_id,
                "planTemplateId": d.plan_template_id,
                "currentVersion": d.current_version,
                // The compaction knobs off the latest version's config — the
                // Config tab reads the current values from here, and null
                // means "the def says nothing, Hermes' defaults are in force".
                "compression": compression_wire(latest.get("config").unwrap_or(&Value::Null)),
                // Null unless another agent holds this def's department too
                // (only ever a pair that predates the one-agent rule): then
                // who runs in the department's container, and who else holds
                // it — what the roster and the agent page warn with.
                "departmentShare": department_share_wire(&shared, &d.id, &d.department),
                "createdAt": epoch_ms_to_iso(d.created_ms),
                "updatedAt": epoch_ms_to_iso(d.updated_ms),
                "latest": latest,
            })
        })
        .collect())
}

/// Flip "pinned sessions" (fleet.defs.$id PATCH). A pinned agent is driven
/// through Hermes' session API — one Hermes session whose id is the
/// conversation id, one new message per turn, Hermes holding the history —
/// so Hermes' compaction, session_search and /compress see the whole
/// conversation instead of the fragments a shifting system prompt files it
/// under. Returns whether the stored value changed, for the audit row.
pub async fn set_pinned_sessions(pg: &PgPool, id: &str, on: bool) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(
        "update agent_defs set pinned_sessions = $1, updated_at = now() \
         where id = $2::uuid and pinned_sessions is distinct from $1",
    )
    .bind(on)
    .bind(id)
    .execute(pg)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// Replace an agent's disabled-toolsets list (fleet.defs.$id PATCH), the list
/// the renderer merges on top of [`MANDATORY_DISABLED`]. The caller passes it
/// through [`validate_toolsets`] first — this writes what it is given. Whole
/// list, not a delta: the UI edits a set of checkboxes and the column is what
/// they show. Returns whether the stored value changed, for the audit row and
/// for the roll (a re-save of the same set must not bounce a container).
pub async fn set_disabled_toolsets(
    pg: &PgPool,
    id: &str,
    names: &[String],
) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(
        "update agent_defs set disabled_toolsets = $1, updated_at = now() \
         where id = $2::uuid and disabled_toolsets is distinct from $1",
    )
    .bind(serde_json::json!(names))
    .bind(id)
    .execute(pg)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// Whether the agent behind a model id is pinned — the chat path's question,
/// asked by the identity it routes on. An unknown model is not pinned: the
/// full-history path is the one every agent speaks.
pub async fn pinned_sessions_for_model(pg: &PgPool, model: &str) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> =
        sqlx::query_as("select pinned_sessions from agent_defs where model = $1")
            .bind(model)
            .fetch_optional(pg)
            .await?;
    Ok(row.is_some_and(|(on,)| on))
}

/// One agent def by id — the getAgentDef wire shape (the same columns the
/// list emits, minus `latest`: the versions route pairs this with the
/// history it is already reading).
pub async fn get_agent_def_wire(pg: &PgPool, id: &str) -> Result<Option<Value>, sqlx::Error> {
    let d: Option<DefListRow> = sqlx::query_as(
        "select id::text, slug, department, model, display_name, role, email_alias::text, \
           owner_user_id::text, enabled, managed, source, developer, max_children, \
           pinned_sessions, disabled_toolsets, ticket_template_id::text, plan_template_id::text, \
           current_version, (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
           (trunc(extract(epoch from updated_at) * 1000))::bigint as updated_ms \
         from agent_defs where id = $1::uuid",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(d.map(|d| {
        serde_json::json!({
            "id": d.id,
            "slug": d.slug,
            "department": d.department,
            "model": d.model,
            "displayName": d.display_name,
            "role": d.role,
            "emailAlias": d.email_alias,
            "ownerUserId": d.owner_user_id,
            "enabled": d.enabled,
            "managed": d.managed,
            "source": d.source,
            "developer": d.developer,
            "maxChildren": d.max_children,
            "pinnedSessions": d.pinned_sessions,
            "disabledToolsets": d.disabled_toolsets,
            "ticketTemplateId": d.ticket_template_id,
            "planTemplateId": d.plan_template_id,
            "currentVersion": d.current_version,
            "createdAt": epoch_ms_to_iso(d.created_ms),
            "updatedAt": epoch_ms_to_iso(d.updated_ms),
        })
    }))
}

type VersionTuple = (
    String,
    String,
    i32,
    String,
    Value,
    Option<String>,
    Option<String>,
    i64,
);

fn version_row(
    (id, agent_id, version, soul, config, note, created_by, created_ms): VersionTuple,
) -> AgentVersionRow {
    AgentVersionRow {
        id,
        agent_id,
        version,
        soul,
        config,
        note,
        created_by,
        created_at: epoch_ms_to_iso(created_ms),
    }
}

/// Versions newest-first (listVersions).
pub async fn list_versions(
    pg: &PgPool,
    agent_id: &str,
) -> Result<Vec<AgentVersionRow>, sqlx::Error> {
    let rows: Vec<VersionTuple> = sqlx::query_as(
        "select id::text, agent_id::text, version, soul, config, note, created_by, \
             (trunc(extract(epoch from created_at) * 1000))::bigint \
             from agent_versions where agent_id = $1::uuid order by version desc",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(version_row).collect())
}

// ── A department is one agent's key ──────────────────────────────────────────
//
// The department names the compose service (`agent-<department>`), the
// Hermes data volume (`<project>_hermes-<department>`), the roll lock and the
// model id (`<slug>-<department>`). Two agents in one department are one
// container and one Hermes home: whichever renders last takes the service,
// and both write the same memory. So a department belongs to ONE agent, from
// its hire until it is deleted (a retired agent keeps its volume, so it keeps
// its department too).
//
// No unique index says so. docs/UPDATES.md lets a roll-safe migration only
// add, and an index build fails outright on an install that already holds two
// agents in one department — which is exactly the install this rule is for.
// So the writer checks under a per-department advisory lock
// ([`upsert_agent_def`]), every writer goes through it, and a pair that
// predates the rule is reported ([`shared_departments`]) and never merged: the
// render runs the older agent in the department's container and leaves the
// other out, and the roster, the agent page and the alerts say so.

/// The sentence every writer refuses a held department with. Its tail is the
/// part [`is_department_taken`] recognises, so a caller that only holds the
/// error as a string (the hire run, the personal-assistant route) can still
/// answer 409.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepartmentTaken {
    pub department: String,
    pub holder_slug: String,
    pub holder_name: String,
}

const DEPARTMENT_TAKEN_TAIL: &str = "so two agents cannot share it";

impl std::fmt::Display for DepartmentTaken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the department \"{}\" already belongs to {} ({}) — pick another one: a department names one agent's container and memory, {DEPARTMENT_TAKEN_TAIL}",
            self.department, self.holder_name, self.holder_slug
        )
    }
}

/// Whether an error string is [`DepartmentTaken`]'s sentence.
pub fn is_department_taken(msg: &str) -> bool {
    msg.contains(DEPARTMENT_TAKEN_TAIL)
}

/// Why [`upsert_agent_def`] wrote nothing.
#[derive(Debug)]
pub enum UpsertDefError {
    DepartmentTaken(DepartmentTaken),
    Db(sqlx::Error),
}

impl std::fmt::Display for UpsertDefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpsertDefError::DepartmentTaken(t) => t.fmt(f),
            UpsertDefError::Db(e) => e.fmt(f),
        }
    }
}

impl From<sqlx::Error> for UpsertDefError {
    fn from(e: sqlx::Error) -> Self {
        UpsertDefError::Db(e)
    }
}

/// The agent other than `slug` that holds `department`, if any — retired or
/// not, managed or not: the oldest, when (before the rule) there are several.
/// The writers' pre-check and the check under the lock both ask this.
pub async fn department_holder<'e, E>(
    exec: E,
    department: &str,
    slug: &str,
) -> Result<Option<DepartmentTaken>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<(String, String)> = sqlx::query_as(
        "select slug, display_name from agent_defs \
         where department = $1 and slug <> $2 \
         order by created_at, id limit 1",
    )
    .bind(department)
    .bind(slug)
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|(holder_slug, holder_name)| DepartmentTaken {
        department: department.to_string(),
        holder_slug,
        holder_name,
    }))
}

/// One agent in a department another agent also holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepartmentMember {
    pub id: String,
    pub slug: String,
    pub display_name: String,
    /// Whether the render runs it at all: managed, enabled, with a current
    /// version — the same three conditions as the render's own roster.
    #[serde(skip)]
    pub renders: bool,
}

/// A department two or more agents hold — only ever one that predates the
/// rule above, or one a direct SQL write made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedDepartment {
    pub department: String,
    /// Oldest first: the order that decides which one runs.
    pub members: Vec<DepartmentMember>,
}

impl SharedDepartment {
    /// The agent the render runs in the department's container: the oldest
    /// one it renders (talaria-fleet-render's `managed_agents` ranks them the
    /// same way). None when it renders none of them.
    pub fn runner(&self) -> Option<&DepartmentMember> {
        self.members.iter().find(|m| m.renders)
    }

    /// The agent running in the department's container when that agent is
    /// not `id` — the one an act on `id`'s container would really reach.
    pub fn runs_instead_of(&self, id: &str) -> Option<&DepartmentMember> {
        self.runner().filter(|r| r.id != id)
    }
}

/// Rows in (department, oldest first) order, grouped; a department with one
/// member is not shared and is dropped.
fn group_shared(rows: Vec<(String, DepartmentMember)>) -> Vec<SharedDepartment> {
    let mut out: Vec<SharedDepartment> = Vec::new();
    for (department, member) in rows {
        match out.last_mut() {
            Some(last) if last.department == department => last.members.push(member),
            _ => out.push(SharedDepartment {
                department,
                members: vec![member],
            }),
        }
    }
    out.retain(|s| s.members.len() > 1);
    out
}

type MemberTuple = (String, String, String, String, bool);

fn member_row(
    (department, id, slug, display_name, renders): MemberTuple,
) -> (String, DepartmentMember) {
    (
        department,
        DepartmentMember {
            id,
            slug,
            display_name,
            renders,
        },
    )
}

/// Every department more than one agent holds, for the people who can untie
/// it: the roster read, the agent page and the alerts.
pub async fn shared_departments(pg: &PgPool) -> Result<Vec<SharedDepartment>, sqlx::Error> {
    let rows: Vec<MemberTuple> = sqlx::query_as(
        "select d.department, d.id::text, d.slug, d.display_name, \
           (d.managed and d.enabled and exists (select 1 from agent_versions v \
              where v.agent_id = d.id and v.version = d.current_version)) as renders \
         from agent_defs d \
         where d.department in \
           (select department from agent_defs group by department having count(*) > 1) \
         order by d.department, d.created_at, d.id",
    )
    .fetch_all(pg)
    .await?;
    Ok(group_shared(rows.into_iter().map(member_row).collect()))
}

/// [`shared_departments`] for one department: None when one agent (or none)
/// holds it.
pub async fn department_share(
    pg: &PgPool,
    department: &str,
) -> Result<Option<SharedDepartment>, sqlx::Error> {
    let rows: Vec<MemberTuple> = sqlx::query_as(
        "select d.department, d.id::text, d.slug, d.display_name, \
           (d.managed and d.enabled and exists (select 1 from agent_versions v \
              where v.agent_id = d.id and v.version = d.current_version)) as renders \
         from agent_defs d where d.department = $1 \
         order by d.created_at, d.id",
    )
    .bind(department)
    .fetch_all(pg)
    .await?;
    Ok(group_shared(rows.into_iter().map(member_row).collect())
        .into_iter()
        .next())
}

/// The roster's `departmentShare` for one def: null for a department it holds
/// alone; otherwise who runs in the department's container and who else holds
/// it.
fn department_share_wire(shared: &[SharedDepartment], id: &str, department: &str) -> Value {
    let Some(share) = shared.iter().find(|s| s.department == department) else {
        return Value::Null;
    };
    let runner = share.runner();
    serde_json::json!({
        "runs": runner.is_some_and(|r| r.id == id),
        "runner": runner,
        "others": share.members.iter().filter(|m| m.id != id).collect::<Vec<_>>(),
    })
}

/// Insert-or-refresh a def by slug (upsertAgentDef). The create flow reaches
/// this only after its exists pre-check, but the upsert — not a bare insert —
/// is the contract: a slug already present keeps its role unless a new one is
/// supplied (imports don't carry it).
///
/// Every writer of `agent_defs.department` comes through here, and here a
/// department another agent holds is refused ([`DepartmentTaken`]). The check
/// and the write share one transaction under a per-department advisory lock,
/// so two hires racing into one department cannot both pass it: the second
/// waits for the first to commit and then sees its row.
pub async fn upsert_agent_def(
    pg: &PgPool,
    input: &UpsertDef<'_>,
) -> Result<AgentDefRow, UpsertDefError> {
    let model = format!("{}-{}", input.slug, input.department);
    let mut tx = pg.begin().await?;
    sqlx::query("select pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("agent-department:{}", input.department))
        .execute(&mut *tx)
        .await?;
    if let Some(taken) = department_holder(&mut *tx, input.department, input.slug).await? {
        return Err(UpsertDefError::DepartmentTaken(taken));
    }
    // A NEW row starts closed ([`default_disabled_for_new`]); an existing
    // slug keeps whatever its managers opened — the default is for the
    // insert only, never a reset on re-import. Nothing here knows whether
    // the agent will be somebody's assistant, so the shared-agent names ride
    // along and the personal create path opens them once it marks the owner.
    let row: DefTuple = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name, role, source, disabled_toolsets) \
         values ($1, $2, $3, $4, $5, $6, $7) \
         on conflict (slug) do update set \
           department = excluded.department, model = excluded.model, \
           display_name = excluded.display_name, \
           role = coalesce(excluded.role, agent_defs.role), updated_at = now() \
         returning id::text, slug, department, model, display_name, role, source, managed, current_version",
    )
    .bind(input.slug)
    .bind(input.department)
    .bind(&model)
    .bind(input.display_name)
    .bind(input.role)
    .bind(input.source)
    .bind(serde_json::json!(default_disabled_for_new(false)))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(def_row(row))
}

pub struct UpsertDef<'a> {
    pub slug: &'a str,
    pub department: &'a str,
    pub display_name: &'a str,
    pub role: Option<&'a str>,
    pub source: &'a str,
}

/// Key-order-insensitive stringify — Postgres jsonb reorders object keys, so
/// a plain `to_string` comparison would see every round-trip as a change.
/// ASCII keys sort identically under a UTF-16
/// code-unit compare and Rust's byte compare; config keys are ASCII.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical(&map[k.as_str()])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        leaf => serde_json::to_string(leaf).unwrap_or_else(|_| "null".into()),
    }
}

/// One knob onto one key of a raw block: left alone, removed, or written.
fn turn_knob<T: Into<Value>>(block: &mut Map<String, Value>, key: &str, knob: Knob<T>) {
    match knob {
        Knob::Keep => {}
        Knob::Clear => {
            block.remove(key);
        }
        Knob::Set(v) => {
            block.insert(key.to_string(), v.into());
        }
    }
}

/// Structured edits applied onto a previous version's config
/// (applyConfigEdits). The raw Hermes config is rebuilt in the same stroke so
/// rendering reflects the edit: model/base_url/api_key come from the endpoint
/// registry; extra raw keys (e.g. context_length env refs) survive when the
/// endpoint is unchanged.
pub async fn apply_config_edits(
    pg: &PgPool,
    prev: &Value,
    edits: &ConfigEdits,
) -> Result<Value, String> {
    let endpoints = list_endpoints(pg).await.map_err(|e| e.to_string())?;
    apply_config_edits_over(&endpoints, prev, edits)
}

/// The pure core of apply_config_edits, over an already-loaded endpoint
/// registry — separated so the block-building rules are testable without a
/// database.
pub fn apply_config_edits_over(
    endpoints: &[LlmEndpoint],
    prev: &Value,
    edits: &ConfigEdits,
) -> Result<Value, String> {
    /// One raw model block: endpoint facts under the target's model, keeping
    /// the previous block's keys only when it pointed at the same place.
    fn block(ep: &LlmEndpoint, target: &ModelTarget, prev_block: Option<&Value>) -> Value {
        let prev_b = prev_block.and_then(Value::as_object);
        // `(prevB.provider === ep.provider) && (prevB.base_url ?? null) === (ep.baseUrl ?? null)`
        let same_place = prev_b
            .map(|b| {
                b.get("provider").and_then(Value::as_str) == Some(ep.provider.as_str())
                    && b.get("base_url").and_then(Value::as_str) == ep.base_url.as_deref()
            })
            .unwrap_or(false);
        let mut out: Map<String, Value> = if same_place {
            prev_b.cloned().unwrap_or_default()
        } else {
            Map::new()
        };
        out.insert("provider".into(), Value::String(ep.provider.clone()));
        out.insert("model".into(), Value::String(target.model.clone()));
        // truthiness on baseUrl: null and '' both skip the key.
        if let Some(base) = ep.base_url.as_deref().filter(|b| !b.is_empty()) {
            out.insert("base_url".into(), Value::String(base.into()));
        }
        if !same_place && let Some(env) = ep.api_key_env.as_deref().filter(|e| !e.is_empty()) {
            out.insert("api_key".into(), Value::String(format!("${{{env}}}")));
        }
        Value::Object(out)
    }

    let ep_of = |name: &str| -> Result<&LlmEndpoint, String> {
        endpoints
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| format!("unknown endpoint \"{name}\""))
    };

    let prev_raw = prev
        .get("raw")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let prev_aliases = prev_raw
        .get("model_aliases")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut raw = prev_raw;
    raw.insert(
        "model".into(),
        block(
            ep_of(&edits.main.endpoint)?,
            &edits.main,
            prev.get("raw").and_then(|r| r.get("model")),
        ),
    );
    let mut aliases = Map::new();
    for alias in &edits.aliases {
        aliases.insert(
            alias.name.clone(),
            block(
                ep_of(&alias.target.endpoint)?,
                &alias.target,
                prev_aliases.get(&alias.name),
            ),
        );
    }
    raw.insert("model_aliases".into(), Value::Object(aliases));
    let mut fallbacks: Vec<Value> = Vec::new();
    for f in &edits.fallbacks {
        fallbacks.push(block(ep_of(&f.endpoint)?, f, None));
    }
    raw.insert("fallback_providers".into(), Value::Array(fallbacks));

    // raw.compression — the two knobs, turned inside the block the def author
    // wrote. Clearing the last one takes the block with it: an empty
    // `compression: {}` in the rendered config says nothing Hermes' defaults
    // do not already say.
    let mut compression = raw
        .get("compression")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    turn_knob(&mut compression, "threshold", edits.compression.threshold);
    turn_knob(
        &mut compression,
        "threshold_tokens",
        edits.compression.threshold_tokens,
    );
    if compression.is_empty() {
        raw.remove("compression");
    } else {
        raw.insert("compression".into(), Value::Object(compression));
    }

    // `{...prev, main, aliases, fallbacks, raw}` — the structured fields
    // replace, every other key the previous config carried survives.
    let mut out = prev.clone();
    if !out.is_object() {
        out = Value::Object(Map::new());
    }
    let obj = out.as_object_mut().unwrap();
    obj.insert("main".into(), serde_json::to_value(&edits.main).unwrap());
    obj.insert(
        "aliases".into(),
        serde_json::to_value(&edits.aliases).unwrap(),
    );
    obj.insert(
        "fallbacks".into(),
        serde_json::to_value(&edits.fallbacks).unwrap(),
    );
    obj.insert("raw".into(), Value::Object(raw));
    Ok(out)
}

pub struct NewVersion<'a> {
    pub soul: &'a str,
    pub config: &'a Value,
    pub note: Option<&'a str>,
    pub created_by: Option<&'a str>,
}

/// Append a new version (and point current_version at it). Skips when the
/// payload equals the latest version — safe to re-run imports, and the hire
/// run's resume depends on it (addVersionIfChanged).
pub async fn add_version_if_changed(
    pg: &PgPool,
    agent_id: &str,
    payload: &NewVersion<'_>,
) -> Result<(i32, bool), sqlx::Error> {
    let latest: Option<(i32, String, Value)> = sqlx::query_as(
        "select version, soul, config from agent_versions \
         where agent_id = $1::uuid order by version desc limit 1",
    )
    .bind(agent_id)
    .fetch_optional(pg)
    .await?;
    if let Some((version, soul, config)) = &latest
        && soul == payload.soul
        && canonical(config) == canonical(payload.config)
    {
        return Ok((*version, false));
    }
    let next = latest.as_ref().map(|(v, _, _)| *v + 1).unwrap_or(1);
    let mut tx = pg.begin().await?;
    sqlx::query(
        "insert into agent_versions (agent_id, version, soul, config, note, created_by) \
         values ($1::uuid, $2, $3, $4, $5, $6::text)",
    )
    .bind(agent_id)
    .bind(next)
    .bind(payload.soul)
    .bind(payload.config)
    .bind(payload.note)
    .bind(payload.created_by)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "update agent_defs set current_version = $1, updated_at = now() where id = $2::uuid",
    )
    .bind(next)
    .bind(agent_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((next, true))
}

/// Update editable identity metadata (role, display name, email alias). Not
/// versioned — this is the agent's identity, not its config payload.
/// `None` means "leave the column alone"; `Some(None)` clears it.
pub struct AgentMetaPatch<'a> {
    pub role: Option<Option<&'a str>>,
    pub display_name: Option<&'a str>,
    pub email_alias: Option<Option<&'a str>>,
}

pub async fn update_agent_meta(
    pg: &PgPool,
    id: &str,
    patch: &AgentMetaPatch<'_>,
) -> Result<(), sqlx::Error> {
    if let Some(role) = patch.role {
        sqlx::query("update agent_defs set role = $1, updated_at = now() where id = $2::uuid")
            .bind(role)
            .bind(id)
            .execute(pg)
            .await?;
    }
    if let Some(display_name) = patch.display_name {
        sqlx::query(
            "update agent_defs set display_name = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(display_name)
        .bind(id)
        .execute(pg)
        .await?;
    }
    if let Some(email_alias) = patch.email_alias {
        sqlx::query(
            "update agent_defs set email_alias = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(email_alias)
        .bind(id)
        .execute(pg)
        .await?;
    }
    Ok(())
}

/// Set the delegation cap (fleet.defs.$id PATCH) — how many subagents this
/// agent may run at once. Returns whether the stored value changed, so the
/// caller rolls the agent only when there is something to apply: the number
/// reaches Hermes through the rendered config, and a render it already has is
/// not worth a restart.
pub async fn set_max_children(pg: &PgPool, id: &str, n: i32) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(
        "update agent_defs set max_children = $1, updated_at = now() \
         where id = $2::uuid and max_children is distinct from $1",
    )
    .bind(n)
    .bind(id)
    .execute(pg)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// Re-hire a retired agent: re-enable so it renders + can start again.
pub async fn set_agent_enabled(pg: &PgPool, id: &str, enabled: bool) -> Result<(), sqlx::Error> {
    sqlx::query("update agent_defs set enabled = $1, updated_at = now() where id = $2::uuid")
        .bind(enabled)
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // No database in here: the alphabets, the canonical form, and the
    // block-building rules are pure; the row plumbing is thin sqlx,
    // exercised later by the run's own tests.

    #[test]
    fn the_alphabets_admit_slugs_and_refuse_everything_else() {
        assert!(slug_ok("analyst"));
        assert!(slug_ok("a1"));
        assert!(!slug_ok("Analyst"));
        assert!(!slug_ok("a"));
        assert!(!slug_ok("a-b"));
        assert!(!slug_ok("-a"));
        assert!(!slug_ok("a\nB=1"));
        assert!(
            slug_ok(&"a".repeat(31)),
            "1 + 30 is the ceiling, and it is in"
        );
        assert!(!slug_ok(&"a".repeat(32)));
        assert!(dept_ok("research"));
        assert!(dept_ok("customer-success-2"));
        assert!(!dept_ok("Research"));
        assert!(!dept_ok("research_"));
        assert!(!dept_ok(&"d".repeat(42)));
    }

    #[test]
    fn validate_toolsets_trims_dedupes_and_names_the_stranger() {
        let got = validate_toolsets(&[
            "  browser ".into(),
            "memory".into(),
            "browser".into(),
            "memory".into(),
        ])
        .unwrap();
        assert_eq!(
            got,
            vec!["browser".to_string(), "memory".to_string()],
            "trimmed, first occurrence kept, order as asked"
        );
        assert_eq!(validate_toolsets(&[]).unwrap(), Vec::<String>::new());
        // The message has to carry the name: eight checkboxes and one typo is
        // otherwise a 400 the operator cannot act on.
        assert_eq!(
            validate_toolsets(&["browser".into(), "brower".into(), "nope".into()]).unwrap_err(),
            "unknown toolset \"brower\"",
            "the FIRST stranger is the one named"
        );
        // Hermes' platform toolsets are real names Talaria still refuses:
        // nothing here is wired to Discord, so disabling it would be theatre.
        assert_eq!(
            validate_toolsets(&["discord".into()]).unwrap_err(),
            "unknown toolset \"discord\""
        );
        // The cap is on the WALK: 41 repeats of a blessed name would dedupe to
        // one, and still never get that far.
        let many: Vec<String> = std::iter::repeat_n("todo".to_string(), MAX_TOOLSETS + 1).collect();
        assert_eq!(validate_toolsets(&many).unwrap_err(), "at most 40 toolsets");
        let at_cap: Vec<String> = std::iter::repeat_n("todo".to_string(), MAX_TOOLSETS).collect();
        assert_eq!(
            validate_toolsets(&at_cap).unwrap(),
            vec!["todo".to_string()],
            "the cap itself is in"
        );
        // The governed list is the floor's home too — the renderer merges
        // MANDATORY_DISABLED, so both names must be nameable here as well.
        for name in MANDATORY_DISABLED {
            assert!(GOVERNED_TOOLSETS.contains(name), "{name} is not governed");
        }
        // And so must every name the closed default, the Developer switch and
        // the shared-agent default write: a stranger here would be refused
        // by the PATCH the moment a manager re-saved the list.
        for name in DEFAULT_DISABLED_FOR_NEW
            .iter()
            .chain(DEVELOPER_TOOLSETS)
            .chain(SHARED_AGENT_DISABLED)
        {
            assert!(GOVERNED_TOOLSETS.contains(name), "{name} is not governed");
        }
    }

    #[test]
    fn a_new_agent_starts_closed_and_a_personal_one_keeps_its_recall() {
        let shared = default_disabled_for_new(false);
        for name in [
            "terminal",
            "code_execution",
            "file",
            "cronjob",
            "session_search",
        ] {
            assert!(
                shared.iter().any(|n| n == name),
                "{name} should start closed"
            );
        }
        for name in ["browser", "delegation", "memory"] {
            assert!(
                !shared.iter().any(|n| n == name),
                "{name} should start open"
            );
        }
        let personal = default_disabled_for_new(true);
        assert!(
            !personal.iter().any(|n| n == "session_search"),
            "a personal assistant recalls its own person's threads"
        );
        assert!(personal.iter().any(|n| n == "terminal"));
        // The Developer switch opens exactly the sandbox trio and nothing the
        // closed default also carries beyond it.
        assert!(
            DEVELOPER_TOOLSETS
                .iter()
                .all(|n| DEFAULT_DISABLED_FOR_NEW.contains(n))
        );
        assert!(!DEVELOPER_TOOLSETS.contains(&"cronjob"));
    }

    #[test]
    fn the_org_floor_merges_governed_names_behind_the_mandatory_pair() {
        // Nothing stored, nothing in the env: the two-name floor.
        assert_eq!(
            merge_org_floor(&Value::Null, None),
            vec!["todo".to_string(), "clarify".to_string()]
        );
        // Stored and env names land after the pair, deduped against it and
        // each other; a stranger and a blank are dropped, not rendered.
        let stored = serde_json::json!(["terminal", "todo", "brower", " browser "]);
        assert_eq!(
            merge_org_floor(&stored, Some("web, terminal,, nope")),
            vec![
                "todo".to_string(),
                "clarify".to_string(),
                "terminal".to_string(),
                "browser".to_string(),
                "web".to_string()
            ]
        );
        // A stored value that is not an array is the same as none.
        assert_eq!(
            merge_org_floor(&serde_json::json!("terminal"), None),
            vec!["todo".to_string(), "clarify".to_string()]
        );
    }

    #[test]
    fn a_retired_name_expands_to_what_it_closed_and_never_reopens_anything() {
        // The composites close their members, `search` reads as the stricter
        // `web`, and the no-op names vanish — in a def's list and in the org
        // floor alike.
        assert_eq!(
            validate_toolsets(&["safe".into(), "web".into(), "coding".into()]).unwrap(),
            vec![
                "web".to_string(),
                "vision".to_string(),
                "image_gen".to_string()
            ]
        );
        assert_eq!(
            validate_toolsets(&["debugging".into(), "project".into(), "desktop_ui".into()])
                .unwrap(),
            vec![
                "terminal".to_string(),
                "web".to_string(),
                "file".to_string()
            ]
        );
        assert_eq!(
            merge_org_floor(&serde_json::json!(["search"]), Some("safe")),
            vec![
                "todo".to_string(),
                "clarify".to_string(),
                "web".to_string(),
                "vision".to_string(),
                "image_gen".to_string()
            ],
            "an org floor that closed a composite must not reopen its members"
        );
        // Every expansion lands on governed names only, and no retired name
        // is still governed — the two tables never overlap.
        for (old, members) in RETIRED_TOOLSETS {
            assert!(
                !GOVERNED_TOOLSETS.contains(old),
                "{old} is retired AND governed"
            );
            for m in *members {
                assert!(
                    GOVERNED_TOOLSETS.contains(m),
                    "{old} expands to ungoverned {m}"
                );
            }
        }
    }

    /// The snapshot is Hermes' own `CONFIGURABLE_TOOLSETS` at the pinned
    /// version (the chassis smoke re-reads it from the live image and fails on
    /// drift). The governed list is that list minus the names the snapshot
    /// says Talaria leaves alone, each with its reason — so a Hermes switch
    /// that appears in the snapshot without a decision here fails this test.
    #[test]
    fn the_governed_list_is_hermes_own_switches_minus_the_unwired() {
        #[derive(Deserialize)]
        struct Snapshot {
            configurable: Vec<String>,
            #[serde(rename = "notGoverned")]
            not_governed: std::collections::BTreeMap<String, String>,
        }
        let snap: Snapshot = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../scripts/hermes-toolsets.json"
        )))
        .expect("scripts/hermes-toolsets.json parses");
        for name in snap.not_governed.keys() {
            assert!(
                snap.configurable.contains(name),
                "notGoverned names {name}, which Hermes does not offer"
            );
        }
        let mut want: Vec<&str> = snap
            .configurable
            .iter()
            .map(String::as_str)
            .filter(|n| !snap.not_governed.contains_key(*n))
            .collect();
        want.sort_unstable();
        let mut got = GOVERNED_TOOLSETS.to_vec();
        got.sort_unstable();
        assert_eq!(
            got, want,
            "GOVERNED_TOOLSETS drifted from the Hermes snapshot"
        );
        assert_eq!(
            GOVERNED_TOOLSETS.to_vec(),
            got,
            "kept alphabetical, the editor's reading order"
        );
    }

    #[test]
    fn canonical_sorts_keys_and_ignores_array_order() {
        let a: Value = serde_json::from_str(r#"{"b":1,"a":{"y":[1,2],"x":null}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":{"x":null,"y":[1,2]},"b":1}"#).unwrap();
        assert_eq!(canonical(&a), canonical(&b));
        let c: Value = serde_json::from_str(r#"{"a":{"y":[2,1],"x":null},"b":1}"#).unwrap();
        assert_ne!(canonical(&a), canonical(&c), "array order IS significant");
        // The exact form, pinned: sorted keys, no spaces — jsonb's reorder is
        // invisible, a value change never is.
        assert_eq!(canonical(&a), r#"{"a":{"x":null,"y":[1,2]},"b":1}"#);
    }

    fn ep(
        name: &str,
        provider: &str,
        base_url: Option<&str>,
        api_key_env: Option<&str>,
    ) -> LlmEndpoint {
        LlmEndpoint {
            id: format!("id-{name}"),
            name: name.into(),
            provider: provider.into(),
            base_url: base_url.map(String::from),
            class: "local".into(),
            api_key_env: api_key_env.map(String::from),
            models: vec![],
            price_in_per_mtok: None,
            price_out_per_mtok: None,
            model_prices: Value::Null,
            auto_prices: Value::Null,
            request_defaults: Value::Null,
            model_efforts: Value::Null,
            context_length: None,
        }
    }

    fn target(endpoint: &str, model: &str) -> ModelTarget {
        ModelTarget {
            endpoint: endpoint.into(),
            model: model.into(),
            context_length: None,
            effort: None,
        }
    }

    #[test]
    fn a_fresh_default_builds_the_raw_block_from_the_registry() {
        let endpoints = vec![
            ep(
                "ollama",
                "ollama",
                Some("http://127.0.0.1:11434"),
                Some("OLLAMA_API_KEY"),
            ),
            ep("openai", "openai", None, Some("LLM_API_KEY")),
        ];
        // The hire's platform-defaults call: prev is the same bare shape.
        let prev: Value = serde_json::from_str(
            r#"{"main":{"endpoint":"ollama","model":"m"},"aliases":[],"fallbacks":[]}"#,
        )
        .unwrap();
        let edits = ConfigEdits {
            main: target("ollama", "m"),
            aliases: vec![],
            fallbacks: vec![],
            compression: CompressionEdits::default(),
        };
        let out = apply_config_edits_over(&endpoints, &prev, &edits).unwrap();
        let raw = out.get("raw").unwrap();
        // no previous raw → the block is registry facts only, and the
        // api_key placeholder interpolates the endpoint's env name
        assert_eq!(
            raw.get("model"),
            Some(&serde_json::json!({"provider": "ollama", "model": "m",
                                     "base_url": "http://127.0.0.1:11434",
                                     "api_key": "${OLLAMA_API_KEY}"}))
        );
        assert_eq!(raw.get("model_aliases"), Some(&serde_json::json!({})));
        assert_eq!(raw.get("fallback_providers"), Some(&serde_json::json!([])));
        // the structured fields ride along camelCase
        assert_eq!(
            out.get("main"),
            Some(&serde_json::json!({"endpoint": "ollama", "model": "m"}))
        );
    }

    #[test]
    fn a_same_place_edit_keeps_the_previous_raw_keys() {
        let endpoints = vec![ep("ollama", "ollama", Some("http://127.0.0.1:11434"), None)];
        let prev: Value = serde_json::from_str(
            r#"{"main":{"endpoint":"ollama","model":"old"},
                "aliases":[{"name":"deep","endpoint":"ollama","model":"old"}],
                "fallbacks":[],
                "raw":{"model":{"provider":"ollama","base_url":"http://127.0.0.1:11434",
                                  "model":"old","context_length_env":"CTX"},
                        "model_aliases":{"deep":{"provider":"ollama","model":"old"}}}}"#,
        )
        .unwrap();
        let edits = ConfigEdits {
            main: target("ollama", "new"),
            aliases: vec![AliasTarget {
                name: "deep".into(),
                target: target("ollama", "new-deep"),
            }],
            fallbacks: vec![],
            compression: CompressionEdits::default(),
        };
        let out = apply_config_edits_over(&endpoints, &prev, &edits).unwrap();
        let model = out["raw"]["model"].as_object().unwrap();
        // same provider+base_url → the extra key survives, the model swaps
        assert_eq!(
            model.get("context_length_env"),
            Some(&serde_json::json!("CTX"))
        );
        assert_eq!(model.get("model"), Some(&serde_json::json!("new")));
        assert!(
            !model.contains_key("api_key"),
            "no placeholder added on a same-place edit"
        );
        assert_eq!(
            out["raw"]["model_aliases"]["deep"]["model"],
            serde_json::json!("new-deep")
        );
    }

    /// The config the compression tests edit: one endpoint, a hand-authored
    /// compression block with keys no form offers, and a sibling raw key.
    fn prev_with_compression() -> Value {
        serde_json::from_str(
            r#"{"main":{"endpoint":"ollama","model":"m"},"aliases":[],"fallbacks":[],
                "raw":{"model":{"provider":"ollama","model":"m"},
                       "agent":{"name":"scout"},
                       "compression":{"enabled":true,"threshold":0.5,
                                      "threshold_tokens":256000,"protect_last_n":20}}}"#,
        )
        .unwrap()
    }

    fn edits_with(compression: CompressionEdits) -> ConfigEdits {
        ConfigEdits {
            main: target("ollama", "m"),
            aliases: vec![],
            fallbacks: vec![],
            compression,
        }
    }

    /// Setting the knobs writes exactly the two keys and leaves the rest of
    /// the author's block — and the rest of raw — alone.
    #[test]
    fn the_compression_knobs_write_into_the_authors_block() {
        let endpoints = vec![ep("ollama", "ollama", None, None)];
        let out = apply_config_edits_over(
            &endpoints,
            &prev_with_compression(),
            &edits_with(CompressionEdits {
                threshold: Knob::Set(0.8),
                threshold_tokens: Knob::Set(400_000),
            }),
        )
        .unwrap();
        assert_eq!(
            out["raw"]["compression"],
            serde_json::json!({"enabled": true, "threshold": 0.8,
                               "threshold_tokens": 400_000, "protect_last_n": 20})
        );
        // the sibling raw key the renderer owns is untouched
        assert_eq!(out["raw"]["agent"], serde_json::json!({"name": "scout"}));
    }

    /// `Clear` is the null in the request: the key goes, and only that key.
    #[test]
    fn clearing_a_knob_drops_its_key_and_nothing_else() {
        let endpoints = vec![ep("ollama", "ollama", None, None)];
        let out = apply_config_edits_over(
            &endpoints,
            &prev_with_compression(),
            &edits_with(CompressionEdits {
                threshold: Knob::Keep,
                threshold_tokens: Knob::Clear,
            }),
        )
        .unwrap();
        let block = out["raw"]["compression"].as_object().unwrap();
        assert!(!block.contains_key("threshold_tokens"), "{block:?}");
        assert_eq!(block.get("threshold"), Some(&serde_json::json!(0.5)));
        assert_eq!(block.get("enabled"), Some(&serde_json::json!(true)));
    }

    /// Saying nothing about compression changes nothing about compression —
    /// the default every other caller of this function passes.
    #[test]
    fn an_edit_that_says_nothing_leaves_the_block_as_written() {
        let endpoints = vec![ep("ollama", "ollama", None, None)];
        let prev = prev_with_compression();
        let out =
            apply_config_edits_over(&endpoints, &prev, &edits_with(CompressionEdits::default()))
                .unwrap();
        assert_eq!(out["raw"]["compression"], prev["raw"]["compression"]);
    }

    /// Nothing authored and nothing asked for leaves no block at all: an
    /// empty `compression: {}` would be a key in the rendered config that
    /// says nothing Hermes' own defaults do not already say.
    #[test]
    fn clearing_the_last_knob_takes_the_block_with_it() {
        let endpoints = vec![ep("ollama", "ollama", None, None)];
        let prev: Value = serde_json::from_str(
            r#"{"main":{"endpoint":"ollama","model":"m"},"aliases":[],"fallbacks":[],
                "raw":{"compression":{"threshold":0.5}}}"#,
        )
        .unwrap();
        let out = apply_config_edits_over(
            &endpoints,
            &prev,
            &edits_with(CompressionEdits {
                threshold: Knob::Clear,
                threshold_tokens: Knob::Clear,
            }),
        )
        .unwrap();
        assert!(out["raw"].get("compression").is_none(), "{}", out["raw"]);
        // and a def that never had a block does not grow an empty one
        let bare: Value = serde_json::from_str(
            r#"{"main":{"endpoint":"ollama","model":"m"},"aliases":[],"fallbacks":[]}"#,
        )
        .unwrap();
        let out =
            apply_config_edits_over(&endpoints, &bare, &edits_with(CompressionEdits::default()))
                .unwrap();
        assert!(out["raw"].get("compression").is_none(), "{}", out["raw"]);
    }

    /// The read side the Config tab uses: the stored block, camelCased, with
    /// a non-number reading as absent rather than as itself.
    #[test]
    fn the_compression_wire_is_the_stored_block_or_null() {
        assert_eq!(
            compression_wire(&prev_with_compression()),
            serde_json::json!({"threshold": 0.5, "thresholdTokens": 256_000})
        );
        assert_eq!(compression_wire(&serde_json::json!({})), Value::Null);
        assert_eq!(
            compression_wire(&serde_json::json!({"raw": {"compression": {}}})),
            serde_json::json!({"threshold": null, "thresholdTokens": null})
        );
        assert_eq!(
            compression_wire(&serde_json::json!({"raw": {"compression": {"threshold": "half"}}})),
            serde_json::json!({"threshold": null, "thresholdTokens": null})
        );
    }

    #[test]
    fn an_unknown_endpoint_is_the_one_edit_error() {
        let out = apply_config_edits_over(
            &[],
            &Value::Null,
            &ConfigEdits {
                main: target("nowhere", "m"),
                aliases: vec![],
                fallbacks: vec![],
                compression: CompressionEdits::default(),
            },
        );
        assert_eq!(out.unwrap_err(), "unknown endpoint \"nowhere\"");
    }

    fn member(id: &str, renders: bool) -> DepartmentMember {
        DepartmentMember {
            id: id.into(),
            slug: id.into(),
            display_name: id.to_uppercase(),
            renders,
        }
    }

    #[test]
    fn a_held_department_is_refused_in_a_sentence_the_string_layers_can_still_recognise() {
        let taken = DepartmentTaken {
            department: "research".into(),
            holder_slug: "ava".into(),
            holder_name: "Ava".into(),
        };
        let msg = UpsertDefError::DepartmentTaken(taken.clone()).to_string();
        assert_eq!(msg, taken.to_string());
        assert!(msg.starts_with("the department \"research\" already belongs to Ava (ava)"));
        assert!(is_department_taken(&msg));
        // The hire's resume path adopts on "already exists": this sentence
        // must never read as that one.
        assert!(!msg.contains("already exists"));
        assert!(!is_department_taken("agent \"ava\" already exists"));
    }

    #[test]
    fn shared_departments_group_in_order_and_drop_a_department_held_once() {
        let rows = vec![
            ("ops".to_string(), member("solo", true)),
            ("research".to_string(), member("ava", true)),
            ("research".to_string(), member("zed", true)),
            ("support".to_string(), member("old", false)),
            ("support".to_string(), member("new", true)),
        ];
        let shared = group_shared(rows);
        assert_eq!(
            shared
                .iter()
                .map(|s| s.department.as_str())
                .collect::<Vec<_>>(),
            vec!["research", "support"],
            "a department one agent holds is not shared"
        );
        // The oldest that renders runs; the other is left out.
        assert_eq!(shared[0].runner().map(|m| m.id.as_str()), Some("ava"));
        assert_eq!(
            shared[0].runs_instead_of("zed").map(|m| m.id.as_str()),
            Some("ava")
        );
        assert_eq!(shared[0].runs_instead_of("ava"), None);
        // An older agent the render does not run (retired) does not hold the
        // container: the oldest one it DOES run does.
        assert_eq!(shared[1].runner().map(|m| m.id.as_str()), Some("new"));
        assert_eq!(
            shared[1].runs_instead_of("old").map(|m| m.id.as_str()),
            Some("new")
        );
        // Nobody runs: nobody else's container to protect.
        let idle = SharedDepartment {
            department: "x".into(),
            members: vec![member("a", false), member("b", false)],
        };
        assert_eq!(idle.runner(), None);
        assert_eq!(idle.runs_instead_of("a"), None);
    }

    #[test]
    fn the_roster_names_the_runner_and_the_others_for_a_shared_department_only() {
        let shared = group_shared(vec![
            ("research".to_string(), member("ava", true)),
            ("research".to_string(), member("zed", true)),
        ]);
        assert_eq!(department_share_wire(&shared, "solo", "ops"), Value::Null);
        assert_eq!(
            department_share_wire(&shared, "zed", "research"),
            serde_json::json!({
                "runs": false,
                "runner": {"id": "ava", "slug": "ava", "displayName": "AVA"},
                "others": [{"id": "ava", "slug": "ava", "displayName": "AVA"}],
            })
        );
        assert_eq!(
            department_share_wire(&shared, "ava", "research"),
            serde_json::json!({
                "runs": true,
                "runner": {"id": "ava", "slug": "ava", "displayName": "AVA"},
                "others": [{"id": "zed", "slug": "zed", "displayName": "ZED"}],
            })
        );
    }

    /// The one-agent rule against a migrated database: a second agent is
    /// refused the department, the same slug re-upserting its own department
    /// is not, and two writers racing into one fresh department land one row
    /// between them. `#[ignore]`d — it needs a database; CI's api-integration
    /// job runs it:
    ///
    ///   DATABASE_URL=postgres://… cargo test -p talaria-agent-defs --lib -- --ignored live_
    #[tokio::test]
    #[ignore = "live DB: set DATABASE_URL"]
    async fn live_a_department_belongs_to_one_agent_even_under_a_race() {
        let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the live tests");
        let pg = PgPool::connect(&url).await.expect("connect");
        let tag: String = uuid::Uuid::new_v4().simple().to_string()[..10].to_string();
        let dept = format!("deptlive-{tag}");
        let upsert = |slug: String, dept: String| {
            let pg = pg.clone();
            async move {
                upsert_agent_def(
                    &pg,
                    &UpsertDef {
                        slug: &slug,
                        department: &dept,
                        display_name: &slug,
                        role: None,
                        source: "created",
                    },
                )
                .await
            }
        };
        let first = format!("dla{tag}");
        let second = format!("dlb{tag}");
        let run = async {
            upsert(first.clone(), dept.clone())
                .await
                .expect("the first agent lands");
            match upsert(second.clone(), dept.clone()).await {
                Err(UpsertDefError::DepartmentTaken(t)) => {
                    assert_eq!(t.holder_slug, first);
                    assert_eq!(t.department, dept);
                }
                other => panic!("a second agent in the department must be refused: {other:?}"),
            }
            // The holder re-upserting itself is not a conflict.
            upsert(first.clone(), dept.clone())
                .await
                .expect("an agent keeps its own department");

            // The race: eight writers, eight slugs, one fresh department.
            let raced = format!("deptrace-{tag}");
            let attempts = (0..8).map(|i| upsert(format!("dlr{i}{tag}"), raced.clone()));
            let results = futures_util::future::join_all(attempts).await;
            let landed = results.iter().filter(|r| r.is_ok()).count();
            let refused = results
                .iter()
                .filter(|r| matches!(r, Err(UpsertDefError::DepartmentTaken(_))))
                .count();
            assert_eq!((landed, refused), (1, 7), "{results:?}");
            let rows: i64 =
                sqlx::query_scalar("select count(*) from agent_defs where department = $1")
                    .bind(&raced)
                    .fetch_one(&pg)
                    .await
                    .unwrap();
            assert_eq!(rows, 1);
        };
        use futures_util::FutureExt as _;
        let outcome = std::panic::AssertUnwindSafe(run).catch_unwind().await;
        sqlx::query("delete from agent_defs where department in ($1, $2)")
            .bind(&dept)
            .bind(format!("deptrace-{tag}"))
            .execute(&pg)
            .await
            .unwrap();
        if let Err(p) = outcome {
            std::panic::resume_unwind(p);
        }
    }
}
