// Create a brand-new agent — from a template (an existing agent's
// definition, model tiers/tools cloned with identity re-stamped) or, with no
// template, from the platform defaults (first local endpoint's model). The
// soul is a scaffold (or supplied, e.g. Muse-designed). A fresh gateway key
// is allocated into the fleet .env (which the renderer + manifest read).
// The delete half lives at the bottom of this file.

use serde_json::Value;
use sqlx::PgPool;

use talaria_agent_defs::{
    AgentDefRow, CompressionEdits, ConfigEdits, ModelTarget, NewVersion, UpsertDef, UpsertDefError,
    add_version_if_changed, agent_def_by_id, agent_def_by_slug, apply_config_edits_over,
    department_holder, dept_ok, list_versions, slug_ok, upsert_agent_def,
};
use talaria_gateway::registry::list_endpoints;

use futures_util::future::BoxFuture;
use std::sync::{Arc, OnceLock};

/// Wired from the api binary so delete can re-render without this crate
/// depending on fleet/render.rs.
pub static RENDER_FLEET: OnceLock<
    Arc<
        dyn Fn(sqlx::PgPool, talaria_secretbox::SecretBox) -> BoxFuture<'static, Result<(), String>>
            + Send
            + Sync,
    >,
> = OnceLock::new();

/// Re-stamp identity: replace a slug in every string value of the raw config
/// (X-Agent-Name headers, hook args like "outline_org_gate.py sam"). Used for
/// template cloning and for handle renames.
pub fn restamp_slug(value: &Value, from: &str, to: &str) -> Value {
    // Both slugs passed the alphabet on their way in, so neither carries
    // regex metacharacters; \b guards the boundary.
    // Built per call — a hire stamps once, never in a loop.
    let re = regex::Regex::new(&format!(r"\b{from}\b")).expect("slug alphabets are regex-safe");
    fn walk(value: &Value, re: &regex::Regex, to: &str) -> Value {
        match value {
            Value::String(s) => Value::String(re.replace_all(s, to).into_owned()),
            Value::Array(items) => Value::Array(items.iter().map(|v| walk(v, re, to)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), walk(v, re, to)))
                    .collect(),
            ),
            leaf => leaf.clone(),
        }
    }
    walk(value, &re, to)
}

fn starter_soul(display_name: &str, department: &str) -> String {
    let role = department.replace('-', " ");
    format!(
        "# {display_name} — {role}

## Who you are
You are {display_name}, the {role} agent. (Written by Talaria's template —
replace this section with a real personality and operating principles.)

## How you work
- Keep humans in the loop: create and triage tickets, never assign or close them.
- Never do dev work in chat: code changes ship through a ticket and a workbench job, however small, and instructions given in chat land on that ticket as comments.
- Prefer the local model tier for routine work; escalate deliberately.
- When unsure, ask in the channel instead of guessing.
"
    )
}

/// Ensure a HERMES_KEY_<SLUG> exists in the fleet .env; returns whether
/// created. The read is NOT best-effort — a hire against an unreadable fleet
/// env fails honestly.
pub async fn ensure_agent_key(slug: &str) -> Result<bool, String> {
    let env_path = talaria_fleet_layout::fleet_env();
    let name = format!("HERMES_KEY_{}", slug.to_uppercase());
    let content = tokio::fs::read_to_string(&env_path)
        .await
        .map_err(|e| format!("fleet .env unreadable ({}): {e}", env_path.display()))?;
    if content
        .lines()
        .any(|line| line.starts_with(&format!("{name}=")))
    {
        return Ok(false);
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| format!("key material unavailable: {e}"))?;
    let key: String = key.iter().map(|b| format!("{b:02x}")).collect();
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&env_path)
        .await
        .map_err(|e| format!("fleet .env unwritable ({}): {e}", env_path.display()))?;
    file.write_all(format!("\n# added by Talaria (agent create)\n{name}={key}\n").as_bytes())
        .await
        .map_err(|e| format!("fleet .env append failed: {e}"))?;
    // The file is fleet-wide plaintext credentials — never leave it readable
    // to every local account (fleet-render writes it 0600; appending must not
    // undo that, and an install created before that rule gets fixed here).
    // Best-effort.
    if let Ok(meta) = tokio::fs::metadata(&env_path).await {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = tokio::fs::set_permissions(&env_path, perms).await;
    }
    Ok(true)
}

