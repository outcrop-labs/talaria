// renderFleet — regenerate everything the managed fleet runs on from the
// database: each managed agent's config.yaml + SOUL.md + git credential helper
// + gitconfig + secrets.env, the shared skills root, and the one compose file
// + manifest that describe the whole fleet. Idempotent, re-run on any change;
// Hermes re-reads config on mtime, so a render lands in running agents without
// a restart. Port of ui/src/server/fleet-render.ts.
//
// THIS MODULE carries the DATA PLANE (the managed-agent roster, the stable
// loopback port assignment, the chassis parse) and the KEY PLANE (the 0600
// fleet .env writers, the shared + per-agent credential seeding, and the
// shared-skill seeding with its pristine/adopt/never-clobber tree). The render
// loop and the compose/manifest emit land next (they drag secrets,
// mcp-registry, guardrails, org).
//
// THE CHASSIS PARSE carries one documented YAML resolution divergence. TS
// parses chassis.yml in YAML 1.1 — `mode: 0400` reads as OCTAL 256 — then
// emits compose in 1.2, where 256 lands as the decimal literal `256`. This
// parse (serde_yaml_ng, 1.2 resolution) keeps `0400` a STRING; the emitter
// writes those bytes back; docker's own 1.1 reader re-reads them as octal
// 256 — the same mode TS produces, via a different byte spelling. What this
// does NOT carry is the 1.1 bool set (`on`/`off`/`yes`/`no` stay strings
// here, became booleans in TS): a chassis that spelled a compose boolean as
// `on` would render differently, and no compose idiom does.

use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agent_auth::ensure_agent_api_key;
use crate::fleet_layout::{self, GATEWAY_PORT_BASE};
use crate::secretbox::SecretBox;
use crate::workbench_harnesses::{HarnessAuth, list_harness_defs};

/// The def columns the render loop reads — the agent_defs row for
/// MANAGED+ENABLED agents (the current version rides alongside in
/// [`RenderTarget`]).
#[derive(Debug, Clone)]
pub struct RenderDef {
    pub id: String,
    pub slug: String,
    pub department: String,
    /// The persona id (`<slug>-<department>`) — what grants, manifests, and
    /// gateway headers address.
    pub model: String,
    pub display_name: String,
    pub role: Option<String>,
    pub source: String,
    pub active_slot: Option<String>,
    pub workbench: Option<String>,
    pub workbench_profile: Option<String>,
    pub workbench_harness: Option<String>,
}

/// The version's render-facing columns (the full version shape lives in
/// agent_defs; the loop reads soul + config).
#[derive(Debug, Clone)]
pub struct RenderVersion {
    pub version: i32,
    pub soul: String,
    pub config: Value,
}

#[derive(Debug, Clone)]
pub struct RenderTarget {
    pub def: RenderDef,
    pub version: RenderVersion,
}

/// The managed_agents row shape, spelled once.
type TargetTuple = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i32,
    String,
    Value,
);

fn target_of(r: TargetTuple) -> RenderTarget {
    RenderTarget {
        def: RenderDef {
            id: r.0,
            slug: r.1,
            department: r.2,
            model: r.3,
            display_name: r.4,
            role: r.5,
            source: r.6,
            active_slot: r.7,
            workbench: r.8,
            workbench_profile: r.9,
            workbench_harness: r.10,
        },
        version: RenderVersion {
            version: r.11,
            soul: r.12,
            config: r.13,
        },
    }
}

