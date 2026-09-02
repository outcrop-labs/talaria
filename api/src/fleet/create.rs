// Create a brand-new agent — from a template (an existing agent's
// definition, model tiers/tools cloned with identity re-stamped) or, with no
// template, from the platform defaults (first local endpoint's model). The
// soul is a scaffold (or supplied, e.g. Muse-designed). A fresh gateway key
// is allocated into the fleet .env (which the renderer + manifest read).
// The delete half lives at the bottom of this file.

use serde_json::Value;
use sqlx::PgPool;

use crate::agent_defs::{
    AgentDefRow, ConfigEdits, ModelTarget, NewVersion, UpsertDef, add_version_if_changed,
    agent_def_by_id, agent_def_by_slug, apply_config_edits_over, dept_ok, list_versions, slug_ok,
    upsert_agent_def,
};
use crate::gateway::registry::list_endpoints;

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
- Prefer the local model tier for routine work; escalate deliberately.
- When unsure, ask in the channel instead of guessing.
"
    )
}

/// Ensure a HERMES_KEY_<SLUG> exists in the fleet .env; returns whether
/// created. The read is NOT best-effort — a hire against an unreadable fleet
/// env fails honestly.
pub async fn ensure_agent_key(slug: &str) -> Result<bool, String> {
    let env_path = crate::fleet::layout::fleet_env();
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
        .map_err(|e| e.to_string())?;
    if exists.is_some() {
        return Err(format!("agent \"{}\" already exists", input.slug));
    }

    // An empty-string templateId is no template (truthiness on the way in).
    let template_id = input.template_id.as_deref().filter(|t| !t.is_empty());
    let (config, note) = match template_id {
        Some(template_id) => {
            let template_version = list_versions(pg, template_id)
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .next()
                .ok_or("template agent has no versions")?;
            let from_slug = agent_def_by_id(pg, template_id)
                .await
                .map_err(|e| e.to_string())?
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
            let eps = list_endpoints(pg).await.map_err(|e| e.to_string())?;
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

    let mut def = upsert_agent_def(
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
    .map_err(|e| e.to_string())?;
    sqlx::query("update agent_defs set managed = true, updated_at = now() where id = $1::uuid")
        .bind(&def.id)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    def.managed = true;

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
    .map_err(|e| e.to_string())?;
    Ok(CreatedAgent { def, key_created })
}

/// The "already exists" resume: a re-entered `create` stage finds the def the
/// previous driver already wrote and hands it back instead of erroring. The
/// slug is the identity a person chose; two hires racing on the same slug
/// still collide honestly at the route's pre-check.
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
            match def {
                Some(def) => Ok(CreatedAgent {
                    def,
                    key_created: false,
                }),
                // The row never appeared — the exists error was about
                // something else wearing this text, so rethrow the original.
                None => Err(e),
            }
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
    sb: &crate::secretbox::SecretBox,
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

    // Containers first (should already be gone after retire; both slots,
    // best-effort).
    for slot in [crate::fleet::docker::Slot::A, crate::fleet::docker::Slot::B] {
        let _ = crate::fleet::docker::remove_container_by_name(
            &crate::fleet::docker::slot_container(&department, slot),
        )
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
        "delete from fleet_agents where name = $1",
    ] {
        sqlx::query(stmt)
            .bind(&model)
            .execute(pg)
            .await
            .map_err(|e| e.to_string())?;
    }

    // Rendered dir (best-effort cleanup).
    let _ = tokio::fs::remove_dir_all(crate::fleet::layout::fleet_dir().join("agents").join(&slug))
        .await;

    // Every per-slug secret line: BOTH credentials — the Hermes key AND the
    // agent's own TALARIA_AGENT_KEY_<SLUG>. Leaving the latter behind would
    // keep a deleted agent's plaintext credential on disk forever, and —
    // because the agent_keys row cascaded away with the def — recreating the
    // slug would inherit a dead secret (the container 401s with no
    // diagnostic). The renderer now rewrites rather than skips, so this is
    // belt and braces; the lingering plaintext is the reason it matters.
    // Best-effort: the DB is authoritative; the next render re-materializes.
    if let Ok(content) = tokio::fs::read_to_string(crate::fleet::layout::fleet_env()).await {
        let hermes = format!("HERMES_KEY_{}=", slug.to_uppercase());
        let agent_key = format!("{}=", crate::fleet::layout::agent_key_var(&slug));
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
        if next != content {
            use std::os::unix::fs::PermissionsExt;
            let _ = tokio::fs::write(crate::fleet::layout::fleet_env(), &next).await;
            if let Ok(meta) = tokio::fs::metadata(crate::fleet::layout::fleet_env()).await {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = tokio::fs::set_permissions(crate::fleet::layout::fleet_env(), perms).await;
            }
        }
    }

    // State volume: only for created agents (imported volumes are external
    // legacy).
    let removed_volume = source == "created"
        && crate::fleet::docker::remove_volume(&format!(
            "{}_hermes-{department}",
            crate::fleet::layout::fleet_project()
        ))
        .await;

    crate::fleet::render::render_fleet(pg, sb, None).await?; // compose + manifest drop the agent
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
    }
}
