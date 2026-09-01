// renderFleet — regenerate everything the managed fleet runs on from the
// database: each managed agent's config.yaml + SOUL.md + git credential helper
// + gitconfig + secrets.env, the shared skills root, and the one compose file
// + manifest that describe the whole fleet. Idempotent, re-run on any change;
// Hermes re-reads config on mtime, so a render lands in running agents without
// a restart. Port of ui/src/server/fleet-render.ts.
//
// THIS MODULE carries the whole render: the DATA PLANE (the managed-agent
// roster, the stable loopback port assignment, the chassis parse), the KEY
// PLANE (the 0600 fleet .env writers, the shared + per-agent credential
// seeding, and the shared-skill seeding with its pristine/adopt/never-clobber
// tree), and THE LOOP itself — per-agent config.yaml/SOUL.md/git helper, the
// workbench overlay, the MCP pass-through, the compose emit, and the manifest.
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
//
// config.yaml — the agent-facing half — is the OTHER direction: TS EMITS it
// with a YAML-1.1 writer because Hermes reads it with PyYAML 1.1, so the
// hand-rolled emitter below quotes everything a 1.1 reader would re-resolve
// (see the emitter's own header). The compose file is emitted with
// serde_yaml_ng (1.2), matching TS's default `stringifyYaml`.

use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agent_auth::ensure_agent_api_key;
use crate::fleet_layout::{self, GATEWAY_PORT_BASE};
use crate::secretbox::SecretBox;
use crate::workbench_harnesses::{HarnessAuth, McpConfigFormat, list_harness_defs};

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

// ── config.yaml emission — YAML 1.1 aware ─────────────────────────────────────
//
// Hermes re-reads config.yaml with PyYAML, which resolves PLAIN scalars under
// YAML 1.1 rules: `on`/`yes`/`off`/`no`/`y`/`n` are booleans, `~`/`null` are
// nulls, `0400`/`1_000`/`0x1a` are ints, `1:30` is sexagesimal. serde_yaml_ng
// emits YAML 1.2 — where none of those resolve specially, so it leaves such
// strings UNQUOTED and PyYAML would silently change their types on read. TS
// emits with `stringifyYaml(routed, { version: '1.1' })`; this is that, by
// hand: a JSON-tree walker that quotes every scalar a 1.1 reader would
// resolve to something other than the string it is.
//
// (Block-seq indentation and scalar-quote-style choices are the emitter's
// own; byte-identity with the TS file is not the contract — re-read equality
// under a 1.1 parser is, and the round-trip test pins it.)

fn yaml11_bool_like(s: &str) -> bool {
    matches!(
        s,
        "y" | "Y"
            | "yes"
            | "Yes"
            | "YES"
            | "n"
            | "N"
            | "no"
            | "No"
            | "NO"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    )
}

fn yaml11_null_like(s: &str) -> bool {
    matches!(s, "~" | "null" | "Null" | "NULL")
}

/// 1.1 integers: decimal (with `_` separators — `0400` is octal under 1.1,
/// still an int), plus the 0x/0o/0b radix forms.
fn yaml11_int_like(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    let (radix, digits) = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, h)
    } else if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        (8, o)
    } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        (2, b)
    } else {
        (10, t)
    };
    !digits.is_empty() && digits.chars().all(|c| c == '_' || c.is_digit(radix))
}

/// 1.1 floats: `.inf`/`.nan` specials plus mantissa[.frac][e±exp] with `_`
/// separators allowed, and a bare digit run (already an int — still a number).
fn yaml11_float_like(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    if matches!(t, ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let (mantissa, exp) = match t.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (t, None),
    };
    if let Some(e) = exp {
        let e = e.strip_prefix(['-', '+']).unwrap_or(e);
        if e.is_empty() || !e.chars().all(|c| c.is_ascii_digit() || c == '_') {
            return false;
        }
    }
    let parts: Vec<&str> = mantissa.split('.').collect();
    if parts.len() > 2 {
        return false;
    }
    let digits: String = parts.concat();
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '_')
}

/// 1.1 sexagesimal (`190:20:30`) — later groups are at most two digits, which
/// is also what keeps URLs (`http://h:8642/x` — non-digit groups) plain.
fn yaml11_sexagesimal(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    let parts: Vec<&str> = t.split(':').collect();
    parts.len() > 1
        && parts.iter().enumerate().all(|(i, p)| {
            !p.is_empty()
                && p.chars().all(|c| c.is_ascii_digit() || c == '_')
                && (i == 0 || p.len() <= 2)
        })
}

/// Can this string stay a PLAIN scalar — will every YAML 1.1 reader resolve it
/// back to the same string?
fn yaml11_plain_ok(s: &str) -> bool {
    if s.is_empty()
        || yaml11_bool_like(s)
        || yaml11_null_like(s)
        || yaml11_int_like(s)
        || yaml11_float_like(s)
        || yaml11_sexagesimal(s)
        || s == "="
        || s == "---"
        || s == "..."
    {
        return false;
    }
    if s.starts_with(' ') || s.ends_with(' ') {
        return false;
    }
    for (i, c) in s.char_indices() {
        if i == 0
            && matches!(
                c,
                ',' | '['
                    | ']'
                    | '{'
                    | '}'
                    | '#'
                    | '&'
                    | '*'
                    | '!'
                    | '|'
                    | '>'
                    | '\''
                    | '"'
                    | '%'
                    | '@'
                    | '`'
            )
        {
            return false;
        }
        match c {
            // ": " mid-string, or a trailing ':' — both end a plain scalar
            ':' if s[i + 1..].starts_with(' ') || i == s.len() - 1 => return false,
            // " #" starts a comment
            '#' if s[..i].ends_with(' ') => return false,
            // a leading "- " / "? " reads as a block indicator
            '-' | '?' if i == 0 && (s.len() == 1 || s[1..].starts_with(' ')) => return false,
            c if c.is_control() => return false,
            _ => {}
        }
    }
    true
}