/// Every managed+enabled agent with its current version, slug-ordered — the
/// render's whole world. Agents without a current version are absent (the
/// join drops them): a def with no version cannot be rendered.
pub async fn managed_agents(pg: &PgPool) -> Result<Vec<RenderTarget>, sqlx::Error> {
    // Literal SQL: one auditable statement, columns spelled once.
    let rows: Vec<TargetTuple> = sqlx::query_as(
        "select d.id::text, d.slug, d.department, d.model, d.display_name, d.role, d.source, \
         d.active_slot, d.workbench, d.workbench_profile, d.workbench_harness, \
         v.version, v.soul, v.config \
         from agent_defs d \
         join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
         where d.managed and d.enabled \
         order by d.slug",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(target_of).collect())
}

/// Assign each managed agent a stable loopback port (persisted, never reused),
/// so the rendered compose and the manifest agree across membership changes.
/// New ports continue past the highest ever assigned — including ports held by
/// agents that no longer render (disabled/unmanaged), which is why the
/// existing-port scan is NOT scoped to the roster.
pub async fn ensure_gateway_ports(
    pg: &PgPool,
    slugs: &[String],
) -> Result<HashMap<String, i64>, sqlx::Error> {
    let existing: Vec<(String, i64)> = sqlx::query_as(
        "select slug, gateway_port::int8 from agent_defs where gateway_port is not null",
    )
    .fetch_all(pg)
    .await?;
    let mut map: HashMap<String, i64> = existing.into_iter().collect();
    let mut next = map
        .values()
        .max()
        .map(|m| m + 1)
        .unwrap_or(GATEWAY_PORT_BASE);
    for slug in slugs {
        if map.contains_key(slug) {
            continue;
        }
        let port = next;
        next += 1;
        sqlx::query("update agent_defs set gateway_port = $1::int4 where slug = $2")
            .bind(port as i32)
            .bind(slug)
            .execute(pg)
            .await?;
        map.insert(slug.clone(), port);
    }
    Ok(map)
}

/// The next unclaimed loopback port for an incoming slot.
pub async fn next_free_port(pg: &PgPool) -> Result<i64, sqlx::Error> {
    let row: (i64,) =
        sqlx::query_as("select coalesce(max(gateway_port), $1)::int8 as m from agent_defs")
            .bind((GATEWAY_PORT_BASE - 1) as i32)
            .fetch_one(pg)
            .await?;
    Ok(row.0.max(GATEWAY_PORT_BASE - 1) + 1)
}

/// Per-agent additions beyond the uniform chassis, keyed by slug.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChassisExtras {
    #[serde(default)]
    pub environment: Option<Map<String, Value>>,
    #[serde(default)]
    pub volumes: Option<Vec<String>>,
    #[serde(default)]
    pub secrets: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkDef {
    pub name: String,
}

/// The chassis every agent renders from: one service block + per-slug extras.
/// Talaria-owned (extracted once at cutover from the legacy stack). Everything
/// but the service block is optional.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Chassis {
    /// The uniform service block — cloned per agent, then tailored.
    pub service: Value,
    /// Per-agent additions beyond the uniform chassis, keyed by slug.
    #[serde(default)]
    pub extras: HashMap<String, ChassisExtras>,
    /// Shared named-volume definitions (workspaces, repos, kanban).
    #[serde(default)]
    pub volumes: Map<String, Value>,
    /// Secret definitions the extras may reference.
    #[serde(default)]
    pub secrets: Map<String, Value>,
    /// External docker network the fleet joins (fresh installs use their own;
    /// this machine's legacy default is ai_default).
    #[serde(default)]
    pub network: Option<NetworkDef>,
}

/// Parse the chassis YAML into the render's shapes. Pure — the file read (and
/// its two operator-facing errors) lives in [`read_chassis`]. Merge keys
/// (`<<:`) resolve here, matching TS's `parseYaml(text, { merge: true })`.
pub fn parse_chassis(text: &str) -> Result<Chassis, String> {
    let mut yaml: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|e| format!("chassis.yml is not YAML: {e}"))?;
    yaml.apply_merge()
        .map_err(|e| format!("chassis.yml has an unusable merge key: {e}"))?;
    let json: Value = serde_json::to_value(yaml)
        .map_err(|e| format!("chassis.yml carries a shape JSON cannot hold: {e}"))?;
    let Some(obj) = json.as_object() else {
        return Err("chassis.yml is not a mapping".into());
    };
    if obj.get("service").map(Value::is_object) != Some(true) {
        return Err("chassis.yml has no \"service\" block".into());
    }
    serde_json::from_value(json).map_err(|e| format!("chassis.yml has an unreadable block: {e}"))
}