/// `content` without `slug`'s per-agent lines — its HERMES_KEY line (with
/// the comment [`ensure_agent_key`] seeded above it) and its
/// TALARIA_AGENT_KEY line.
fn without_agent_env_lines(content: &str, slug: &str) -> String {
    let hermes = format!("HERMES_KEY_{}=", slug.to_uppercase());
    let agent_key = format!("{}=", talaria_fleet_layout::agent_key_var(slug));
    // The seeded comment rides out with the HERMES key line it announced
    // — only that line's pattern owns it, never the agent-key line.
    let comment = "# added by Talaria (agent create)";
    let comment_line = format!("{comment}\n");
    let mut next = String::with_capacity(content.len());
    for line in content.lines() {
        let is_hermes = line.starts_with(&hermes);
        let is_agent_key = line.starts_with(&agent_key);
        if is_hermes && next.ends_with(&comment_line) {
            next.truncate(next.len() - comment_line.len());
        }
        if !is_hermes && !is_agent_key {
            next.push_str(line);
            next.push('\n');
        }
    }
    next
}

/// Drop `slug`'s per-agent lines from the fleet .env, keeping it 0600.
/// Best-effort: the DB is authoritative, and the next render's stale-line
/// sweep drops a line for a slug no def holds anyway.
async fn strip_agent_env_lines(slug: &str) {
    let env_path = talaria_fleet_layout::fleet_env();
    let Ok(content) = tokio::fs::read_to_string(&env_path).await else {
        return;
    };
    let next = without_agent_env_lines(&content, slug);
    if next != content {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::write(&env_path, &next).await;
        if let Ok(meta) = tokio::fs::metadata(&env_path).await {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = tokio::fs::set_permissions(&env_path, perms).await;
        }
    }
}

pub struct CreateAgentInput {
    pub slug: String,
    pub department: String,
    pub display_name: String,
    pub role: Option<String>,
    /// Clone this agent's config; omit to start from the platform defaults.
    pub template_id: Option<String>,
    pub created_by: String,
    /// Override the starter-soul scaffold (e.g. a personalized soul).
    pub soul: Option<String>,
    /// The human this agent belongs to — its first manager, and so the only
    /// non-admin who can change it until they name others
    /// (docs/PERMISSIONS.md, "Agent managers"). A uuid, not the `created_by`
    /// display string. None when nobody can be traced: the agent lands
    /// admin-managed.
    pub manager_user_id: Option<String>,
}

pub struct CreatedAgent {
    pub def: AgentDefRow,
    pub key_created: bool,
}

