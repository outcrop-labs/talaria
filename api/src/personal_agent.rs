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

use crate::agent_defs::{
    AgentDefRow, ConfigEdits, ModelTarget, add_version_if_changed, apply_config_edits,
    list_versions,
};
use crate::fleet::create::{CreateAgentInput, create_agent, ensure_agent_key, restamp_slug};
use crate::fleet::docker::{container_status, fleet_restart, fleet_up, wait_healthy};
use crate::fleet::render::render_fleet;
use crate::kb::sync_user_private_docs;
use crate::retrieval::collections::ensure_personal_collection;
use crate::retrieval::{embed, qdrant};
use crate::state::AppState;

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
    let (latest, containers) = tokio::join!(list_versions(pg, &id), container_status(&departments));
    let latest = latest?.into_iter().next();
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
        running: containers
            .first()
            .and_then(|c| c.managed.as_ref())
            .is_some_and(|m| m.state == "running"),
        current_model: main_target.as_ref().map(|t| t.model.clone()),
        tiers,
    }))
}

/// slug + department derived from the user — BOTH must be unique per user,
/// since the department names the container (agent-<department>). `create_agent`
/// enforces slug uniqueness and errs on collision.
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
        let qd = qdrant::real_deps();
        let ed = embed::real_deps();
        let _ = sync_user_private_docs(&pg, &qd, &ed, &user_id).await;
    });
}

/// Create + start a personal agent for the user, based on a template (any
/// enabled agent). Idempotent: an existing assistant is returned as-is
/// (re-enabled if retired) — creation options don't apply.
pub async fn create_personal_agent(
    state: &AppState,
    user: &PersonalUser<'_>,
    input: &PersonalAgentInput<'_>,
) -> Result<PersonalAgent, String> {
    let pg = &state.pg;
    if let Some(existing) = personal_agent_for(pg, user.id)
        .await
        .map_err(|e| e.to_string())?
    {
        if !existing.enabled {
            sqlx::query(
                "update agent_defs set enabled = true, updated_at = now() where id = $1::uuid",
            )
            .bind(&existing.id)
            .execute(pg)
            .await
            .map_err(|e| e.to_string())?;
        }
        // Make sure their personal RAG exists + their agent is bound to it.
        let qd = qdrant::real_deps();
        let ed = embed::real_deps();
        let _ = ensure_personal_collection(
            pg,
            &qd,
            &ed,
            user.id,
            &crate::retrieval::collections::PersonalOpts {
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
    .map_err(|e| e.to_string())?;

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
        .map_err(|e| e.to_string())?;
    sqlx::query(
        "insert into user_agent_access (user_id, agent_model) values ($1::uuid, $2) \
         on conflict do nothing",
    )
    .bind(user.id)
    .bind(&def.model)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
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
    .map_err(|e| e.to_string())?;

    // Personal RAG: a private collection bound to the user + this agent,
    // seeded with any private docs they already have.
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    let _ = ensure_personal_collection(
        pg,
        &qd,
        &ed,
        user.id,
        &crate::retrieval::collections::PersonalOpts {
            name: Some(&format!("{display_name} · knowledge")),
            agent_model: Some(&def.model),
        },
    )
    .await;
    private_doc_sync(state, user.id);

    if let Ok(sb) = state.secretbox().await {
        let _ = render_fleet(pg, &sb, None).await;
    }
    let _ = fleet_up(pg, &department).await;
    wait_healthy_spawn(state, &department);

    personal_agent_for(pg, user.id)
        .await
        .map_err(|e| e.to_string())?
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

/// Rename the slug (@handle). The department — container name + state volume —
/// stays put, so the agent keeps its memory and workspace. Everything keyed by
/// the model string follows: access grants, chat history, board/channel
/// membership, usage attribution, the heartbeat registry. The old
/// HERMES_KEY_<SLUG> line is left in the stack .env (harmless orphan).
async fn rename_agent_slug(
    pg: &PgPool,
    def: &AgentDefRow,
    new_slug: &str,
) -> Result<String, String> {
    let exists: Option<(i32,)> = sqlx::query_as("select 1 from agent_defs where slug = $1")
        .bind(new_slug)
        .fetch_optional(pg)
        .await
        .map_err(|e| e.to_string())?;
    if exists.is_some() {
        return Err(format!("agent \"{new_slug}\" already exists"));
    }
    let new_model = format!("{new_slug}-{}", def.department);
    ensure_agent_key(new_slug).await?;
    // Created agents keep their skills under fleet/agents/<slug>/ — carry them.
    let from = crate::fleet::layout::fleet_dir()
        .join("agents")
        .join(&def.slug);
    let to = crate::fleet::layout::fleet_dir()
        .join("agents")
        .join(new_slug);
    let _ = tokio::fs::rename(&from, &to).await;
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
        (
            "update rag_collection_access set principal_id = $1 \
             where principal_type = 'agent' and principal_id = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
        (
            "delete from fleet_agents where name = $1",
            vec![new_model.as_str()],
        ),
        (
            "update fleet_agents set name = $1 where name = $2",
            vec![new_model.as_str(), def.model.as_str()],
        ),
    ] {
        let mut q = sqlx::query(sql);
        for b in bind {
            q = q.bind(b);
        }
        q.execute(pg).await.map_err(|e| e.to_string())?;
    }
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
/// immutable version and are applied to the running container right away —
/// same pipeline as the admin editor.
pub async fn update_personal_agent(
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
    .map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())?
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
            let alias_list: Vec<crate::agent_defs::AliasTarget> = aliases
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
            let mut main: ModelTarget =
                serde_json::from_value(target.clone()).map_err(|e| e.to_string())?;
            main.context_length = main.context_length.filter(|c| *c != 0);
            config = apply_config_edits(
                pg,
                &config,
                &ConfigEdits {
                    main,
                    aliases: alias_list,
                    fallbacks,
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
            &crate::agent_defs::NewVersion {
                soul: &soul,
                config: &config,
                note: Some("personalized by owner"),
                created_by: Some(user.email.or(user.name).unwrap_or("owner")),
            },
        )
        .await
        .map_err(|e| e.to_string())?
        .1;
        if (created || rehandled) && managed && enabled {
            // A render failure fails the whole PATCH — after
            // the version row was written; that write stands. Only the docker
            // restart/up half is best-effort.
            let sb = state.secretbox().await.map_err(|e| e.to_string())?;
            render_fleet(pg, &sb, None).await?;
            // A handle rename changes the service definition (key env, model
            // name, config-mount path), so the container must be RECREATED
            // (up -d), not restarted — restart keeps the old mounts and
            // crashes on the moved dir.
            if rehandled {
                let _ = fleet_up(pg, &department).await;
            } else {
                let _ = fleet_restart(pg, &department).await;
            }
        }
    }

    personal_agent_for(pg, user.id)
        .await
        .map_err(|e| e.to_string())?
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