/// Read + parse the fleet chassis. The two failures are the operator's to fix:
/// a missing file and a file with no service block both stop the render (there
/// is no default chassis — the harness cannot render agents without one).
pub async fn read_chassis() -> Result<Chassis, String> {
    let path = fleet_layout::chassis_file();
    let text = tokio::fs::read_to_string(&path).await.map_err(|_| {
        format!(
            "fleet chassis missing at {} — the harness cannot render agents without it",
            path.display()
        )
    })?;
    parse_chassis(&text)
}

// ── The fleet .env — the key plane ───────────────────────────────────────────
// fleet/.env holds every agent's PLAINTEXT credential, so it is written and
// kept at 0600 inside a 0700 dir — same rule as agent-secrets' secrets.env.
// Anything else means any local account (or any workbench agent with a shell)
// can impersonate the whole fleet, and re-rendering wouldn't fix it.

/// Best-effort lock-down: file 0600, parent dir 0700. The `.catch(() => {})`
/// of the TS chmods — a filesystem that refuses chmod still gets the render.
async fn lock_down_fleet_env(env_path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = tokio::fs::metadata(env_path).await {
        let mut p = meta.permissions();
        p.set_mode(0o600);
        let _ = tokio::fs::set_permissions(env_path, p).await;
    }
    if let Some(dir) = env_path.parent()
        && let Ok(meta) = tokio::fs::metadata(dir).await
    {
        let mut p = meta.permissions();
        p.set_mode(0o700);
        let _ = tokio::fs::set_permissions(dir, p).await;
    }
}

/// Write the fleet .env at 0600 (create-mode applies only to new files, so the
/// chmod after is what fixes an existing world-readable one), parent 0700.
async fn write_fleet_env(env_path: &Path, content: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(env_path)
        .await
        .map_err(|e| format!("fleet .env unwritable ({}): {e}", env_path.display()))?;
    file.write_all(content.as_bytes())
        .await
        .map_err(|e| format!("fleet .env write failed ({}): {e}", env_path.display()))?;
    lock_down_fleet_env(env_path).await;
    Ok(())
}

/// Replace the KEY= line if present — the replacement rides a closure, not the
/// regex template engine, because a secret is never a `$`-pattern (TS spells
/// the same care as `.replace(re, () => line)`). None means "append instead".
fn replace_env_line(content: &str, name: &str, line: &str) -> Option<String> {
    let re =
        regex::Regex::new(&format!(r"(?m)^{}=.*$", regex::escape(name))).expect("env line pattern");
    if !re.is_match(content) {
        return None;
    }
    Some(
        re.replace(content, |_: &regex::Captures| line.to_string())
            .into_owned(),
    )
}

/// The `\n?$ → \n` discipline every append shares: strip ONE trailing newline
/// if present, then exactly one joins the old content to what follows.
fn with_trailing_newline(content: &str) -> &str {
    content.strip_suffix('\n').unwrap_or(content)
}