/// Quote a non-plain string. Single quotes with `''` escaping is the readable
/// form; control characters go through serde_json (whose escape set — `\"`
/// `\\` `\n` `\t` `\u00XX` — is exactly YAML's double-quoted subset).
fn yaml11_quote(s: &str) -> String {
    if s.chars().any(|c| c.is_control()) {
        serde_json::to_string(s).unwrap_or_else(|_| format!("'{}'", s.replace('\'', "''")))
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

fn yaml11_key(k: &str) -> String {
    if yaml11_plain_ok(k) {
        k.to_string()
    } else {
        yaml11_quote(k)
    }
}

/// A scalar's rendered text, or None when the value is a collection.
fn yaml11_scalar(v: &Value) -> Option<String> {
    match v {
        Value::Null => Some("null".into()),
        Value::Bool(b) => Some(if *b { "true".into() } else { "false".into() }),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(if yaml11_plain_ok(s) {
            s.clone()
        } else {
            yaml11_quote(s)
        }),
        _ => None,
    }
}

fn yaml11_entry(k: &str, val: &Value, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    let key = yaml11_key(k);
    if let Some(s) = yaml11_scalar(val) {
        out.push_str(&format!("{pad}{key}: {s}\n"));
    } else if val.as_object().is_some_and(Map::is_empty) {
        out.push_str(&format!("{pad}{key}: {{}}\n"));
    } else if val.as_array().is_some_and(Vec::is_empty) {
        out.push_str(&format!("{pad}{key}: []\n"));
    } else if val.as_object().is_some() {
        out.push_str(&format!("{pad}{key}:\n"));
        for (k2, v2) in val.as_object().expect("just checked") {
            yaml11_entry(k2, v2, indent + 1, out);
        }
    } else {
        out.push_str(&format!("{pad}{key}:\n"));
        yaml11_seq(val.as_array().expect("just checked"), indent + 1, out);
    }
}

fn yaml11_seq(items: &[Value], indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    for item in items {
        if let Some(s) = yaml11_scalar(item) {
            out.push_str(&format!("{pad}- {s}\n"));
        } else if item.as_object().is_some_and(Map::is_empty) {
            out.push_str(&format!("{pad}- {{}}\n"));
        } else if item.as_array().is_some_and(Vec::is_empty) {
            out.push_str(&format!("{pad}- []\n"));
        } else if item.as_array().is_some() {
            // seq-in-seq has no block spelling that starts on the dash line
            // without chaining; JSON flow is valid YAML and always correct.
            out.push_str(&format!(
                "{pad}- {}\n",
                serde_json::to_string(item).unwrap_or_default()
            ));
        } else if let Some(m) = item.as_object() {
            // The map's first key rides the `- ` line (the dash eats two
            // columns); its later entries align under it at indent+1. A
            // COLLECTION under that first key continues one level deeper still,
            // so it clears the dash rather than aligning with it.
            let mut entries = m.iter();
            let Some((k1, v1)) = entries.next() else {
                continue;
            };
            let key1 = yaml11_key(k1);
            match yaml11_scalar(v1) {
                Some(s1) => out.push_str(&format!("{pad}- {key1}: {s1}\n")),
                None if v1.as_object().is_some_and(Map::is_empty) => {
                    out.push_str(&format!("{pad}- {key1}: {{}}\n"))
                }
                None if v1.as_array().is_some_and(Vec::is_empty) => {
                    out.push_str(&format!("{pad}- {key1}: []\n"))
                }
                None => {
                    out.push_str(&format!("{pad}- {key1}:\n"));
                    match v1 {
                        Value::Object(m2) => {
                            for (k2, v2) in m2 {
                                yaml11_entry(k2, v2, indent + 2, out);
                            }
                        }
                        Value::Array(a2) => yaml11_seq(a2, indent + 2, out),
                        _ => unreachable!("guarded by the None arms above"),
                    }
                }
            }
            for (k2, v2) in entries {
                yaml11_entry(k2, v2, indent + 1, out);
            }
        }
    }
}

/// Emit a config tree as YAML a 1.1 reader resolves identically. The root is
/// always a mapping (a scalar root renders as one bare line).
pub fn yaml11_emit(v: &Value) -> String {
    let mut out = String::new();
    if let Some(m) = v.as_object().filter(|m| !m.is_empty()) {
        for (k, val) in m {
            yaml11_entry(k, val, 0, &mut out);
        }
    } else if let Some(s) = yaml11_scalar(v) {
        out.push_str(&s);
        out.push('\n');
    }
    out
}

// ── The rendered soul ─────────────────────────────────────────────────────────

/// SOUL.md: the standing headers, the coaching block, the secret-handle
/// briefing, then the stored soul. `[soulHeader, coaching, handles]` filtered
/// to non-empty and joined with blank lines — an agent with no coaching and no
/// grants gets `header\n\nsoul`, nothing else.
fn soul_md(soul_header: &str, coaching: &str, handles: &str, soul: &str) -> String {
    let parts = [soul_header, coaching, handles]
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    format!("{}\n\n{soul}", parts.join("\n\n"))
}

// ── The git credential helper ─────────────────────────────────────────────────
//
// WHERE A HANDLE CANNOT REACH: handles substitute at the MCP gateway, which is
// every tool call an agent makes through Talaria — but NOT the shell inside a
// workbench sandbox, where a coding harness runs `git push` with its own bash
// tool and we are not in the path. Git's credential protocol closes it: git
// hands the helper protocol/host/path on stdin and reads username/password
// back; this one asks Talaria over the agent's OWN key. The value never enters
// the model's context, never lands on disk — `git push` just works.
//
// PORTABLE ACROSS HARNESS IMAGES, and that is not a nicety: the first version
// used `curl` unconditionally; `alpine/git` has no curl, so the helper exited
// 0 having printed nothing — which git reports as `could not read Username`,
// indistinguishable from "no credential exists". It falls back to wget (two
// wget flavors exist — BusyBox takes --post-data, GNU takes --body-data with
// --method=POST) and says so on stderr if neither is present. Silence is the
// one answer this script is not allowed to give. Parsed with sed, not jq, for
// the same reason.
const GIT_CREDENTIAL_HELPER: &str = concat!(
    "#!/bin/sh\n",
    "# Written by Talaria (fleet-render). Answers `get`; `store` and `erase`\n",
    "# are no-ops because we ARE the store, and forgetting is done by revoking\n",
    "# the grant rather than by anything git says.\n",
    "[ \"$1\" = \"get\" ] || exit 0\n",
    "host=\"\"; proto=\"\"; path=\"\"\n",
    "while IFS=\"=\" read -r k v; do\n",
    "  [ \"$k\" = \"host\" ] && host=\"$v\"\n",
    "  [ \"$k\" = \"protocol\" ] && proto=\"$v\"\n",
    "  # PATH IS WHAT SCOPES A GITHUB ANSWER to one repo. Git only sends it\n",
    "  # when credential.useHttpPath is set, which the rendered gitconfig does.\n",
    "  [ \"$k\" = \"path\" ] && path=\"$v\"\n",
    "done\n",
    "[ -n \"$host\" ] || exit 0\n",
    "url=\"$TALARIA_API_URL/api/secrets/git-credential\"\n",
    "body=\"{\\\"host\\\":\\\"$host\\\",\\\"protocol\\\":\\\"$proto\\\",\\\"path\\\":\\\"$path\\\"}\"\n",
    "if command -v curl >/dev/null 2>&1; then\n",
    "  resp=$(curl -sS --fail -X POST \"$url\" \\\n",
    "    -H \"X-Agent-Name: $API_SERVER_MODEL_NAME\" -H \"X-Api-Key: $API_SERVER_KEY\" \\\n",
    "    -H \"content-type: application/json\" -d \"$body\" 2>/dev/null) || exit 0\n",
    "elif command -v wget >/dev/null 2>&1; then\n",
    "  # TWO WGETS EXIST and they disagree. BusyBox (every alpine-derived\n",
    "  # harness image) takes --post-data; GNU wget takes --body-data with\n",
    "  # --method=POST and rejects the other. Trying BusyBox first and falling\n",
    "  # through costs one failed call on GNU and works on both, which beats\n",
    "  # guessing the image.\n",
    "  resp=$(wget -qO- --post-data=\"$body\" \\\n",
    "    --header=\"X-Agent-Name: $API_SERVER_MODEL_NAME\" --header=\"X-Api-Key: $API_SERVER_KEY\" \\\n",
    "    --header=\"content-type: application/json\" \"$url\" 2>/dev/null)\n",
    "  [ -n \"$resp\" ] || resp=$(wget -qO- --method=POST --body-data=\"$body\" \\\n",
    "    --header=\"X-Agent-Name: $API_SERVER_MODEL_NAME\" --header=\"X-Api-Key: $API_SERVER_KEY\" \\\n",
    "    --header=\"content-type: application/json\" \"$url\" 2>/dev/null) || exit 0\n",
    "else\n",
    "  echo \"talaria: no curl or wget in this image — cannot fetch a credential for $host\" >&2\n",
    "  exit 0\n",
    "fi\n",
    "# Parsed with sed rather than jq for the same reason: jq is not guaranteed\n",
    "# in every harness image, and a helper that fails because a tool is missing\n",
    "# looks exactly like a credential that does not exist.\n",
    "u=$(printf %s \"$resp\" | sed -n 's/.*\"username\":\"\\([^\"]*\\)\".*/\\1/p')\n",
    "p=$(printf %s \"$resp\" | sed -n 's/.*\"password\":\"\\([^\"]*\\)\".*/\\1/p')\n",
    "[ -n \"$p\" ] || exit 0\n",
    "echo \"username=$u\"\n",
    "echo \"password=$p\"\n",
    "\n",
);

/// SYSTEM-WIDE gitconfig, not the agent's ~/.gitconfig: a workbench job runs
/// the harness as whatever user that image uses, and a helper only the root
/// home knows about is a helper that silently does not run. `useHttpPath` is
/// what makes git send the repo along with the host — without it a GitHub
/// answer could only be scoped to github.com, i.e. every repo the
/// installation can reach.
const GITCONFIG: &str = "[credential]\n\thelper = talaria\n\tuseHttpPath = true\n";

/// Best-effort executable bit for the credential helper. (TS's writeFile mode
/// only stamps newly created files; this also repairs a helper that lost its
/// bit — a non-executable helper IS a silent no-op, the exact failure the
/// portability block above exists to prevent.)
async fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o755);
    let _ = tokio::fs::set_permissions(path, perms).await;
}