/// The whole create flow: alphabets, exists pre-check, template-or-defaults
/// config, the fleet key, the def row, and the v1 version.
pub async fn create_agent(pg: &PgPool, input: &CreateAgentInput) -> Result<CreatedAgent, String> {
    if !slug_ok(&input.slug) {
        return Err("slug must be short lowercase alphanumeric (e.g. \"analyst\")".into());
    }
    if !dept_ok(&input.department) {
        return Err("department must be lowercase-kebab (e.g. \"research\")".into());
    }

    let exists: Option<(i32,)> = sqlx::query_as("select 1 from agent_defs where slug = $1")
        .bind(&input.slug)
        .fetch_optional(pg)
        .await
        .map_err(|e| talaria_error::store_fault("[fleet-create] slug check failed", e))?;
    if exists.is_some() {
        return Err(format!("agent \"{}\" already exists", input.slug));
    }
    // A department is one agent's container and memory. Asked here too, not
    // only under the write's lock, so a refusal leaves nothing behind (the
    // fleet key below is a line in a file); the lock is the race's answer.
    if let Some(taken) = department_holder(pg, &input.department, &input.slug)
        .await
        .map_err(|e| {
            talaria_error::store_fault("[fleet-create] department holder read failed", e)
        })?
    {
        return Err(taken.to_string());
    }

    // An empty-string templateId is no template (truthiness on the way in).
    let template_id = input.template_id.as_deref().filter(|t| !t.is_empty());
    let (config, note) = match template_id {
        Some(template_id) => {
            let template_version = list_versions(pg, template_id)
                .await
                .map_err(|e| {
                    talaria_error::store_fault("[fleet-create] template versions read failed", e)
                })?
                .into_iter()
                .next()
                .ok_or("template agent has no versions")?;
            let from_slug = agent_def_by_id(pg, template_id)
                .await
                .map_err(|e| talaria_error::store_fault("[fleet-create] template read failed", e))?
                .map(|d| d.slug)
                .ok_or("template agent not found")?;
            (
                restamp_slug(&template_version.config, &from_slug, &input.slug),
                format!("created from template {from_slug}"),
            )
        }
        None => {
            // Platform defaults: main model from the first local endpoint
            // (else any).
            let eps = list_endpoints(pg).await.map_err(|e| {
                talaria_error::store_fault("[fleet-create] endpoints read failed", e)
            })?;
            let ep = eps
                .iter()
                .find(|e| e.class == "local" && !e.models.is_empty())
                .or_else(|| eps.iter().find(|e| !e.models.is_empty()))
                .ok_or("no models configured. Add an LLM endpoint first.")?;
            // contextLength: null and 0 both skip the key (truthiness on the
            // way in).
            let context_length = ep.context_length.filter(|c| *c != 0);
            let main = ModelTarget {
                endpoint: ep.name.clone(),
                model: ep.models[0].clone(),
                context_length,
                effort: None,
            };
            let edits = ConfigEdits {
                main: main.clone(),
                aliases: Vec::new(),
                fallbacks: Vec::new(),
                // A hire turns no compaction knob — whatever the template's
                // own `raw.compression` says rides through untouched.
                compression: CompressionEdits::default(),
            };
            let bare = serde_json::json!({
                "main": serde_json::to_value(&main).unwrap(),
                "aliases": [],
                "fallbacks": [],
            });
            (
                apply_config_edits_over(&eps, &bare, &edits)?,
                "created from platform defaults".to_string(),
            )
        }
    };

    let key_created = ensure_agent_key(&input.slug).await?;

    let mut def = match upsert_agent_def(
        pg,
        &UpsertDef {
            slug: &input.slug,
            department: &input.department,
            display_name: &input.display_name,
            role: input.role.as_deref(),
            source: "created",
        },
    )
    .await
    {
        Ok(def) => def,
        // The race the pre-check above cannot see: another hire took the
        // department between it and the write's lock. The key line this
        // call just appended is a live credential for nobody — take it back
        // out now rather than leave it to the next render's stale-line
        // sweep, unless a def under this slug landed meanwhile (a second
        // hire of the same slug), whose line it now is.
        Err(UpsertDefError::DepartmentTaken(taken)) => {
            if key_created
                && agent_def_by_slug(pg, &input.slug)
                    .await
                    .ok()
                    .flatten()
                    .is_none()
            {
                strip_agent_env_lines(&input.slug).await;
            }
            return Err(taken.to_string());
        }
        Err(e) => {
            return Err(talaria_error::store_fault(
                "[fleet-create] def write failed",
                e,
            ));
        }
    };
    // MANAGED AND MANAGED-BY, IN ONE TRANSACTION, STRAIGHT AFTER THE ROW.
    // The def now exists under a slug that any later `create` will see as
    // taken; the ownership stamp has to land with it, or a process death
    // between the two leaves an agent the hirer does not manage — and the
    // resume path below (which adopts only a def the hirer manages) could
    // never pick it back up. The manager insert is `add_manager`'s own
    // statement, written here because that helper takes a pool, not a
    // transaction. (The version write that follows is idempotent on resume:
    // `add_version_if_changed` adds nothing a prior pass already wrote.)
    let manager = input.manager_user_id.as_deref().filter(|m| !m.is_empty());
    let mut tx = pg
        .begin()
        .await
        .map_err(|e| talaria_error::store_fault("[fleet-create] begin failed", e))?;
    sqlx::query("update agent_defs set managed = true, updated_at = now() where id = $1::uuid")
        .bind(&def.id)
        .execute(&mut *tx)
        .await
        .map_err(|e| talaria_error::store_fault("[fleet-create] managed stamp failed", e))?;
    if let Some(manager) = manager {
        sqlx::query(
            "insert into agent_managers (agent_id, user_id, added_by) \
             values ($1::uuid, $2::uuid, $2::uuid) on conflict do nothing",
        )
        .bind(&def.id)
        .bind(manager)
        .execute(&mut *tx)
        .await
        .map_err(|e| talaria_error::store_fault("[fleet-create] manager stamp failed", e))?;
    }
    tx.commit()
        .await
        .map_err(|e| talaria_error::store_fault("[fleet-create] commit failed", e))?;
    def.managed = true;

    // A template's config can name sealed literals (`${CONFIG_…}`, see
    // talaria_agent_secrets::plan_config_seal). They are the template's
    // secrets, and an agent's secrets are its managers' (docs/PERMISSIONS.md,
    // "Agent managers"): the hirer manages the clone and could read them out
    // of its environment. So the clone takes them along, before the version
    // that names them, only when the hirer may already change the template.
    // Otherwise they are left out of the clone and its first version's note
    // says where.
    let (config, note) = match template_id {
        Some(template_id) if !talaria_agent_secrets::config_secret_handles(&config).is_empty() => {
            if takes_template_secrets(pg, manager, template_id).await? {
                talaria_agent_secrets::copy_config_secrets(pg, template_id, &def.id, &config)
                    .await?;
                (config, note)
            } else {
                let (kept, withheld) = talaria_agent_secrets::withhold_config_secrets(&config);
                if withheld.is_empty() {
                    // Handles outside `raw` only: nothing a render emits.
                    (config, note)
                } else {
                    let note = format!(
                        "{note}; left out the template's sealed credentials at {} — only its managers can pass them on",
                        withheld.join(", ")
                    );
                    (kept, note)
                }
            }
        }
        _ => (config, note),
    };

    add_version_if_changed(
        pg,
        &def.id,
        &NewVersion {
            soul: input
                .soul
                .as_deref()
                .unwrap_or(&starter_soul(&input.display_name, &input.department)),
            config: &config,
            note: Some(&note),
            created_by: Some(&input.created_by),
        },
    )
    .await
    .map_err(|e| talaria_error::store_fault("[fleet-create] first version write failed", e))?;

    // The agent belongs to somebody from the transaction above — written in
    // this function rather than at the call sites so every create path (the
    // hire run, a personal assistant) lands with a manager and none of them
    // can forget.
    Ok(CreatedAgent { def, key_created })
}