/// Seed the SHARED keys into the fleet .env: TALARIA_AGENT_KEY (the app's own
/// hop to the toolkit service) plus any provider key a native-auth harness
/// references, provisioned from the server env when present. Absent keys stay
/// absent — the doctor/auth surfaces tell the agent. Shared keys append ONCE
/// (presence is enough); per-agent keys are rewritten from the DB every render
/// ([`ensure_agent_env_keys`]).
pub async fn ensure_fleet_env_key(pg: &PgPool) -> Result<(), String> {
    let env_path = fleet_layout::fleet_env();
    let current = tokio::fs::read_to_string(&env_path)
        .await
        .unwrap_or_default();
    // Even with nothing to append, an existing world-readable file must be
    // locked down — installs that rendered before this change are the ones at
    // risk.
    lock_down_fleet_env(&env_path).await;
    let mut append: Vec<String> = Vec::new();
    let has_line = |name: &str| {
        regex::Regex::new(&format!(r"(?m)^{}=", regex::escape(name)))
            .expect("env line pattern")
            .is_match(&current)
    };
    let mut need = |name: &str, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.is_empty())
            && !has_line(name)
        {
            append.push(format!("{name}={v}"));
        }
    };
    need("TALARIA_AGENT_KEY", std::env::var("TALARIA_AGENT_KEY").ok());
    // Native-auth harness keys: any provider key a registry harness references
    // must reach compose interpolation. The TS block is try/catch — a registry
    // or DB that can't answer skips this provisioning without failing the
    // render (the TALARIA_AGENT_KEY seeding above still holds).
    let registry = async {
        let eps: Vec<(String, String)> = sqlx::query_as(
            "select provider, api_key_env from llm_endpoints where api_key_env is not null",
        )
        .fetch_all(pg)
        .await?;
        let defs = list_harness_defs(pg).await?;
        Ok::<_, sqlx::Error>((eps, defs))
    }
    .await;
    if let Ok((eps, defs)) = registry {
        for h in &defs {
            let HarnessAuth::Provider { provider, .. } = &h.def.auth else {
                continue;
            };
            if let Some((_, key_env)) = eps.iter().find(|(p, _)| p == provider) {
                need(key_env, std::env::var(key_env).ok());
            }
        }
    }
    if append.is_empty() {
        return Ok(());
    }
    write_fleet_env(
        &env_path,
        &format!(
            "{}\n{}\n",
            with_trailing_newline(&current),
            append.join("\n")
        ),
    )
    .await
}

/// Materialize every managed agent's credential into the fleet .env, minting
/// on first render (agent-auth owns the secret; the DB is the source of truth,
/// so a line lost here comes back identical rather than rotating a running
/// container out of its own identity). Same shape as HERMES_KEY_<SLUG>, the
/// other per-agent secret compose interpolates.
///
/// The DB wins over whatever the file says: a line is REWRITTEN, never skipped
/// because it exists. Skipping on presence silently bricks an agent whose slug
/// was reused after a delete (stale line, no agent_keys row → the container
/// presents a dead secret and gets an undiagnosable 401), and the same happens
/// after a DB restore against a preserved .env.
pub async fn ensure_agent_env_keys(
    pg: &PgPool,
    sb: &SecretBox,
    targets: &[RenderTarget],
) -> Result<(), String> {
    let env_path = fleet_layout::fleet_env();
    let current = tokio::fs::read_to_string(&env_path)
        .await
        .unwrap_or_default();
    let mut next = current.clone();
    let mut append: Vec<String> = Vec::new();
    for t in targets {
        let name = fleet_layout::agent_key_var(&t.def.slug);
        let secret = ensure_agent_api_key(pg, sb, &t.def.id).await?;
        let line = format!("{name}={secret}");
        match replace_env_line(&next, &name, &line) {
            Some(rewritten) => next = rewritten,
            None => append.push(line),
        }
    }
    if !append.is_empty() {
        next = format!(
            "{}\n# per-agent credentials — TalarIA-owned\n{}\n",
            with_trailing_newline(&next),
            append.join("\n")
        );
    }
    if next == current {
        lock_down_fleet_env(&env_path).await;
        return Ok(());
    }
    write_fleet_env(&env_path, &next).await
}

// ── Seed skills ──────────────────────────────────────────────────────────────
// Repo-shipped fleet skills (scripts/skills/*) seed into the fleet's shared
// skills root on render. Pristine copies (byte-identical to what was seeded,
// tracked in .seeds.json) follow canonical updates; a copy the admin edited
// via the skills UI is never clobbered. fleet/ itself is gitignored; this is
// how canonical skills like talaria-toolkit reach every install.