/// TALARIA_AGENT_DIAL=container (docker/compose.yml deployments): the app runs
/// ON the fleet network and dials compose service names, so loopback port
/// publishing is skipped entirely — every install sharing one docker host
/// allocates identical ports from the same seed data and would collide.
fn dial_is_container() -> bool {
    matches!(std::env::var("TALARIA_AGENT_DIAL"), Ok(v) if v == "container")
}

// ── The render loop ───────────────────────────────────────────────────────────

pub struct RenderResult {
    pub agents: Vec<String>,
    pub files: Vec<String>,
    pub warnings: Vec<String>,
}

/// During a roll: additionally render this agent's INCOMING slot alongside its
/// active one, on the given port. The manifest keeps pointing at the active
/// port — cutover is a DB update + a plain re-render after health.
pub struct RollOverlay<'a> {
    pub slug: &'a str,
    pub slot: crate::fleet_docker::Slot,
    pub port: i64,
}

/// renderFleet — regenerate everything the managed fleet runs on. See the
/// module header. The one TS step NOT here yet: `ensureMcpService()` (the
/// talaria-mcp container supervisor, mcp-service.ts) crosses with the docker
/// plane in the next batch; the render itself is complete without it.
pub async fn render_fleet(
    pg: &PgPool,
    sb: &SecretBox,
    roll: Option<RollOverlay<'_>>,
) -> Result<RenderResult, String> {
    let targets = managed_agents(pg).await.map_err(|e| e.to_string())?;
    let mut result = RenderResult {
        agents: Vec::new(),
        files: Vec::new(),
        warnings: Vec::new(),
    };

    // The fleet's default brain is Talaria's own gateway. Best-effort — never
    // blocks a render.
    let brain = match crate::fleet_brain::ensure_gateway_brain(pg).await {
        Ok(b) => Some(b),
        Err(e) => {
            result.warnings.push(format!("gateway brain: {e}"));
            None
        }
    };
    if brain.is_some_and(|b| b.managed && b.model.is_none()) {
        result.warnings.push(
            "gateway brain: no model configured yet — add an LLM endpoint on /models to give agents a brain"
                .into(),
        );
    }

    let chassis = read_chassis().await?;

    // Every rendered soul opens with the toolkit contract (always) and the
    // organization context (when configured).
    let org_header = crate::org::org_soul_header(&crate::org::org_profile(pg).await);
    let soul_header = [
        org_header.as_deref(),
        Some(crate::org::voice_soul_header().as_str()),
        Some(crate::org::toolkit_soul_header().as_str()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n\n");
    let coach_on = crate::guard_coaching::coach_enabled(pg).await;

    // Agents' configs point at the toolkit MCP — make sure it's actually up,
    // and that the compose env can interpolate each agent's key into the
    // header.
    crate::mcp_service::ensure_mcp_service();
    ensure_fleet_env_key(pg).await?;
    ensure_agent_env_keys(pg, sb, &targets).await?;
    seed_shared_skills().await?;

    // Credential-migration visibility — a render is the operator's moment.
    if let Ok(legacy) = crate::agent_auth::legacy_migration_status(pg).await
        && let Some(w) = crate::agent_auth::legacy_migration_warning(&legacy)
    {
        result.warnings.push(w.clone());
        if legacy.window_open {
            tracing::warn!("[fleet] {w}");
        } else {
            tracing::error!("[fleet] {w}");
        }
    }

    let gw_models = crate::fleet_brain::gateway_model_set(pg).await;
    let mut remapped: Vec<String> = Vec::new();
    let ports = ensure_gateway_ports(
        pg,
        &targets
            .iter()
            .map(|t| t.def.slug.clone())
            .collect::<Vec<_>>(),
    )
    .await
    .map_err(|e| e.to_string())?;

    // Read once, ahead of the loop: the harness registry and the endpoint env
    // contract are the same rows for every agent (TS re-reads per agent; the
    // data is read-only for the render's duration).
    let harness_registry = list_harness_defs(pg)
        .await
        .map_err(|e| format!("harness registry: {e}"))?;
    let endpoints: Vec<(String, String)> = sqlx::query_as(
        "select provider, api_key_env from llm_endpoints where api_key_env is not null",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;

    let gw_base = fleet_layout::mcp_gw_base();
    let fleet_skills = fleet_layout::fleet_dir().join("skills");

    let mut services: Map<String, Value> = Map::new();
    let mut secrets: Map<String, Value> = Map::new();
    let mut volumes: Map<String, Value> = chassis.volumes.clone();

    for target in &targets {
        let (def, version) = (&target.def, &target.version);
        let agent_dir = fleet_layout::fleet_dir().join("agents").join(&def.slug);
        tokio::fs::create_dir_all(&agent_dir)
            .await
            .map_err(|e| format!("{}: {e}", agent_dir.display()))?;
        tokio::fs::create_dir_all(agent_dir.join("skills"))
            .await
            .map_err(|e| format!("{}: {e}", agent_dir.join("skills").display()))?;

        // Heal docker-made junk: when a container (re)starts while a
        // bind-mount source is missing, docker resurrects the source as a
        // DIRECTORY — which would make the writes below EISDIR forever after.
        for f in ["config.yaml", "SOUL.md"] {
            let p = agent_dir.join(f);
            if tokio::fs::metadata(&p)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                let _ = tokio::fs::remove_dir_all(&p).await;
            }
        }

        // Un-interweave: all model tiers point at Talaria's gateway.
        let raw = def_raw_config(&version.config);
        let routed = crate::fleet_brain::route_config_through_gateway(&raw, &gw_models, |m| {
            if !remapped.contains(&m) {
                remapped.push(m);
            }
        });
        let mut routed = match routed {
            Value::Object(m) => m,
            _ => Map::new(),
        };

        // (Cloned out and inserted back on the same key — position preserved —
        // matching TS's `{...existing, talaria}` spread semantics exactly.)
        let mut mcp_servers = routed
            .get("mcp_servers")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        let agent_servers = crate::mcp_registry::servers_for_agent(pg, &def.model)
            .await
            .map_err(|e| e.to_string())?;

        // The Talaria toolkit MCP, through the MCP gateway — the org's
        // per-agent/per-person tool subsets apply to every call.
        mcp_servers.insert(
            "talaria".into(),
            json!({
                "url": format!("{gw_base}/talaria"),
                "headers": { "X-Agent-Name": def.model, "X-Api-Key": "${TALARIA_AGENT_KEY}" },
            }),
        );

        let wb_agent = crate::workbench::WorkbenchAgent {
            department: &def.department,
            role: def.role.as_deref(),
            workbench: def.workbench.as_deref().unwrap_or("auto"),
            workbench_profile: def.workbench_profile.as_deref(),
        };
        let wb = crate::workbench::resolve_workbench(pg, &wb_agent)
            .await
            .unwrap_or(None);

        // The agent's CHOSEN coding harness, as an MCP server on its own
        // config (stdio, in-sandbox).
        if let Some(wb) = &wb {
            let pick = def
                .workbench_harness
                .as_deref()
                .filter(|p| wb.harnesses.iter().any(|h| h == p))
                .or_else(|| wb.harnesses.first().map(|s| s.as_str()));
            if let Some(pick) = pick
                && let Some(h) = harness_registry.iter().find(|r| r.def.slug == pick)
                && let Some(serve) = &h.def.mcp_serve
            {
                mcp_servers.insert(
                    h.def.slug.clone(),
                    json!({ "command": serve.command, "args": serve.args }),
                );
            }
        }

        // Org-registry MCP servers ride in as GATEWAY URLs — the agent never
        // sees an upstream address or credential.
        for srv in &agent_servers {
            let mut entry = json!({
                "url": format!("{gw_base}/{}", srv.name),
                "headers": { "X-Agent-Name": def.model, "X-Api-Key": "${TALARIA_AGENT_KEY}" },
            });
            if let Some(t) = srv.timeout_secs.filter(|t| *t != 0) {
                entry["timeout"] = json!(t);
            }
            mcp_servers.insert(srv.name.clone(), entry);
        }
        routed.insert("mcp_servers".into(), Value::Object(mcp_servers));

        // Hermes only discovers skills outside ~/.hermes/skills via
        // skills.external_dirs.
        let skills_cfg = routed
            .get("skills")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut ext_dirs: Vec<String> = skills_cfg
            .get("external_dirs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        for d in ["/opt/skills", "/opt/dept-skills"] {
            if !ext_dirs.contains(&d.to_string()) {
                ext_dirs.push(d.to_string());
            }
        }
        let mut skills_out = skills_cfg.clone();
        skills_out.insert("external_dirs".into(), json!(ext_dirs));
        routed.insert("skills".into(), Value::Object(skills_out));

        let cfg_path = agent_dir.join("config.yaml");
        let cfg_text = format!(
            "# Rendered by Talaria — {} v{}. Do not hand-edit; edit in Talaria.\n{}",
            def.model,
            version.version,
            yaml11_emit(&Value::Object(routed))
        );
        tokio::fs::write(&cfg_path, cfg_text)
            .await
            .map_err(|e| format!("{}: {e}", cfg_path.display()))?;

        // The rendered soul: standing headers + coaching + handles + soul.
        let coaching = if coach_on {
            crate::guard_coaching::guard_coaching_for(pg, &def.model).await
        } else {
            String::new()
        };
        let secret_handles = crate::workspace_handles::granted_handles_for(pg, &def.model)
            .await
            .unwrap_or_default();
        let soul_path = agent_dir.join("SOUL.md");
        let soul_text = soul_md(&soul_header, &coaching, &secret_handles, &version.soul);
        tokio::fs::write(&soul_path, soul_text)
            .await
            .map_err(|e| format!("{}: {e}", soul_path.display()))?;
        result.files.push(cfg_path.display().to_string());
        result.files.push(soul_path.display().to_string());

        let helper_path = agent_dir.join("git-credential-talaria");
        tokio::fs::write(&helper_path, GIT_CREDENTIAL_HELPER)
            .await
            .map_err(|e| format!("{}: {e}", helper_path.display()))?;
        set_executable(&helper_path).await;
        let gitconfig_path = agent_dir.join("gitconfig");
        tokio::fs::write(&gitconfig_path, GITCONFIG)
            .await
            .map_err(|e| format!("{}: {e}", gitconfig_path.display()))?;
        result.files.push(helper_path.display().to_string());
        result.files.push(gitconfig_path.display().to_string());

        // The service name carries the active slot ('a' = bare, 'b' = "-b").
        let slot_b = def.active_slot.as_deref() == Some("b");
        let service_name = format!("agent-{}{}", def.department, if slot_b { "-b" } else { "" });
        let imported = def.source == "imported";
        let extras = chassis.extras.get(&def.slug);

        // Every agent is the SAME chassis — dumb agents, harness-owned settings.
        let mut svc = chassis.service.clone();
        {
            let obj = svc
                .as_object_mut()
                .ok_or_else(|| "chassis.yml service block is not a mapping".to_string())?;
            obj.remove("build");
            obj.remove("depends_on");
            obj.remove("profiles");
            if !dial_is_container() {
                let port = ports.get(&def.slug).copied().unwrap_or(0);
                obj.insert("ports".into(), json!([format!("127.0.0.1:{port}:8642")]));
            }
        }

        let mut env: Map<String, Value> = svc
            .get("environment")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if let Some(ex) = extras.and_then(|e| e.environment.as_ref()) {
            for (k, v) in ex {
                env.insert(k.clone(), v.clone());
            }
        }
        env.insert(
            "API_SERVER_KEY".into(),
            json!(format!("${{HERMES_KEY_{}}}", def.slug.to_uppercase())),
        );
        env.insert("API_SERVER_MODEL_NAME".into(), json!(def.model));
        env.insert(
            "TALARIA_AGENT_KEY".into(),
            json!(format!("${{{}}}", fleet_layout::agent_key_var(&def.slug))),
        );
        env.insert(
            "TALARIA_HEARTBEAT_SECONDS".into(),
            json!("${TALARIA_HEARTBEAT_SECONDS:-45}"),
        );

        // Workbench overlay — the agent's runtime profile. Harness state
        // PERSISTS on the department state volume (hand-offs: a session one
        // agent starts, a department-mate can resume); GitHub attribution
        // AUTHORS commits as the agent (display name + stable per-agent email).
        if let Some(wb) = &wb {
            let obj = svc
                .as_object_mut()
                .ok_or_else(|| "chassis.yml service block is not a mapping".to_string())?;
            if !wb.image.is_empty() {
                obj.insert("image".into(), json!(wb.image));
            }
            for (k, v) in &wb.env {
                env.insert(k.clone(), v.clone());
            }
            env.insert("TALARIA_WORKBENCH_PROFILE".into(), json!(wb.slug));
            env.insert(
                "CLAUDE_CONFIG_DIR".into(),
                json!("/opt/data/workbench/harness/claude"),
            );
            env.insert(
                "CODEX_HOME".into(),
                json!("/opt/data/workbench/harness/codex"),
            );
            env.insert(
                "XDG_DATA_HOME".into(),
                json!("/opt/data/workbench/harness/xdg"),
            );
            env.insert(
                "PLAYWRIGHT_BROWSERS_PATH".into(),
                json!("/opt/data/workbench/harness/playwright"),
            );
            env.insert(
                "npm_config_cache".into(),
                json!("/opt/data/workbench/harness/npm"),
            );
            let agent_label = format!("{} (Talaria agent)", def.display_name);
            let agent_email = format!("{}@agents.talaria.local", def.model);
            env.insert("GIT_AUTHOR_NAME".into(), json!(agent_label.clone()));
            env.insert("GIT_AUTHOR_EMAIL".into(), json!(agent_email.clone()));
            env.insert("GIT_COMMITTER_NAME".into(), json!(agent_label));
            env.insert("GIT_COMMITTER_EMAIL".into(), json!(agent_email));
            // Harness auth, gateway-first: OpenAI-compatible harnesses point
            // at Talaria's gateway; native harnesses get their provider's key
            // interpolated from the endpoint registry's env contract.
            for slug in &wb.harnesses {
                let Some(h) = harness_registry.iter().find(|r| r.def.slug == *slug) else {
                    continue;
                };
                for (k, v) in &h.full_env {
                    env.insert(k.clone(), v.clone());
                }
                if let HarnessAuth::Provider { provider, env_var } = &h.def.auth
                    && let Some((_, key_env)) = endpoints.iter().find(|(p, _)| p == provider)
                {
                    env.insert(env_var.clone(), json!(format!("${{{key_env}}}")));
                }
            }
            obj.insert("environment".into(), Value::Object(env));
        } else {
            svc.as_object_mut()
                .expect("checked above")
                .insert("environment".into(), Value::Object(env));
        }

        // Per-agent state volume: imported agents keep their pre-Talaria
        // volume (external, legacy-named) so their memory survives.
        let state_volume = format!("hermes-{}", def.department);
        volumes.insert(
            state_volume.clone(),
            if imported {
                json!({
                    "external": true,
                    "name": format!("{}_{}", fleet_layout::LEGACY_DOCKER_PROJECT, state_volume),
                })
            } else {
                json!({})
            },
        );

        // MCP pass-through: the agent's EXISTING grants, rendered into each
        // harness's NATIVE config format — same per-agent gateway, same
        // env-interpolated fleet key. Zero in-sandbox reconnection; grant
        // changes re-render, revocations bite at the gateway instantly.
        let wb_dir = agent_dir.join("workbench");
        if let Some(wb) = &wb {
            let mut names: Vec<String> = vec!["talaria".into()];
            for srv in &agent_servers {
                if !names.contains(&srv.name) {
                    names.push(srv.name.clone());
                }
            }
            tokio::fs::create_dir_all(&wb_dir)
                .await
                .map_err(|e| format!("{}: {e}", wb_dir.display()))?;
            let mut written: Vec<String> = Vec::new();
            for slug in &wb.harnesses {
                let Some(h) = harness_registry.iter().find(|r| r.def.slug == *slug) else {
                    continue;
                };
                let Some(mc) = &h.def.mcp_config else {
                    continue;
                };
                if written.contains(&mc.filename) {
                    continue;
                }
                // 'custom' hands rendering to the app-shipped harness's own
                // code — functions can't ride JSON, so nothing reachable from
                // this registry layer uses it (see workbench_harnesses' header).
                let body = match mc.format {
                    McpConfigFormat::Custom => None,
                    McpConfigFormat::ClaudeJson => {
                        Some(claude_mcp_config(&names, &def.model, &gw_base))
                    }
                    McpConfigFormat::OpencodeJson => {
                        Some(opencode_mcp_config(&names, &def.model, &gw_base))
                    }
                };
                let Some(body) = body else {
                    continue;
                };
                let p = wb_dir.join(&mc.filename);
                tokio::fs::write(&p, serde_json::to_string_pretty(&body).unwrap_or_default())
                    .await
                    .map_err(|e| format!("{}: {e}", p.display()))?;
                written.push(mc.filename.clone());
            }
        }

        let mut vols: Vec<String> = Vec::new();
        if wb.is_some() {
            vols.push(format!("{}:/opt/workbench-config:ro", wb_dir.display()));
        }
        if let Some(wb) = &wb {
            for m in &wb.mounts {
                vols.push(m.clone());
            }
        }
        vols.push(format!("{state_volume}:/opt/data"));
        vols.push(format!("{}:/opt/data/config.yaml:ro", cfg_path.display()));
        vols.push(format!("{}:/opt/data/SOUL.md:ro", soul_path.display()));
        vols.push(format!(
            "{}:/usr/local/bin/git-credential-talaria:ro",
            helper_path.display()
        ));
        vols.push(format!("{}:/etc/gitconfig:ro", gitconfig_path.display()));
        vols.push(format!(
            "{}:/opt/dept-skills:ro",
            agent_dir.join("skills").display()
        ));
        vols.push(format!("{}:/opt/skills:ro", fleet_skills.display()));
        let protected = [
            "/opt/data",
            "/opt/data/config.yaml",
            "/opt/data/SOUL.md",
            "/opt/dept-skills",
            "/opt/skills",
        ];
        for v in svc
            .get("volumes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        {
            let dest = v.split(':').nth(1).unwrap_or("");
            if !protected.contains(&dest) {
                vols.push(v);
            }
        }
        if let Some(exv) = extras.and_then(|e| e.volumes.as_ref()) {
            for v in exv {
                vols.push(v.clone());
            }
        }
        svc.as_object_mut()
            .expect("checked above")
            .insert("volumes".into(), json!(vols));

        // Per-agent secrets (UI-configured, DB-encrypted) materialize into the
        // agent dir and load via env_file.
        if crate::agent_secrets::materialize_agent_secrets(pg, sb, &def.id, &def.slug).await? {
            svc.as_object_mut().expect("checked above").insert(
                "env_file".into(),
                json!([agent_dir.join("secrets.env").display().to_string()]),
            );
        } else {
            svc.as_object_mut()
                .expect("checked above")
                .remove("env_file");
        }

        svc.as_object_mut()
            .expect("checked above")
            .insert("networks".into(), json!(["fleet"]));
        if let Some(secs) = extras.and_then(|e| e.secrets.as_ref()) {
            // A reference without a definition in chassis.secrets is DROPPED
            // (with a warning) — keeping it would make compose reject the
            // whole file.
            let mut kept: Vec<Value> = Vec::new();
            for s in secs {
                let name = match s {
                    Value::String(n) => Some(n.clone()),
                    Value::Object(o) => o.get("source").and_then(Value::as_str).map(str::to_string),
                    _ => None,
                };
                match name.as_ref().and_then(|n| chassis.secrets.get(n)) {
                    Some(defn) => {
                        secrets.insert(name.clone().expect("just matched"), defn.clone());
                        kept.push(s.clone());
                    }
                    None => result.warnings.push(format!(
                        "{}: secret {} not defined in chassis.yml — dropped",
                        service_name,
                        name.unwrap_or_else(|| s.to_string())
                    )),
                }
            }
            svc.as_object_mut()
                .expect("checked above")
                .insert("secrets".into(), Value::Array(kept));
        }
        services.insert(service_name.clone(), svc);

        // Mid-roll: the INCOMING slot renders alongside, identical except for
        // its fresh published port.
        if let Some(r) = &roll
            && r.slug == def.slug
        {
            let mut incoming = services.get(&service_name).cloned().unwrap_or(Value::Null);
            if !dial_is_container()
                && let Value::Object(o) = &mut incoming
            {
                o.insert(
                    "ports".into(),
                    json!([format!("127.0.0.1:{}:8642", r.port)]),
                );
            }
            let incoming_name = format!(
                "agent-{}{}",
                def.department,
                match r.slot {
                    crate::fleet_docker::Slot::B => "-b",
                    crate::fleet_docker::Slot::A => "",
                }
            );
            services.insert(incoming_name, incoming);
        }
        result.agents.push(def.model.clone());
    }

    for m in &remapped {
        result.warnings.push(format!(
            "gateway: {m} (register the provider on /models to restore this tier)"
        ));
    }

    let mut compose: Map<String, Value> = Map::new();
    compose.insert("name".into(), json!(fleet_layout::fleet_project()));
    compose.insert("services".into(), Value::Object(services));
    compose.insert("volumes".into(), Value::Object(volumes));
    if !secrets.is_empty() {
        compose.insert("secrets".into(), Value::Object(secrets));
    }
    compose.insert(
        "networks".into(),
        json!({ "fleet": { "external": true, "name": chassis.network.as_ref().map(|n| n.name.clone()).unwrap_or_else(|| "talaria".into()) } }),
    );
    let compose_yaml = serde_yaml_ng::to_string(&Value::Object(compose))
        .map_err(|e| format!("compose emit: {e}"))?;
    let compose_path = fleet_layout::fleet_dir().join("docker-compose.yml");
    tokio::fs::create_dir_all(fleet_layout::fleet_dir())
        .await
        .map_err(|e| format!("{}: {e}", fleet_layout::fleet_dir().display()))?;
    tokio::fs::write(
        &compose_path,
        format!("# Generated by Talaria — the managed fleet. Do not hand-edit.\n{compose_yaml}"),
    )
    .await
    .map_err(|e| format!("{}: {e}", compose_path.display()))?;
    result.files.push(compose_path.display().to_string());

    write_fleet_manifest(pg, &mut result).await?;
    Ok(result)
}

/// The agent's raw config, as authored (version.config.raw ?? {}).
fn def_raw_config(config: &Value) -> Value {
    config
        .get("raw")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()))
}