/// May this hirer take a template's sealed config secrets into the clone?
/// Only one who may already change the template: an admin or one of its
/// managers. A hire nobody can be traced to lands admin-managed, so only
/// those who manage every agent, the template included, can reach the
/// clone, and it takes them too.
async fn takes_template_secrets(
    pg: &PgPool,
    manager: Option<&str>,
    template_id: &str,
) -> Result<bool, String> {
    let Some(user) = manager else {
        return Ok(true);
    };
    let role: Option<String> = sqlx::query_scalar("select role from users where id = $1::uuid")
        .bind(user)
        .fetch_optional(pg)
        .await
        .map_err(|e| talaria_error::store_fault("[fleet-create] hirer role read failed", e))?;
    talaria_agent_managers::manages_agent(
        pg,
        user,
        role.as_deref().unwrap_or("member"),
        template_id,
    )
    .await
    .map_err(|e| talaria_error::store_fault("[fleet-create] template managers read failed", e))
}

/// The "already exists" resume: a re-entered `create` stage finds the def the
/// previous driver already wrote and hands it back instead of erroring.
///
/// ONLY A DEF THIS HIRE WROTE. The slug is the identity a person chose, and
/// "a def with this slug exists" is also what a hire racing another manager's
/// hire sees — adopting that one would write this hire's skills and
/// department onto somebody else's agent and report success, with the hirer
/// never added as a manager. The create transaction stamps the hirer as
/// manager in the same write as the row, so "did THIS hire write it" is
/// "is the hirer among its managers, and is it managed": anything else is
/// the honest conflict. A hire with no traceable manager cannot prove
/// authorship and does not adopt either.
pub async fn create_or_resume(
    pg: &PgPool,
    input: &CreateAgentInput,
) -> Result<CreatedAgent, String> {
    match create_agent(pg, input).await {
        Ok(created) => Ok(created),
        Err(e) if e.contains("already exists") => {
            let def = agent_def_by_slug(pg, &input.slug)
                .await
                .map_err(|e| e.to_string())?;
            let Some(def) = def else {
                // The row never appeared — the exists error was about
                // something else wearing this text, so rethrow the original.
                return Err(e);
            };
            let Some(manager) = input.manager_user_id.as_deref().filter(|m| !m.is_empty()) else {
                return Err(format!(
                    "agent \"{}\" already exists and this hire has no manager to claim it by",
                    input.slug
                ));
            };
            let ours = def.managed
                && talaria_agent_managers::list_managers(pg, &def.id)
                    .await
                    .map_err(|e| e.to_string())?
                    .iter()
                    .any(|m| m.user_id == manager);
            if !ours {
                return Err(format!(
                    "agent \"{}\" already exists and belongs to someone else",
                    input.slug
                ));
            }
            Ok(CreatedAgent {
                def,
                key_created: false,
            })
        }
        Err(e) => Err(e),
    }
}