/// Content hash of a skill dir: sorted relative paths + file bytes, dotfiles
/// excluded. (TS sorts names with localeCompare; skill dir names are ASCII,
/// where that and byte order agree.)
async fn skill_dir_hash(root: &Path) -> Result<String, String> {
    async fn walk(dir: &Path, rel: &str, h: &mut Sha256) -> Result<(), String> {
        let mut names: Vec<String> = Vec::new();
        let mut entries = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        while let Some(e) = entries
            .next_entry()
            .await
            .map_err(|e| format!("{}: {e}", dir.display()))?
        {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        for name in names {
            if name.starts_with('.') {
                continue;
            }
            let p = dir.join(&name);
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let meta = tokio::fs::metadata(&p)
                .await
                .map_err(|e| format!("{}: {e}", p.display()))?;
            if meta.is_dir() {
                Box::pin(walk(&p, &r, h)).await?;
            } else if meta.is_file() {
                let bytes = tokio::fs::read(&p)
                    .await
                    .map_err(|e| format!("{}: {e}", p.display()))?;
                h.update(r.as_bytes());
                h.update(b"\0");
                h.update(&bytes);
            }
        }
        Ok(())
    }
    let mut h = Sha256::new();
    walk(root, "", &mut h).await?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Recursive dir copy (fs.cp recursive — file modes ride tokio's copy).
async fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut entries = tokio::fs::read_dir(src)
        .await
        .map_err(|e| format!("{}: {e}", src.display()))?;
    while let Some(e) = entries
        .next_entry()
        .await
        .map_err(|e| format!("{}: {e}", src.display()))?
    {
        let from = e.path();
        let to = dest.join(e.file_name());
        if e.file_type()
            .await
            .map_err(|e| format!("{}: {e}", from.display()))?
            .is_dir()
        {
            Box::pin(copy_dir_recursive(&from, &to)).await?;
        } else {
            tokio::fs::copy(&from, &to)
                .await
                .map_err(|e| format!("{} → {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// One seed through the decision tree. The caller logs a failure and moves on
/// — one broken skill dir never blocks the rest of the render.
async fn seed_one(
    name: &str,
    src: &Path,
    target: &Path,
    manifest: &mut Map<String, Value>,
) -> Result<bool, String> {
    let src_hash = skill_dir_hash(src).await?;
    if tokio::fs::metadata(target).await.is_err() {
        copy_dir_recursive(src, target).await?;
        manifest.insert(name.into(), Value::String(src_hash));
        return Ok(true);
    }
    if manifest.get(name).and_then(Value::as_str) == Some(src_hash.as_str()) {
        return Ok(false); // already carrying this seed
    }
    let target_hash = skill_dir_hash(target).await?;
    if target_hash == src_hash {
        // In sync (e.g. pre-manifest install that never diverged) — adopt.
        manifest.insert(name.into(), Value::String(src_hash));
        Ok(true)
    } else if manifest.get(name).and_then(Value::as_str) == Some(target_hash.as_str()) {
        // Pristine copy of an older seed — carry the canonical update forward.
        match tokio::fs::remove_dir_all(target).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", target.display())),
        }
        copy_dir_recursive(src, target).await?;
        manifest.insert(name.into(), Value::String(src_hash));
        Ok(true)
    } else if !manifest.contains_key(name) {
        // Pre-manifest install that differs from today's seed: admin edit or
        // stale canonical — can't tell, so never clobber. Update via skills UI.
        tracing::warn!(
            "[fleet] skill {name} predates seed tracking and differs from canonical — left as-is"
        );
        Ok(false)
    } else {
        // Admin-edited — theirs wins, silently.
        Ok(false)
    }
}

/// Seed the repo-shipped shared skills (see the section header). Purely
/// best-effort at the top: a missing seeds dir is a repo layout with nothing
/// to seed, not a failure.
pub async fn seed_shared_skills() -> Result<(), String> {
    let dest = fleet_layout::fleet_dir().join("skills");
    tokio::fs::create_dir_all(&dest)
        .await
        .map_err(|e| format!("{}: {e}", dest.display()))?;
    let manifest_path = dest.join(".seeds.json");
    let manifest: Map<String, Value> = tokio::fs::read_to_string(&manifest_path)
        .await
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut manifest = manifest;
    let mut dirty = false;
    let seeds_dir = fleet_layout::seed_skills_dir();
    let Ok(mut entries) = tokio::fs::read_dir(&seeds_dir).await else {
        return Ok(());
    };
    let mut names: Vec<String> = Vec::new();
    while let Ok(Some(e)) = entries.next_entry().await {
        if e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    for name in names {
        let src: PathBuf = seeds_dir.join(&name);
        let target = dest.join(&name);
        match seed_one(&name, &src, &target, &mut manifest).await {
            Ok(true) => dirty = true,
            Ok(false) => {}
            Err(e) => tracing::error!("[fleet] seeding skill {name} failed: {e}"),
        }
    }
    if dirty
        && let Err(e) = tokio::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )
        .await
    {
        tracing::error!(
            "[fleet] seeds manifest write failed ({}): {e}",
            manifest_path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CHASSIS: &str = "\
service:
  image: hermes:latest
  environment:
    MODE: 0400
  volumes:
    - shared-hooks:/opt/hooks:ro
extras:
  analyst:
    environment:
      EXTRA_TOOL: on
    volumes:
      - workspace:/opt/ws
    secrets:
      - gh
volumes:
  shared-hooks: {}
secrets:
  gh:
    file: ./gh.txt
network:
  name: talaria
";

    #[test]
    fn the_chassis_parses_with_extras_volumes_secrets_and_network() {
        let c = parse_chassis(CHASSIS).unwrap();
        assert_eq!(c.service["image"], json!("hermes:latest"));
        assert_eq!(
            c.service["environment"]["MODE"],
            json!("0400"),
            "1.2 resolution keeps the octal-looking mode a string; see the module header"
        );
        let ex = c.extras.get("analyst").unwrap();
        assert_eq!(ex.environment.as_ref().unwrap()["EXTRA_TOOL"], json!("on"));
        assert_eq!(
            ex.volumes.as_deref().unwrap(),
            &["workspace:/opt/ws".to_string()]
        );
        assert_eq!(ex.secrets.as_ref().unwrap()[0], json!("gh"));
        assert!(c.volumes.contains_key("shared-hooks"));
        assert!(c.secrets.contains_key("gh"));
        assert_eq!(c.network.as_ref().unwrap().name, "talaria");
    }

    #[test]
    fn merge_keys_resolve_like_ts_merge_true() {
        let merged = "\
base: &base
  image: hermes:latest
  environment:
    A: '1'
service:
  <<: *base
  environment:
    B: '2'
";
        let c = parse_chassis(merged).unwrap();
        assert_eq!(
            c.service["image"],
            json!("hermes:latest"),
            "inherited via <<"
        );
        // serde's apply_merge replaces the key wholesale (last wins), same as
        // the yaml package's merge resolution for a whole-key override.
        assert_eq!(c.service["environment"]["B"], json!("2"));
    }

    #[test]
    fn a_chassis_without_a_service_block_is_the_operator_error() {
        assert_eq!(
            parse_chassis("volumes:\n  x: {}\n").unwrap_err(),
            "chassis.yml has no \"service\" block"
        );
        assert!(parse_chassis("not: [a, mapping").is_err());
        assert!(parse_chassis("- just\n- a\n- list\n").is_err());
    }

    #[test]
    fn an_empty_chassis_still_parses_to_defaults() {
        let c = parse_chassis("service:\n  image: x\n").unwrap();
        assert!(c.extras.is_empty());
        assert!(c.network.is_none());
    }

    #[test]
    fn env_line_rewrites_are_never_a_regex_pattern() {
        // A secret containing template metacharacters must land verbatim —
        // this is why the replacement rides a closure.
        let out = replace_env_line(
            "A=1\nTALARIA_AGENT_KEY_ANALYST=tak_old\n",
            "TALARIA_AGENT_KEY_ANALYST",
            "tak_$1$&${x}",
        )
        .unwrap();
        assert_eq!(out, "A=1\ntak_$1$&${x}\n");
        assert_eq!(
            replace_env_line("A=1\n", "TALARIA_AGENT_KEY_ANALYST", "tak_x"),
            None,
            "absent means append"
        );
        // A prefix of another variable is not that variable.
        assert_eq!(replace_env_line("KEY_X=1\n", "KEY", "v"), None);
    }

    #[test]
    fn the_append_newline_discipline_strips_exactly_one() {
        assert_eq!(with_trailing_newline(""), "");
        assert_eq!(with_trailing_newline("A=1"), "A=1");
        assert_eq!(with_trailing_newline("A=1\n"), "A=1");
        assert_eq!(with_trailing_newline("A=1\n\n"), "A=1\n");
        // The joined shape both .env append sites produce.
        assert_eq!(
            format!("{}\nK=v\n", with_trailing_newline("A=1\n\n")),
            "A=1\n\nK=v\n"
        );
        assert_eq!(format!("{}\nK=v\n", with_trailing_newline("")), "\nK=v\n");
    }

    fn temp_skill_root(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("talaria-skill-tests-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[tokio::test]
    async fn skill_hashes_cover_sorted_paths_and_bytes_and_skip_dotfiles() {
        let root = temp_skill_root("hash");
        write_file(&root, "SKILL.md", b"hello");
        write_file(&root, "sub/deep.md", b"world");
        write_file(&root, ".seeds.json", b"ignored");
        write_file(&root, "sub/.hidden", b"ignored");
        let h1 = skill_dir_hash(&root).await.unwrap();
        // Content change → different hash.
        write_file(&root, "SKILL.md", b"hello!");
        let h2 = skill_dir_hash(&root).await.unwrap();
        assert_ne!(h1, h2);
        // Dotfiles never counted: restoring them to anything is invisible.
        write_file(&root, ".seeds.json", b"other");
        let h3 = skill_dir_hash(&root).await.unwrap();
        assert_eq!(h2, h3);
        // And the hash is the sha256 the TS dir hash computes: path, NUL, bytes.
        let mut expect = Sha256::new();
        expect.update(b"SKILL.md");
        expect.update(b"\0");
        expect.update(b"hello!");
        expect.update(b"sub/deep.md");
        expect.update(b"\0");
        expect.update(b"world");
        let hex: String = expect
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(h3, hex);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn seed_one_pristine_adopt_never_clobber_and_first_copy() {
        let root = temp_skill_root("seed");
        let src = root.join("canonical");
        write_file(&src, "SKILL.md", b"v2");
        let target = root.join("toolkit");
        let mut manifest = Map::new();

        // First render: target absent → copy + record the SRC hash.
        let canonical = skill_dir_hash(&src).await.unwrap();
        assert!(
            seed_one("toolkit", &src, &target, &mut manifest)
                .await
                .unwrap()
        );
        assert_eq!(manifest["toolkit"], json!(canonical));
        assert_eq!(skill_dir_hash(&target).await.unwrap(), canonical);

        // Same seed again: no change, no dirty.
        assert!(
            !seed_one("toolkit", &src, &target, &mut manifest)
                .await
                .unwrap()
        );

        // Canonical moves to v3; the copy is PRISTINE (hash == recorded) → the
        // update carries forward.
        write_file(&src, "SKILL.md", b"v3");
        assert!(
            seed_one("toolkit", &src, &target, &mut manifest)
                .await
                .unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(target.join("SKILL.md")).unwrap(),
            "v3"
        );

        // The copy is now edited by an admin (hash ≠ recorded) → never clobber.
        write_file(&target, "SKILL.md", b"admin's own");
        let before = skill_dir_hash(&target).await.unwrap();
        assert!(
            !seed_one("toolkit", &src, &target, &mut manifest)
                .await
                .unwrap()
        );
        assert_eq!(skill_dir_hash(&target).await.unwrap(), before);

        // Pre-manifest install that differs: also left as-is (warn, no touch).
        let mut no_manifest = Map::new();
        assert!(
            !seed_one("toolkit", &src, &target, &mut no_manifest)
                .await
                .unwrap()
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
