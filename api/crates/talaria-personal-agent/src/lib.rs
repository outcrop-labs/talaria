// Personal assistants: each person can spin up their own Hermes agent from the
// dashboard — a real fleet agent (its own container, key, memory) owned by them,
// created from a base template and auto-allowed for its owner. The owner names
// it, picks its handle, and writes its personality — no admin role needed; every
// mutation here is scoped to `agent_defs.owner_user_id = user.id`.
//
// `personality_of` and `owns_agent` are borrowed by /api/history; the rest is
// the engine — the read projection, the create flow, and the owner-scoped
// update.

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;

use talaria_agent_defs::{
    AgentDefRow, CompressionEdits, ConfigEdits, ModelTarget, add_version_if_changed,
    apply_config_edits, list_versions,
};
use talaria_error::store_fault;
use talaria_fleet_create::{CreateAgentInput, create_agent, ensure_agent_key, restamp_slug};

/// Whether a create refusal is the one-agent-per-department rule: the
/// handle's department (`personal-<handle>`) is already another agent's
/// (talaria-agent-defs, "A department is one agent's key").
pub use talaria_agent_defs::is_department_taken;
use talaria_fleet_docker::{
    ANSWER_WITHIN, Detached, container_status, fleet_up, run_detached, wait_healthy,
};
use talaria_fleet_reconcile::apply_after_render;
use talaria_retrieval_collections::ensure_personal_collection;
use talaria_retrieval_embed as embed;
use talaria_retrieval_qdrant as qdrant;
use talaria_state::AppState;

use futures_util::future::BoxFuture;
use std::sync::{Arc, OnceLock};

/// Why a retired assistant cannot come back: another agent shares its
/// department (a pair from before a department became one agent's) and runs
/// in its container. Said without naming that agent — any signed-in user
/// reaches this door — and pointing at who can untie it.
pub const ASSISTANT_DISPLACED: &str = "your assistant cannot come back: another agent runs in its container now — ask an admin to untie the two";

pub static SYNC_PRIVATE_DOCS: OnceLock<
    Arc<dyn Fn(sqlx::PgPool, String) -> BoxFuture<'static, ()> + Send + Sync>,
> = OnceLock::new();

// The owner edits one marked section of the soul; the rest of the soul (role
// scaffold, guardrails) stays out of their way. Markers are HTML comments so
// they're invisible wherever the soul renders as markdown.
const PERSONA_START: &str = "<!-- talaria:personality -->";
const PERSONA_END: &str = "<!-- /talaria:personality -->";