// ── The delete half ──────────────────────────────────────────────────────────

/// Remove an agent FOREVER — the row (versions + secrets cascade with it),
/// every access reference keyed by its model string, leftover containers,
/// the rendered agent dir, its fleet .env keys, and — for created agents —
/// the state volume. Imported agents keep their pre-Talaria volume (it
/// predates us; deleting it isn't ours to do). History that references the
/// agent by model string (ledger, messages, tickets) is deliberately kept.
/// Answers whether the state volume went.
pub async fn delete_agent_forever(
    pg: &sqlx::PgPool,
    sb: &talaria_secretbox::SecretBox,
    def_id: &str,
) -> Result<bool, String> {
    let def: Option<(String, String, String, bool)> = sqlx::query_as(
        "select slug, department, source, enabled from agent_defs where id = $1::uuid",
    )
    .bind(def_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    let Some((slug, department, source, enabled)) = def else {
        return Err("not found".into());
    };
    if enabled {
        return Err("retire the agent first. Delete is for retired agents only.".into());
    }

    // Another agent in this department (a pair that predates the one-agent
    // rule) means the department's container and volume are not this
    // agent's alone: the other one runs in them, or will. Deleting this one
    // removes its def and files, and leaves those to the agent still there.
    let shared = talaria_agent_defs::department_holder(pg, &department, &slug)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(other) = &shared {
        tracing::warn!(
            "[fleet] deleting {slug}: its department \"{department}\" is also {}'s, so its container and memory volume stay",
            other.holder_slug
        );
    }

    // Containers first (should already be gone after retire; both service
    // names, a legacy -b included; best-effort, and graceful — never a
    // SIGKILL).
    let slots: &[talaria_fleet_docker::Slot] = if shared.is_some() {
        &[]
    } else {
        &[talaria_fleet_docker::Slot::A, talaria_fleet_docker::Slot::B]
    };
    for slot in slots {
        let _ = talaria_fleet_docker::retire_container(&talaria_fleet_docker::slot_container(
            &department,
            *slot,
        ))
        .await;
    }

    // The def row — versions + secrets cascade with it. Access references
    // keyed by the model string are scrubbed too: per-user agent allow-lists,
    // board and channel seats, and RAG-collection bindings must not dangle on
    // a ghost. (Usage/ledger/message HISTORY keeps the model string on purpose.)
    let model = format!("{slug}-{department}");
    sqlx::query("delete from agent_defs where id = $1::uuid")
        .bind(def_id)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    for stmt in [
        "delete from user_agent_access where agent_model = $1",
        "delete from board_agents where agent_model = $1",
        "delete from channel_agents where agent_model = $1",
        "delete from rag_collection_access where principal_type = 'agent' and principal_id = $1",
    ] {
        sqlx::query(stmt)
            .bind(&model)
            .execute(pg)
            .await
            .map_err(|e| e.to_string())?;
    }

    // Rendered dir (best-effort cleanup).
    let _ = tokio::fs::remove_dir_all(talaria_fleet_layout::fleet_dir().join("agents").join(&slug))
        .await;

    // Every per-slug secret line: BOTH credentials — the Hermes key AND the
    // agent's own TALARIA_AGENT_KEY_<SLUG>. Leaving the latter behind would
    // keep a deleted agent's plaintext credential on disk forever, and —
    // because the agent_keys row cascaded away with the def — recreating the
    // slug would inherit a dead secret (the container 401s with no
    // diagnostic). The renderer now rewrites rather than skips, so this is
    // belt and braces; the lingering plaintext is the reason it matters.
    // Best-effort: the DB is authoritative; the next render re-materializes.
    strip_agent_env_lines(&slug).await;

    // State volume: only for created agents (imported volumes are external
    // legacy).
    let removed_volume = source == "created"
        && shared.is_none()
        && talaria_fleet_docker::remove_volume(&format!(
            "{}_hermes-{department}",
            talaria_fleet_layout::fleet_project()
        ))
        .await;

    match RENDER_FLEET.get() {
        Some(rf) => rf(pg.clone(), sb.clone()).await?,
        None => return Err("fleet renderer is not wired".into()),
    }
    Ok(removed_volume)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restamp_replaces_whole_words_in_every_string_value() {
        let config: Value = serde_json::json!({
            "main": {"endpoint": "ollama", "model": "sam"},
            "aliases": [{"name": "deep", "model": "sam-v2"}],
            "raw": {
                "headers": {"X-Agent-Name": "sam"},
                "hooks": ["outline_org_gate.py sam", "sammy-stays"],
                "nested": {"args": ["--as", "sam"]}
            }
        });
        let out = restamp_slug(&config, "sam", "sloane");
        assert_eq!(out["main"]["model"], serde_json::json!("sloane"));
        assert_eq!(
            out["aliases"][0]["model"],
            serde_json::json!("sloane-v2"),
            "a substring hit inside a model id is still re-stamped — the re-stamp is a plain substring replace, and \\b matches at the hyphen"
        );
        assert_eq!(
            out["raw"]["headers"]["X-Agent-Name"],
            serde_json::json!("sloane")
        );
        assert_eq!(
            out["raw"]["hooks"][0],
            serde_json::json!("outline_org_gate.py sloane")
        );
        assert_eq!(
            out["raw"]["hooks"][1],
            serde_json::json!("sammy-stays"),
            "no boundary, no replace"
        );
        assert_eq!(out["raw"]["nested"]["args"][1], serde_json::json!("sloane"));
        // keys never touched — only values walk
        assert!(out.get("raw").is_some());
    }

    #[test]
    fn the_starter_soul_speaks_the_role_in_spaces() {
        let soul = starter_soul("Sloane", "customer-success");
        assert!(soul.starts_with("# Sloane — customer success\n"));
        assert!(soul.contains("You are Sloane, the customer success agent."));
        assert!(soul.contains("- Keep humans in the loop"));
        assert!(soul.contains("- Never do dev work in chat"));
    }

    /// What a refused hire (and a delete) takes back out of the fleet .env:
    /// that slug's two credential lines and the comment the hire seeded —
    /// never a neighbour's, not even one whose slug it prefixes.
    #[test]
    fn a_slugs_env_lines_leave_with_their_comment_and_nothing_else_does() {
        let env = "\
LLM_API_KEY=abc
# added by Talaria (agent create)
HERMES_KEY_AVA=aaa
# added by Talaria (agent create)
HERMES_KEY_BEA=bbb
HERMES_KEY_BEAU=ccc
TALARIA_AGENT_KEY_BEA=tak_b
TALARIA_AGENT_KEY_BEAU=tak_bu
";
        assert_eq!(
            without_agent_env_lines(env, "bea"),
            "\
LLM_API_KEY=abc
# added by Talaria (agent create)
HERMES_KEY_AVA=aaa
HERMES_KEY_BEAU=ccc
TALARIA_AGENT_KEY_BEAU=tak_bu
"
        );
        // A slug with no lines leaves the file as it was.
        assert_eq!(without_agent_env_lines(env, "zed"), env);
    }
}