/// Claude Code's .mcp.json shape — `${VAR}` expands from the container env.
fn claude_mcp_config(names: &[String], model: &str, gw_base: &str) -> Value {
    json!({
        "mcpServers": names.iter().map(|n| (n.clone(), json!({
            "type": "http",
            "url": format!("{gw_base}/{n}"),
            "headers": { "X-Agent-Name": model, "X-Api-Key": "${TALARIA_AGENT_KEY}" },
        }))).collect::<Map<String, Value>>(),
    })
}

/// opencode's config — `{env:VAR}` is its env-substitution syntax.
fn opencode_mcp_config(names: &[String], model: &str, gw_base: &str) -> Value {
    json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": names.iter().map(|n| (n.clone(), json!({
            "type": "remote",
            "url": format!("{gw_base}/{n}"),
            "headers": { "X-Agent-Name": model, "X-Api-Key": "{env:TALARIA_AGENT_KEY}" },
            "enabled": true,
        }))).collect::<Map<String, Value>>(),
    })
}

/// The fleet manifest Talaria reads to reach agents directly: every enabled
/// agent's persona gateway URL (host + its published port) + HERMES key, plus
/// one entry per model tier (`<base>-<alias>`). No bridge — the app calls each
/// URL itself. Written to fleet/fleet.json.
async fn write_fleet_manifest(pg: &PgPool, result: &mut RenderResult) -> Result<(), String> {
    type DefRow = (
        String,
        String,
        String,
        Option<i64>,
        Option<String>,
        Option<Value>,
    );
    let defs: Vec<DefRow> = sqlx::query_as(
        "select d.slug, d.department, d.model, d.gateway_port::int8, d.active_slot, v.config \
         from agent_defs d \
         left join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
         where d.enabled order by d.slug",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;

    let env = tokio::fs::read_to_string(fleet_layout::fleet_env())
        .await
        .unwrap_or_default();
    let hermes_re =
        regex::Regex::new(r"^HERMES_KEY_([A-Z0-9_]+)=(.*)$").expect("hermes key pattern");
    let mut keys: HashMap<String, String> = HashMap::new();
    for line in env.lines() {
        if let Some(c) = hermes_re.captures(line.trim()) {
            keys.insert(c[1].to_lowercase(), c[2].trim().to_string());
        }
    }

    let mut manifest: Vec<Value> = Vec::new();
    for (slug, department, model, gateway_port, active_slot, config) in &defs {
        let key = keys.get(&slug.to_lowercase()).cloned().unwrap_or_default();
        if key.is_empty() {
            result.warnings.push(format!(
                "{slug}: no HERMES_KEY_{} in the fleet .env",
                slug.to_uppercase()
            ));
        }
        if gateway_port.is_none() {
            result.warnings.push(format!(
                "{slug}: no gateway port assigned yet — render again"
            ));
        }
        let url = if dial_is_container() {
            format!(
                "http://agent-{}{}:8642",
                department,
                if *active_slot == Some("b".into()) {
                    "-b"
                } else {
                    ""
                }
            )
        } else {
            format!(
                "http://{}:{}",
                fleet_layout::agent_host(),
                gateway_port.unwrap_or(0)
            )
        };
        manifest.push(json!({ "model": model, "url": url, "key": key }));
        if let Some(aliases) = config
            .as_ref()
            .and_then(|c| c.get("aliases"))
            .and_then(Value::as_array)
        {
            for a in aliases {
                if let Some(name) = a.get("name").and_then(Value::as_str) {
                    manifest.push(
                        json!({ "model": format!("{model}-{name}"), "url": url, "key": key }),
                    );
                }
            }
        }
    }
    let path = fleet_layout::fleet_dir().join("fleet.json");
    tokio::fs::write(&path, serde_json::to_string(&manifest).unwrap_or_default())
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    result.files.push(path.display().to_string());
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

    // ── the YAML 1.1 emitter ─────────────────────────────────────────────────

    #[test]
    fn yaml11_quotes_what_a_one_point_one_reader_would_reresolve() {
        // bools / nulls / numbers / specials → quoted.
        for s in [
            "on",
            "On",
            "ON",
            "off",
            "no",
            "No",
            "yes",
            "y",
            "Y",
            "n",
            "true",
            "TRUE",
            "false",
            "null",
            "Null",
            "~",
            "42",
            "-7",
            "+1",
            "1_000",
            "0400",
            "0x1a",
            "0o17",
            "0b101",
            "3.14",
            "-2.5e10",
            "1e5",
            ".inf",
            "-.inf",
            ".NaN",
            "1:30",
            "=",
            "",
            "---",
            "...",
            " leading",
            "trailing ",
            "a: b",
            "a #b",
            "ends:",
            "#frag",
            "'quoted'",
            "@at",
            "*star",
        ] {
            assert!(!yaml11_plain_ok(s), "should quote: {s:?}");
        }
        // Plain: URLs, env interpolations, model ids, dash words.
        for s in [
            "http://host.docker.internal:5273/api/mcp/gw/talaria",
            "${TALARIA_AGENT_KEY}",
            "${TALARIA_HEARTBEAT_SECONDS:-45}",
            "z-ai/glm-5.2",
            "-y",
            "a-b-c",
            "127.0.0.1:8770:8642",
            "hello world",
            "x:y",
        ] {
            assert!(yaml11_plain_ok(s), "should stay plain: {s:?}");
        }
        // Quoted strings single-quote with '' escaping.
        assert_eq!(yaml11_quote("it's on"), "'it''s on'");
    }

    #[test]
    fn yaml11_emit_round_trips_through_a_real_yaml_parser() {
        let cfg = json!({
            "models": {
                "fast": { "model": "gpt-5", "base_url": "http://gw/api/llm/v1", "api_key": "${TALARIA_AGENT_KEY}", "provider": "custom" },
                "deep": { "model": "o4", "base_url": "http://gw/api/llm/v1", "api_key": "${TALARIA_AGENT_KEY}" }
            },
            "mcp_servers": {
                "talaria": { "url": "http://h:5273/api/mcp/gw/talaria", "headers": { "X-Agent-Name": "x-eng", "X-Api-Key": "${TALARIA_AGENT_KEY}" } },
                "claude-code": { "command": "npx", "args": ["-y", "@anthropic-claude/code"], "env": { "NO_COLOR": "1" } }
            },
            "registry-srv": { "url": "http://h:5273/api/mcp/gw/srv", "timeout": 30 },
            "skills": { "external_dirs": ["/opt/skills", "/opt/dept-skills"], "dedupe": true },
            "aliases": [ { "name": "fast", "endpoint": "gpt-5" }, { "name": "deep", "endpoint": "o4" } ],
            "tricky": { "flag": "on", "mode": "0400", "count": "42", "label": "", "port": 8642, "nan": null }
        });
        let yaml = yaml11_emit(&cfg);
        // The 1.1-dangerous strings are quoted in the emitted bytes…
        for needle in ["flag: 'on'", "mode: '0400'", "count: '42'", "label: ''"] {
            assert!(yaml.contains(needle), "missing {needle:?} in:\n{yaml}");
        }
        // …and the tree parses back to EXACTLY the same JSON (a 1.2 parse of
        // the quoted strings returns the strings; PyYAML 1.1 agrees because
        // quoting is quoting).
        let back: Value = serde_yaml_ng::from_str(&yaml).expect("emitted yaml parses");
        assert_eq!(back, cfg, "round-trip must be lossless");
    }

    #[test]
    fn yaml11_emit_shapes_seq_items_and_nested_blocks() {
        let cfg = json!({
            "list": ["a", "b"],
            "maps": [ { "name": "one", "at": 1 }, { "name": "two", "at": 2 } ],
            "deep": [ { "head": { "inner": "v" }, "tail": 9 } ],
            "empty_map": {}, "empty_list": []
        });
        let y = yaml11_emit(&cfg);
        let want = "\
list:
  - a
  - b
maps:
  - name: one
    at: 1
  - name: two
    at: 2
deep:
  - head:
      inner: v
    tail: 9
empty_map: {}
empty_list: []
";
        assert_eq!(y, want);
        // …and the shape parses back identically.
        let back: Value = serde_yaml_ng::from_str(&y).unwrap();
        assert_eq!(back, cfg);
    }

    // ── the soul and the git helper ──────────────────────────────────────────

    #[test]
    fn the_soul_joins_standing_parts_over_the_stored_soul() {
        assert_eq!(soul_md("H", "C", "S", "soul"), "H\n\nC\n\nS\n\nsoul");
        // Empty coaching and handles drop out entirely.
        assert_eq!(soul_md("H", "", "", "soul"), "H\n\nsoul");
        assert_eq!(soul_md("H", "", "S", "soul"), "H\n\nS\n\nsoul");
    }

    #[tokio::test]
    async fn the_git_helper_answers_get_over_the_agent_key_and_survives_missing_tools() {
        for needle in [
            "#!/bin/sh\n",
            "[ \"$1\" = \"get\" ] || exit 0\n",
            "url=\"$TALARIA_API_URL/api/secrets/git-credential\"\n",
            "-H \"X-Api-Key: $API_SERVER_KEY\" \\\n",
            "wget -qO- --post-data=\"$body\" \\\n",
            "wget -qO- --method=POST --body-data=\"$body\" \\\n",
            "no curl or wget in this image",
            "sed -n 's/.*\"username\":\"\\([^\"]*\\)\".*/\\1/p')\n",
        ] {
            assert!(
                GIT_CREDENTIAL_HELPER.contains(needle),
                "helper lost: {needle:?}"
            );
        }
        assert!(GIT_CREDENTIAL_HELPER.ends_with('\n'));
        assert_eq!(
            GITCONFIG,
            "[credential]\n\thelper = talaria\n\tuseHttpPath = true\n"
        );
        // The helper lands executable — a non-executable helper is a silent
        // no-op, the exact failure the portability block exists to prevent.
        let dir = std::env::temp_dir().join(format!(
            "talaria-render-tests-{}-helper",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("git-credential-talaria");
        std::fs::write(&p, GIT_CREDENTIAL_HELPER).unwrap();
        set_executable(&p).await;
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "helper must be executable");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