/// The marked personality section of a soul, trimmed — null when the markers
/// are absent, out of order, or bracket only whitespace. Version history
/// serves this for `kind=personality`, so the null/empty distinction is the
/// wire contract.
pub fn personality_of(soul: &str) -> Option<String> {
    let m = soul.find(PERSONA_START)?;
    let e = soul.find(PERSONA_END)?;
    if e < m {
        return None;
    }
    let inner = soul[m + PERSONA_START.len()..e].trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

/// Replace (or append) the marked personality section of a soul.
pub fn with_personality(soul: &str, personality: &str) -> String {
    let block = format!("{PERSONA_START}\n{}\n{PERSONA_END}", personality.trim());
    if let (Some(m), Some(e)) = (soul.find(PERSONA_START), soul.find(PERSONA_END))
        && e > m
    {
        let mut out = String::with_capacity(soul.len());
        out.push_str(&soul[..m]);
        out.push_str(&block);
        out.push_str(&soul[e + PERSONA_END.len()..]);
        return out;
    }
    format!("{}\n\n## Personality\n{block}\n", soul.trim_end())
}

/// The handle (slug) a user may pick — same alphabet as SLUG_RE in
/// fleet-create. 2–30 lowercase letters/numbers, starting with a letter.
pub const HANDLE_RE_NOTE: &str =
    "handles are 2–30 lowercase letters/numbers, starting with a letter";

pub fn handle_ok(handle: &str) -> bool {
    let n = handle.len();
    if !(2..=30).contains(&n) {
        return false;
    }
    let mut chars = handle.chars();
    let first = chars.next().unwrap_or('!');
    first.is_ascii_lowercase() && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The dev-work routing rule, stamped beside the human-in-the-loop line:
/// code changes ship through a ticket and a workbench job, however small,
/// and instructions given in chat land on the ticket.
const DEV_WORK_RULE: &str = "- Never do dev work in chat: code changes ship through a ticket and a workbench job, however small, and instructions given in chat land on that ticket as comments.";

const DEFAULT_PERSONALITY: &str = "Be warm, direct, and useful. Lead with the answer, keep routine replies short, and ask rather than guess when a request is ambiguous.";

fn personal_soul(display_name: &str, owner_name: &str, personality: &str) -> String {
    format!(
        "# {display_name} — {owner_name}'s personal assistant\n\n\
         ## Who you are\n\
         You are {display_name}, {owner_name}'s personal assistant inside Talaria. You work\n\
         for {owner_name} specifically: their tasks, their notes, their preferences.\n\n\
         ## Personality\n\
         {PERSONA_START}\n{}\n{PERSONA_END}\n\n\
         ## How you work\n\
         - Keep {owner_name} in the loop: create and triage tickets, never assign or close them.\n\
         {DEV_WORK_RULE}\n\
         - Prefer the local model tier for routine work; escalate deliberately.\n\
         - Remember durable preferences and context in your memory as you learn them.\n",
        personality.trim()
    )
}

/// One named model tier the owner can switch between (a config alias).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantTier {
    pub name: String,
    pub model: String,
    /// This tier is the current default (main) target.
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalAgent {
    pub id: String,
    pub slug: String,
    pub model: String,
    pub department: String,
    pub display_name: String,
    pub enabled: bool,
    /// Owner-authored personality text (the marked soul section).
    pub personality: Option<String>,
    /// Container reality — matches the agents-roster dots.
    pub running: bool,
    /// The main target's model, e.g. "qwen3-a3b" — what powers it right now.
    pub current_model: Option<String>,
    /// Named model tiers the owner can switch between (the config aliases).
    pub tiers: Vec<AssistantTier>,
}

/// The user's personal agent, if they have one.
pub async fn personal_agent_for(
    pg: &PgPool,
    user_id: &str,
) -> Result<Option<PersonalAgent>, sqlx::Error> {
    let def: Option<(String, String, String, String, String, bool)> = sqlx::query_as(
        "select id::text, slug, model, department, display_name, enabled \
         from agent_defs where owner_user_id = $1::uuid limit 1",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    let Some((id, slug, model, department, display_name, enabled)) = def else {
        return Ok(None);
    };
    let departments = vec![department.clone()];
    let (latest, containers, displaced) = tokio::join!(
        list_versions(pg, &id),
        container_status(&departments),
        talaria_agent_defs::runner_instead_of(pg, &id, &department)
    );
    let latest = latest?.into_iter().next();
    // The container is found by department; where another agent shares this
    // one's and runs there, it is not this assistant's, and its state is not
    // this assistant's either.
    let own_container = displaced?.is_none();
    let containers = containers.unwrap_or_default();
    let main = latest.as_ref().and_then(|v| v.config.get("main")).cloned();
    let main_target: Option<ModelTarget> = main
        .as_ref()
        .and_then(|m| serde_json::from_value(m.clone()).ok());
    let main_endpoint = main
        .as_ref()
        .and_then(|m| m.get("endpoint"))
        .and_then(Value::as_str);
    let main_model = main
        .as_ref()
        .and_then(|m| m.get("model"))
        .and_then(Value::as_str);
    let aliases: Vec<Value> = latest
        .as_ref()
        .and_then(|v| v.config.get("aliases"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tiers = aliases
        .iter()
        .filter_map(|a| {
            let name = a.get("name")?.as_str()?.to_string();
            let model = a.get("model")?.as_str()?.to_string();
            let endpoint = a.get("endpoint").and_then(Value::as_str);
            let active = main_endpoint.is_some_and(|me| endpoint == Some(me))
                && main_model.is_some_and(|mm| model == mm);
            Some(AssistantTier {
                name,
                model,
                active,
            })
        })
        .collect();
    Ok(Some(PersonalAgent {
        id,
        slug,
        model,
        department,
        display_name,
        enabled,
        personality: latest.as_ref().and_then(|v| personality_of(&v.soul)),
        running: own_container
            && containers
                .first()
                .and_then(|c| c.managed.as_ref())
                .is_some_and(|m| m.state == "running"),
        current_model: main_target.as_ref().map(|t| t.model.clone()),
        tiers,
    }))
}

/// slug + department derived from the user — BOTH must be unique, since the
/// department names the container (agent-<department>) and its memory.
/// `create_agent` refuses a taken slug and a taken department alike.
fn personal_identity(email: Option<&str>, name: Option<&str>) -> (String, String) {
    let raw = email
        .map(|e| e.split('@').next().unwrap_or(""))
        .or(name)
        .unwrap_or("me");
    let base: String = raw
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut base: String = base.chars().take(24).collect();
    if base.is_empty() {
        base.push_str("me");
    }
    let mut slug = format!("pa{base}");
    slug.truncate(30);
    let mut department = format!("personal-{base}");
    department.truncate(40);
    (slug, department)
}

/// Who the create/update flows call back for: the three identifiers a
/// personal agent keys on.
pub struct PersonalUser<'a> {
    pub id: &'a str,
    pub email: Option<&'a str>,
    pub name: Option<&'a str>,
}

pub struct PersonalAgentInput<'a> {
    /// Display name, e.g. "Maxie".
    pub name: Option<&'a str>,
    /// Handle → slug + container identity. Immutable after creation.
    pub handle: Option<&'a str>,
    pub personality: Option<&'a str>,
}

fn owner_first_name(user: &PersonalUser<'_>) -> String {
    user.name
        .map(|n| n.split(' ').next().unwrap_or(""))
        .filter(|s| !s.is_empty())
        .or_else(|| user.email.map(|e| e.split('@').next().unwrap_or("")))
        .filter(|s| !s.is_empty())
        .unwrap_or("you")
        .to_string()
}

fn private_doc_sync(state: &AppState, user_id: &str) {
    let pg = state.pg.clone();
    let user_id = user_id.to_string();
    tokio::spawn(async move {
        if let Some(f) = SYNC_PRIVATE_DOCS.get() {
            f(pg, user_id).await;
        }
    });
}

/// A [`PersonalUser`] an owner's call carries onto a task of its own.
struct OwnedUser {
    id: String,
    email: Option<String>,
    name: Option<String>,
}

impl OwnedUser {
    fn of(user: &PersonalUser<'_>) -> Self {
        OwnedUser {
            id: user.id.to_string(),
            email: user.email.map(str::to_string),
            name: user.name.map(str::to_string),
        }
    }

    fn user(&self) -> PersonalUser<'_> {
        PersonalUser {
            id: &self.id,
            email: self.email.as_deref(),
            name: self.name.as_deref(),
        }
    }
}

// AN OWNER'S CALL RUNS WHOLE ON A TASK OF ITS OWN (talaria-fleet-docker
// `run_detached`), like a control verb. Each is several steps — a create's
// def, grants, boards and bring-up; an edit's version, a rename's one
// transaction then its directory move, the render and the apply — and a
// request future that is dropped (a closed tab, a curl that gave up, the app
// proxy's 300 s) stops at whichever await it was on. Measured live under IO
// starvation: a rename's COMMIT took 5 s, the caller left at 5 s, and the
// assistant was left renamed in the database with its files, its render and
// its container still on the old handle. The caller waits `ANSWER_WITHIN`
// for the answer; past that it gets the assistant as it is, and the call goes
// on to its end.

/// Create + start a personal agent for the user, based on a template (any
/// enabled agent). Idempotent: an existing assistant is returned as-is
/// (re-enabled if retired) — creation options don't apply. Runs whole on a
/// task of its own (above).
pub async fn create_personal_agent(
    state: &AppState,
    user: &PersonalUser<'_>,
    input: &PersonalAgentInput<'_>,
) -> Result<PersonalAgent, String> {
    let (on_task, owner) = (state.clone(), OwnedUser::of(user));
    let [name, handle, personality] =
        [input.name, input.handle, input.personality].map(|f| f.map(str::to_string));
    let create = async move {
        let input = PersonalAgentInput {
            name: name.as_deref(),
            handle: handle.as_deref(),
            personality: personality.as_deref(),
        };
        create_whole(&on_task, &owner.user(), &input).await
    };
    match run_detached(create, ANSWER_WITHIN).await {
        Detached::Settled(answer) => answer,
        Detached::StillRunning => {
            tracing::warn!(
                "[personal-agent] creating {}'s assistant is still running after {}s — answering with it as it is; it goes on",
                user.id,
                ANSWER_WITHIN.as_secs()
            );
            personal_agent_for(&state.pg, user.id)
                .await
                .map_err(|e| store_fault("[personal-agent] assistant read failed", e))?
                .ok_or_else(|| {
                    "your assistant is still being set up — it appears here in a moment".to_string()
                })
        }
        // A panic's text is the server's, never the caller's: logged here,
        // and the route answers the house 500.
        Detached::Lost(why) => Err(store_fault(
            "[personal-agent] creating the assistant did not finish",
            why,
        )),
    }
}

async fn create_whole(
    state: &AppState,
    user: &PersonalUser<'_>,
    input: &PersonalAgentInput<'_>,
) -> Result<PersonalAgent, String> {
    let pg = &state.pg;
    if let Some(existing) = personal_agent_for(pg, user.id)
        .await
        .map_err(|e| store_fault("[personal-agent] assistant read failed", e))?
    {
        if !existing.enabled {
            // ONE CONTAINER PER DEPARTMENT. Where another agent holds this
            // assistant's department too (a pair from before a department
            // became one agent's) and runs in its container, bringing this
            // one back would take that container from it at the next render
            // when this one is the older — the takeover the control route's
            // unretire refuses — or leave it with no container at all. So it
            // stays retired, and the owner is told without naming the other.
            if talaria_agent_defs::runner_instead_of(pg, &existing.id, &existing.department)
                .await
                .map_err(|e| store_fault("[personal-agent] department holder read failed", e))?
                .is_some()
            {
                return Err(ASSISTANT_DISPLACED.to_string());
            }
            sqlx::query(
                "update agent_defs set enabled = true, updated_at = now() where id = $1::uuid",
            )
            .bind(&existing.id)
            .execute(pg)
            .await
            .map_err(|e| store_fault("[personal-agent] re-enable failed", e))?;
        }
        // Make sure their personal RAG exists + their agent is bound to it.
        let qd = qdrant::real_deps();
        let ed = embed::real_deps();
        let _ = ensure_personal_collection(
            pg,
            &qd,
            &ed,
            user.id,
            &talaria_retrieval_collections::PersonalOpts {
                name: None,
                agent_model: Some(&existing.model),
            },
        )
        .await;
        private_doc_sync(state, user.id);
        return Ok(PersonalAgent {
            enabled: true,
            ..existing
        });
    }

    // CHASSIS, not identity. The only thing an existing agent supplies here is
    // model tiers, tools and plugins — the assistant's soul, role and
    // department are all written below. Cloning one when it exists is a
    // convenience: a new assistant inherits whatever endpoint the fleet
    // already runs on. It is NOT a requirement: a fresh install has no agents,
    // and the first thing a new user asks for — their own assistant — must not
    // answer "import a stack first". `create_agent` falls back to platform
    // defaults, and if there is no endpoint either, IT says so, which is the
    // real missing piece.
    let tmpl: Option<(String,)> = sqlx::query_as(
        "select id::text from agent_defs where enabled \
         order by (department = 'administrative-assistant') desc, updated_at desc limit 1",
    )
    .fetch_optional(pg)
    .await
    .map_err(|e| store_fault("[personal-agent] template read failed", e))?;

    let owner_name = owner_first_name(user);
    let display_name = input
        .name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{owner_name}'s assistant"));
    let (slug, department) = match input.handle.filter(|h| !h.is_empty()) {
        Some(handle) => {
            let mut department = format!("personal-{handle}");
            department.truncate(40);
            (handle.to_string(), department)
        }
        None => personal_identity(user.email, user.name),
    };
    let personality = input
        .personality
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(DEFAULT_PERSONALITY);

    let created = create_agent(
        pg,
        &CreateAgentInput {
            slug: slug.clone(),
            department: department.clone(),
            display_name: display_name.clone(),
            role: Some("Personal assistant".into()),
            template_id: tmpl.map(|(id,)| id),
            created_by: user.email.or(user.name).unwrap_or("user").to_string(),
            soul: Some(personal_soul(&display_name, &owner_name, personality)),
            manager_user_id: Some(user.id.to_string()),
        },
    )
    .await?;
    let def: &AgentDefRow = &created.def;

    // Mark ownership + grant the user access to their own agent.
    sqlx::query("update agent_defs set owner_user_id = $1::uuid where id = $2::uuid")
        .bind(user.id)
        .bind(&def.id)
        .execute(pg)
        .await
        .map_err(|e| store_fault("[personal-agent] owner stamp failed", e))?;
    // A personal assistant works for ONE person, so the recall the closed
    // default keeps off a shared agent (`session_search`) is this person's
    // own: open it now that the owner is marked. The create path could not
    // know — it writes the shared-agent default for every new def.
    talaria_agent_defs::open_toolsets(pg, &def.id, talaria_agent_defs::SHARED_AGENT_DISABLED)
        .await
        .map_err(|e| store_fault("[personal-agent] toolset open failed", e))?;
    sqlx::query(
        "insert into user_agent_access (user_id, agent_model) values ($1::uuid, $2) \
         on conflict do nothing",
    )
    .bind(user.id)
    .bind(&def.model)
    .execute(pg)
    .await
    .map_err(|e| store_fault("[personal-agent] owner access grant failed", e))?;
    // ...and start it ALLOWED on the boards it will be told it owns: GET
    // /api/boards owner-proxies the owner's boards to a personal assistant
    // from the moment it exists, and every board-scoped route answers the
    // allowlist — without this, a fresh assistant sees boards it can only ever
    // 403 against (boards created before it existed). Boards the owner does
    // NOT own stay the board owner's call, and a removal via set_board_agents
    // is never re-added.
    sqlx::query(
        "insert into board_agents (board_id, agent_model) \
         select id, $1 from boards where owner_id = $2::uuid on conflict do nothing",
    )
    .bind(&def.model)
    .bind(user.id)
    .execute(pg)
    .await
    .map_err(|e| store_fault("[personal-agent] board grant failed", e))?;

    // Personal RAG: a private collection bound to the user + this agent,
    // seeded with any private docs they already have.
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    let _ = ensure_personal_collection(
        pg,
        &qd,
        &ed,
        user.id,
        &talaria_retrieval_collections::PersonalOpts {
            name: Some(&format!("{display_name} · knowledge")),
            agent_model: Some(&def.model),
        },
    )
    .await;
    private_doc_sync(state, user.id);

    // The render and the bring-up, on the call's own task (above), so a
    // caller who leaves no longer kills compose halfway through. The
    // assistant row is written either way; a bring-up that fails is the
    // operator's to read — compose's own words, never the user's.
    if let Ok(sb) = state.secretbox().await
        && let Some(rf) = talaria_fleet_create::RENDER_FLEET.get()
    {
        let _ = rf(pg.clone(), sb).await;
    }
    if let Err(e) = fleet_up(pg, &department).await {
        tracing::warn!("[personal-agent] {department}: the assistant did not come up: {e}");
    }
    wait_healthy_spawn(state, &department);

    personal_agent_for(pg, user.id)
        .await
        .map_err(|e| store_fault("[personal-agent] assistant read failed", e))?
        .ok_or_else(|| "assistant vanished during creation".to_string())
}

/// Fire-and-forget the health wait with the default 120s budget — creation
/// returns without waiting on container health.
fn wait_healthy_spawn(state: &AppState, department: &str) {
    let pg = state.pg.clone();
    let department = department.to_string();
    tokio::spawn(async move {
        let _ = wait_healthy(&pg, &department, 120_000).await;
    });
}

async fn rename_agent_slug(
    pg: &PgPool,
    def: &AgentDefRow,
    new_slug: &str,
) -> Result<String, String> {
    let exists: Option<(i32,)> = sqlx::query_as("select 1 from agent_defs where slug = $1")
        .bind(new_slug)
        .fetch_optional(pg)
        .await
        .map_err(|e| store_fault("[personal-agent] handle check failed", e))?;
    if exists.is_some() {
        return Err(format!("agent \"{new_slug}\" already exists"));
    }
    let new_model = format!("{new_slug}-{}", def.department);
    // The fleet .env's own failure names its path: the operator's, in the
    // log, and the caller's house 500.
    ensure_agent_key(new_slug)
        .await
        .map_err(|e| store_fault("[personal-agent] rename: agent key write failed", e))?;
    // ONE TRANSACTION: the def's slug and every table keyed on its model
    // move together or not at all — a rename that landed on agent_defs and
    // died on conversations would leave threads, grants and schedules
    // pointing at a model that no longer exists. The skills directory
    // moves after the commit (best-effort, as before): a directory that
    // stayed under the old slug is a missing skill, not a split identity.
    let mut tx = pg
        .begin()
        .await
        .map_err(|e| store_fault("[personal-agent] rename: begin failed", e))?;
    for (sql, bind) in [
        (
            "update agent_defs set slug = $1, model = $2, updated_at = now() where id = $3::uuid",
            vec![new_slug, new_model.as_str(), def.id.as_str()],
        ),
        (
            "update user_agent_access set agent_model = $1 where agent_model = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
        (
            "update conversations set agent_model = $1 where agent_model = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
        (
            "update board_agents set agent_model = $1 where agent_model = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
        (
            "update channel_agents set agent_model = $1 where agent_model = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
        (
            "update usage_events set agent_model = $1 where agent_model = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
        // The relay's attribution records are keyed by the agent too: left
        // behind, the agent's next calls in a conversation it already
        // reported would book `unattended`. A stale row under the new name
        // (a deleted agent's) goes first, so the move cannot collide.
        (
            "delete from usage_attributions where agent_model = $1",
            vec![new_model.as_str()],
        ),
        (
            "update usage_attributions set agent_model = $1 where agent_model = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
        (
            "delete from usage_affinity_keys where agent_model = $1",
            vec![new_model.as_str()],
        ),
        (
            "update usage_affinity_keys set agent_model = $1 where agent_model = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
        // The threads listening to this agent's cron jobs are keyed by its
        // model too; left behind, every scheduled run would post into
        // nothing after a rename.
        (
            "update conversation_schedules set agent_model = $1 where agent_model = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
        (
            "update rag_collection_access set principal_id = $1 \
             where principal_type = 'agent' and principal_id = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
    ] {
        let mut q = sqlx::query(sql);
        for b in bind {
            q = q.bind(b);
        }
        match q.execute(&mut *tx).await {
            Ok(_) => {}
            // Another rename took the handle between the check above and
            // this write: the caller's conflict, not a store fault.
            Err(sqlx::Error::Database(db))
                if db.is_unique_violation() && db.table() == Some("agent_defs") =>
            {
                return Err(format!("agent \"{new_slug}\" already exists"));
            }
            Err(e) => return Err(store_fault("[personal-agent] rename: write failed", e)),
        }
    }
    tx.commit()
        .await
        .map_err(|e| store_fault("[personal-agent] rename: commit failed", e))?;
    // Created agents keep their skills under fleet/agents/<slug>/ — carry them.
    let from = talaria_fleet_layout::fleet_dir()
        .join("agents")
        .join(&def.slug);
    let to = talaria_fleet_layout::fleet_dir()
        .join("agents")
        .join(new_slug);
    let _ = tokio::fs::rename(&from, &to).await;
    // The old slug's HERMES_KEY_<OLD> / TALARIA_AGENT_KEY_<OLD> lines are
    // stripped by the next render's `ensure_agent_env_keys`, which drops
    // every per-agent line whose slug is no longer in agent_defs.
    Ok(new_model)
}

pub struct PersonalAgentPatch<'a> {
    pub name: Option<&'a str>,
    pub handle: Option<&'a str>,
    pub personality: Option<&'a str>,
    /// A tier name from the assistant's `tiers` — becomes the default model.
    pub model: Option<&'a str>,
}

/// Owner-scoped edits: rename, change the @handle, rewrite the personality, or
/// switch the default model tier. Config/soul changes land as one new
/// immutable version and are APPLIED the way the admin editor applies an
/// edit: in place, the container rolled only when the render moved it, and a
/// stopped assistant left stopped for its next start to read. Runs whole on
/// a task of its own (above `create_personal_agent`).
pub async fn update_personal_agent(
    state: &AppState,
    user: &PersonalUser<'_>,
    patch: &PersonalAgentPatch<'_>,
) -> Result<PersonalAgent, String> {
    let (on_task, owner) = (state.clone(), OwnedUser::of(user));
    let [name, handle, personality, model] =
        [patch.name, patch.handle, patch.personality, patch.model].map(|f| f.map(str::to_string));
    let edit = async move {
        let patch = PersonalAgentPatch {
            name: name.as_deref(),
            handle: handle.as_deref(),
            personality: personality.as_deref(),
            model: model.as_deref(),
        };
        update_whole(&on_task, &owner.user(), &patch).await
    };
    match run_detached(edit, ANSWER_WITHIN).await {
        Detached::Settled(answer) => answer,
        Detached::StillRunning => {
            tracing::warn!(
                "[personal-agent] {}'s assistant update is still running after {}s — answering with it as it is; it goes on",
                user.id,
                ANSWER_WITHIN.as_secs()
            );
            personal_agent_for(&state.pg, user.id)
                .await
                .map_err(|e| store_fault("[personal-agent] assistant read failed", e))?
                .ok_or_else(|| "assistant vanished during update".to_string())
        }
        Detached::Lost(why) => Err(store_fault(
            "[personal-agent] the assistant's update did not finish",
            why,
        )),
    }
}

async fn update_whole(
    state: &AppState,
    user: &PersonalUser<'_>,
    patch: &PersonalAgentPatch<'_>,
) -> Result<PersonalAgent, String> {
    let pg = &state.pg;
    let def: Option<(String, String, String, String, String, bool, bool)> = sqlx::query_as(
        "select id::text, slug, model, department, display_name, managed, enabled \
         from agent_defs where owner_user_id = $1::uuid limit 1",
    )
    .bind(user.id)
    .fetch_optional(pg)
    .await
    .map_err(|e| store_fault("[personal-agent] assistant read failed", e))?;
    let Some((id, slug, model, department, display_name, managed, enabled)) = def else {
        return Err("no assistant yet. Create one first.".into());
    };
    // The row the rename/apply edges below key on — AgentDefRow minus the
    // fields this flow never reads.
    let mut row = AgentDefRow {
        id: id.clone(),
        slug: slug.clone(),
        department: department.clone(),
        model: model.clone(),
        display_name: display_name.clone(),
        role: None,
        source: String::new(),
        managed,
        current_version: 0,
    };

    let new_name = patch.name.map(str::trim).filter(|n| !n.is_empty());
    let renamed = new_name.is_some_and(|n| n != display_name);
    if renamed && let Some(new_name) = new_name {
        sqlx::query(
            "update agent_defs set display_name = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(new_name)
        .bind(&id)
        .execute(pg)
        .await
        .map_err(|e| store_fault("[personal-agent] display name write failed", e))?;
    }

    let new_handle = patch.handle.map(str::trim).filter(|h| !h.is_empty());
    let rehandled = new_handle.is_some_and(|h| h != slug);
    if let Some(new_handle) = new_handle
        && rehandled
    {
        if !handle_ok(new_handle) {
            return Err(HANDLE_RE_NOTE.to_string());
        }
        row.model = rename_agent_slug(pg, &row, new_handle).await?;
        row.slug = new_handle.to_string();
    }

    let latest = list_versions(pg, &id)
        .await
        .map_err(|e| store_fault("[personal-agent] versions read failed", e))?
        .into_iter()
        .next();
    if let Some(latest) = latest {
        let mut soul = latest.soul.clone();
        let mut config = latest.config.clone();
        if rehandled && let Some(new_handle) = new_handle {
            config = restamp_slug(&config, &slug, new_handle);
        }
        if let Some(want) = patch.model.filter(|m| !m.is_empty()) {
            let aliases = config
                .get("aliases")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let target = aliases
                .iter()
                .find(|a| a.get("name").and_then(Value::as_str) == Some(want))
                .ok_or_else(|| format!("unknown model tier \"{want}\""))?;
            let alias_list: Vec<talaria_agent_defs::AliasTarget> = aliases
                .iter()
                .filter_map(|a| serde_json::from_value(a.clone()).ok())
                .collect();
            let fallbacks = config
                .get("fallbacks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();
            // `contextLength` rides only when truthy — null and 0 both
            // drop off the main target (ModelTarget's own serde rule).
            let mut main: ModelTarget = serde_json::from_value(target.clone()).map_err(|e| {
                store_fault(
                    "[personal-agent] a model tier in the stored config is not a model target",
                    e,
                )
            })?;
            main.context_length = main.context_length.filter(|c| *c != 0);
            config = apply_config_edits(
                pg,
                &config,
                &ConfigEdits {
                    main,
                    aliases: alias_list,
                    fallbacks,
                    // A model patch turns no compaction knob.
                    compression: CompressionEdits::default(),
                },
            )
            .await?;
        }
        // Renames carry into the soul so the agent knows its own name;
        // exact-match replace is safe because display names are distinctive
        // multi-char strings.
        if renamed
            && display_name.chars().count() >= 3
            && let Some(new_name) = new_name
        {
            soul = soul.split(&display_name).collect::<Vec<_>>().join(new_name);
        }
        if let Some(personality) = patch.personality {
            soul = with_personality(&soul, personality);
        }
        let created = add_version_if_changed(
            pg,
            &id,
            &talaria_agent_defs::NewVersion {
                soul: &soul,
                config: &config,
                note: Some("personalized by owner"),
                created_by: Some(user.email.or(user.name).unwrap_or("owner")),
            },
        )
        .await
        .map_err(|e| store_fault("[personal-agent] version write failed", e))?
        .1;
        if (created || rehandled) && managed && enabled {
            // A render failure fails the whole PATCH — after
            // the version row was written; that write stands. Only the
            // apply's roll is best-effort.
            let sb = state
                .secretbox()
                .await
                .map_err(|e| store_fault("[personal-agent] secretbox unusable", e))?;
            let rf = talaria_fleet_create::RENDER_FLEET.get().ok_or_else(|| {
                store_fault("[personal-agent] render", "fleet renderer is not wired")
            })?;
            // APPLIED, like the admin editor's edit, never bounced: the render
            // rewrites SOUL.md and config.yaml in place, and the container is
            // rolled only when the render moved it (talaria-fleet-reconcile
            // `apply_after_render`). A personality or name edit reaches the
            // running assistant from its next new conversation without a
            // restart; a model tier the agent reads only at start, or a
            // handle rename (key env, model name, config-mount path — what a
            // plain restart would keep, crashing on the moved dir), rolls it.
            // A stopped assistant stays stopped: a `compose restart` here
            // started it again, and no edit starts an agent its owner stopped.
            // Its next start reads the render, and compose recreates a
            // container whose service moved.
            //
            // On the call's own task (above `create_personal_agent`): a
            // caller that goes away mid-roll no longer takes the compose verb
            // with it. A render that fails still fails the PATCH.
            //
            // The render's error is the operator's (a file, YAML, a docker
            // failure, or the database under it), never the owner's to act
            // on: logged in full here, and the owner gets the house 500.
            rf(pg.clone(), sb.clone()).await.map_err(|e| {
                store_fault(
                    "[personal-agent] the update is saved, but its render failed",
                    e,
                )
            })?;
            // The container is the department's, and where another agent
            // shares this one's department and runs there (a pair from
            // before a department became one agent's), it is THAT agent's:
            // the render left this one out, and applying here would roll
            // somebody else's agent for this owner's edit. The version
            // stands for when the two are untied.
            if let Some(runner) = talaria_agent_defs::runner_instead_of(pg, &id, &department)
                .await
                .map_err(|e| store_fault("[personal-agent] department holder read failed", e))?
            {
                tracing::warn!(
                    "[personal-agent] {}: saved, not applied — {}",
                    row.slug,
                    talaria_agent_defs::shared_department_refusal(
                        "roll",
                        &display_name,
                        &department,
                        &runner.display_name,
                    )
                );
                return personal_agent_for(pg, user.id)
                    .await
                    .map_err(|e| store_fault("[personal-agent] assistant read failed", e))?
                    .ok_or_else(|| "assistant vanished during update".to_string());
            }
            for (_, verdict) in apply_after_render(pg, &sb, std::slice::from_ref(&department)).await
            {
                match verdict {
                    Ok(None) => {}
                    Ok(Some(warning)) => tracing::warn!(
                        "[personal-agent] {department}: the update is saved and rendered, but: {warning}"
                    ),
                    Err(e) => tracing::warn!(
                        "[personal-agent] {department}: the update is saved and rendered, but applying it failed: {e}"
                    ),
                }
            }
        }
    }

    personal_agent_for(pg, user.id)
        .await
        .map_err(|e| store_fault("[personal-agent] assistant read failed", e))?
        .ok_or_else(|| "assistant vanished during update".to_string())
}

/// Does this user own the agent (by slug or def id)? Used to open selected
/// admin surfaces (skills, memory, start/stop) to an assistant's owner.
/// Fail-closed: any error — e.g. a non-uuid defId — reads as no.
pub async fn owns_agent(
    pg: &PgPool,
    user_id: &str,
    slug: Option<&str>,
    def_id: Option<&str>,
) -> bool {
    // An empty slug falls through to the defId
    // branch, an empty defId queries nothing — neither is a lookup.
    let slug = slug.filter(|s| !s.is_empty());
    let def_id = def_id.filter(|s| !s.is_empty());
    let found = if let Some(slug) = slug {
        sqlx::query_scalar::<_, i32>(
            "select 1 from agent_defs \
             where owner_user_id = $1::uuid and slug = $2",
        )
        .bind(user_id)
        .bind(slug)
        .fetch_optional(pg)
        .await
    } else if let Some(def_id) = def_id {
        sqlx::query_scalar::<_, i32>(
            "select 1 from agent_defs \
             where owner_user_id = $1::uuid and id = $2::uuid",
        )
        .bind(user_id)
        .bind(def_id)
        .fetch_optional(pg)
        .await
    } else {
        return false;
    };
    found.map(|row| row.is_some()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personality_extraction() {
        let soul = "## Role\nYou help.\n\n<!-- talaria:personality -->\nWarm and brief.\n<!-- /talaria:personality -->\n";
        assert_eq!(personality_of(soul).as_deref(), Some("Warm and brief."));
    }

    #[test]
    fn personality_markers_missing_or_swapped() {
        assert_eq!(personality_of("no markers at all"), None);
        assert_eq!(
            personality_of("<!-- /talaria:personality -->x<!-- talaria:personality -->"),
            None
        );
        // end marker alone, start marker alone
        assert_eq!(personality_of("<!-- /talaria:personality -->"), None);
        assert_eq!(personality_of("<!-- talaria:personality -->"), None);
    }

    #[test]
    fn personality_empty_section_is_null() {
        assert_eq!(
            personality_of("a<!-- talaria:personality --><!-- /talaria:personality -->b"),
            None
        );
        assert_eq!(
            personality_of("a<!-- talaria:personality --> \n\t <!-- /talaria:personality -->b"),
            None
        );
    }

    #[test]
    fn personality_first_marker_pair_wins() {
        // The FIRST occurrence of each marker wins — a second start
        // marker inside the section is content, not a delimiter.
        let soul = "<!-- talaria:personality -->one<!-- talaria:personality -->two<!-- /talaria:personality -->";
        assert_eq!(
            personality_of(soul).as_deref(),
            Some("one<!-- talaria:personality -->two")
        );
    }

    #[test]
    fn personality_trims_both_sides() {
        let soul = "<!-- talaria:personality -->\n\n  keep this  \n\n<!-- /talaria:personality -->";
        assert_eq!(personality_of(soul).as_deref(), Some("keep this"));
    }
}
