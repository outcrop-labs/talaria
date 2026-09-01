// Workbench runtime profiles — the role-agnostic sandbox methodology. A
// profile is a chassis overlay (image + env + mounts + the harnesses it
// preinstalls) plus autoAttach fit rules; 'dev' ships seeded, and designer /
// data / marketing workbenches ride the exact same table later.
//
// The per-agent control is ONE setting on agent_defs:
//   workbench: 'off' | 'auto' | 'on'   (+ optional explicit profile slug)
// 'auto' attaches when a profile's fit rules match the agent (department /
// role); 'on' forces the explicit profile (else best fit, else 'dev').
//
// Port of ui/src/server/workbench.ts: mount safety, the seeded 'dev'
// profile, listProfiles, resolveWorkbench (the read plane the fleet render
// resolves through), and updateProfile + the wire the /api/workbench routes
// own. setAgentWorkbench / setAgentWorkbenchTuning stay TS with the
// agent-settings surface that owns them.

use serde_json::{Map, Value};
use sqlx::PgPool;
use std::path::{Path, PathBuf};

use crate::fleet_layout;

#[derive(Debug, Clone)]
pub struct WorkbenchProfile {
    pub slug: String,
    pub name: String,
    pub description: String,
    /// Container image override; '' = keep the chassis image.
    pub image: String,
    pub env: Map<String, Value>,
    /// Compose volume strings ("name-or-path:/dest[:ro]").
    pub mounts: Vec<String>,
    /// Harness slugs this profile preinstalls.
    pub harnesses: Vec<String>,
    pub auto_attach: AutoAttach,
    /// The column as stored, for the wire: postgres normalizes jsonb object
    /// keys (shorter first, then bytewise) on write, and TS stringifies the
    /// row's object verbatim — so `{"roles":…,"departments":…}` is the byte
    /// order the SPA sees. The typed struct above is for matching, not wire.
    pub auto_attach_wire: Value,
    /// Room for the later phases: creds scoping, toolkit verbs, effort map.
    pub config: Map<String, Value>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct AutoAttach {
    #[serde(default)]
    pub departments: Vec<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

// ── Mount safety ──────────────────────────────────────────────────────────────
// Profile mounts render verbatim into the fleet's compose volumes, and the
// sandbox runs as root — so a mount string is an arbitrary host-filesystem
// grant. Default-deny: sources must sit under a Talaria-owned root (the fleet
// dir, widenable by the operator) or be named volumes, and the classic escape
// hatches are refused outright regardless of root.

/// Paths that hand the container the host. The docker socket is host root by
/// another name; /proc /sys /dev are the escape hatches sitting next to it.
/// The socket's parent dirs are denied too — mounting /var/run brings it.
const DENIED_SOURCES: [&str; 5] = ["/proc", "/sys", "/dev", "/var/run", "/run"];

const NAMED_VOLUME: &str = "^[a-zA-Z0-9][a-zA-Z0-9_.-]*$";

fn mount_mode_ok(mode: &str) -> bool {
    matches!(
        mode,
        "ro" | "rw" | "z" | "Z" | "ro,z" | "rw,z" | "ro,Z" | "rw,Z"
    )
}

fn mount_roots() -> Vec<PathBuf> {
    let raw = std::env::var("TALARIA_WORKBENCH_MOUNT_ROOTS")
        .unwrap_or_else(|_| fleet_layout::fleet_dir().display().to_string());
    raw.split(',')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .filter_map(|r| std::path::absolute(r).ok())
        .collect()
}

/// Why a compose volume string is unacceptable, or None when it's fine.
pub fn mount_error(mount: &str) -> Option<String> {
    let parts: Vec<&str> = mount.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Some("expected \"source:/dest[:mode]\"".into());
    }
    let (src, dst, mode) = (parts[0], parts[1], parts.get(2).copied());
    if let Some(mode) = mode
        && !mount_mode_ok(mode)
    {
        return Some(format!("unknown mode \"{mode}\""));
    }
    if !dst.starts_with('/') || dst == "/" {
        return Some("destination must be an absolute path inside the container".into());
    }
    if src.is_empty() {
        return Some("source required".into());
    }
    let named = regex::Regex::new(NAMED_VOLUME).expect("named volume pattern");
    if !src.starts_with('/') {
        if named.is_match(src) {
            return None;
        }
        return Some("source must be an absolute host path or a named volume".into());
    }
    let Ok(path) = std::path::absolute(src) else {
        return Some(format!("source path unresolvable: {src}"));
    };
    if path == Path::new("/") {
        return Some("the host root is never mountable".into());
    }
    let denied = DENIED_SOURCES
        .iter()
        .any(|d| path == Path::new(d) || path.starts_with(format!("{d}/")));
    if denied {
        return Some(format!(
            "{} would hand the container the host",
            path.display()
        ));
    }
    let roots = mount_roots();
    let allowed = roots
        .iter()
        .any(|r| path.as_path() == r.as_path() || path.starts_with(format!("{}/", r.display())));
    if !allowed {
        return Some(format!(
            "{} is outside the allowed mount roots ({}). Widen them with TALARIA_WORKBENCH_MOUNT_ROOTS.",
            path.display(),
            roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    None
}

// ── Profiles ─────────────────────────────────────────────────────────────────

/// The shipped default — a coding workbench for dev-leaning agents. Seeded
/// once per process; admins tune it from the API afterwards (never
/// re-clobbered — the insert is on-conflict-do-nothing).
async fn ensure_seed(pg: &PgPool) -> Result<(), sqlx::Error> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SEEDED: AtomicBool = AtomicBool::new(false);
    if SEEDED.swap(true, Ordering::Relaxed) {
        return Ok(());
    }
    sqlx::query(
        "insert into workbench_profiles (slug, name, description, env, harnesses, auto_attach) \
         values ($1, $2, $3, $4, $5, $6) on conflict (slug) do nothing",
    )
    .bind("dev")
    .bind("Coding workbench")
    .bind(
        "A sandboxed development environment: coding harnesses (opencode, claude code, codex) working repo checkouts under the platform-owned git flow.",
    )
    .bind(serde_json::json!({ "TALARIA_WORKBENCH": "dev" }))
    .bind(serde_json::json!(["opencode", "claude-code", "codex", "oh-my-pi"]))
    .bind(serde_json::json!({ "departments": ["engineering"], "roles": ["engineer", "developer"] }))
    .execute(pg)
    .await?;
    Ok(())
}

/// The row shape, spelled once: (slug, name, description, image, env,
/// mounts, harnesses, auto_attach, config, enabled).
type ProfileTuple = (
    String,
    String,
    Option<String>,
    Option<String>,
    Value,
    Option<Value>,
    Value,
    Value,
    Option<Value>,
    bool,
);

fn profile_of(r: ProfileTuple) -> Result<WorkbenchProfile, String> {
    Ok(WorkbenchProfile {
        slug: r.0,
        name: r.1,
        description: r.2.unwrap_or_default(),
        image: r.3.unwrap_or_default(),
        env: serde_json::from_value(r.4).map_err(|e| format!("profile env: {e}"))?,
        mounts: serde_json::from_value(r.5.unwrap_or(Value::Array(Vec::new())))
            .map_err(|e| format!("profile mounts: {e}"))?,
        harnesses: serde_json::from_value(r.6).map_err(|e| format!("profile harnesses: {e}"))?,
        auto_attach: serde_json::from_value(r.7.clone())
            .map_err(|e| format!("profile auto_attach: {e}"))?,
        auto_attach_wire: r.7,
        config: r
            .8
            .and_then(|c| serde_json::from_value(c).ok())
            .unwrap_or_default(),
        enabled: r.9,
    })
}

pub async fn list_profiles(pg: &PgPool) -> Result<Vec<WorkbenchProfile>, String> {
    ensure_seed(pg)
        .await
        .map_err(|e| format!("workbench seed failed: {e}"))?;
    let rows: Vec<ProfileTuple> = sqlx::query_as(
        "select slug, name, description, image, env, mounts, harnesses, auto_attach, config, enabled \
         from workbench_profiles order by slug",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("workbench profiles read failed: {e}"))?;
    rows.into_iter().map(profile_of).collect()
}

/// Does a profile's autoAttach match this agent? Department exact (case
/// insensitive); role substring, because roles are free text.
pub fn fits(p: &WorkbenchProfile, department: &str, role: Option<&str>) -> bool {
    let dept = department.to_lowercase();
    if p.auto_attach
        .departments
        .iter()
        .any(|d| d.to_lowercase() == dept)
    {
        return true;
    }
    role.is_some_and(|r| {
        let r = r.to_lowercase();
        p.auto_attach
            .roles
            .iter()
            .any(|rule| r.contains(&rule.to_lowercase()))
    })
}

/// The profile as the /api/workbench routes answer it — the ROW select's
/// column order (autoAttach last-but-two, config, enabled), env values
/// verbatim. `mask_env` replaces every VALUE with '•••' in place for member
/// reads: a profile's env is injected straight into agent containers and is
/// the documented home for scoped credentials, so its values are not member-
/// readable. Keys stay so the attachment UI can still explain itself.
pub fn profile_wire(p: &WorkbenchProfile, mask_env: bool) -> Value {
    let env: Map<String, Value> = if mask_env {
        p.env
            .iter()
            .map(|(k, _)| (k.clone(), Value::String("•••".into())))
            .collect()
    } else {
        p.env.clone()
    };
    serde_json::json!({
        "slug": p.slug,
        "name": p.name,
        "description": p.description,
        "image": p.image,
        "env": env,
        "mounts": p.mounts,
        "harnesses": p.harnesses,
        "autoAttach": p.auto_attach_wire,
        "config": p.config,
        "enabled": p.enabled,
    })
}

/// The update patch — `None` keeps the column (the SQL's `coalesce(null,
/// col)`), `Some` replaces it.
#[derive(Default)]
pub struct ProfilePatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub env: Option<Map<String, Value>>,
    pub mounts: Option<Vec<String>>,
    pub harnesses: Option<Vec<String>>,
    pub auto_attach: Option<AutoAttach>,
    pub config: Option<Map<String, Value>>,
    pub enabled: Option<bool>,
}

/// updateProfile — the coalesce update, returning whether the slug existed.
pub async fn update_profile(
    pg: &PgPool,
    slug: &str,
    patch: &ProfilePatch,
) -> Result<bool, sqlx::Error> {
    let auto_attach = patch
        .auto_attach
        .as_ref()
        .map(|a| serde_json::json!({ "departments": a.departments, "roles": a.roles }));
    let row = sqlx::query_scalar::<_, String>(
        "update workbench_profiles set \
           name = coalesce($2, name), \
           description = coalesce($3, description), \
           image = coalesce($4, image), \
           enabled = coalesce($5, enabled), \
           env = coalesce($6, env), \
           mounts = coalesce($7, mounts), \
           harnesses = coalesce($8, harnesses), \
           auto_attach = coalesce($9, auto_attach), \
           config = coalesce($10, config), \
           updated_at = now() \
         where slug = $1 returning slug",
    )
    .bind(slug)
    .bind(&patch.name)
    .bind(&patch.description)
    .bind(&patch.image)
    .bind(patch.enabled)
    .bind(patch.env.as_ref().map(|e| Value::Object(e.clone())))
    .bind(patch.mounts.as_ref().map(|m| serde_json::json!(m)))
    .bind(patch.harnesses.as_ref().map(|h| serde_json::json!(h)))
    .bind(auto_attach)
    .bind(patch.config.as_ref().map(|c| Value::Object(c.clone())))
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

/// The agent-side input to [`resolve_workbench`] — the three columns off
/// agent_defs (workbench, workbench_profile) plus department/role.
pub struct WorkbenchAgent<'a> {
    pub department: &'a str,
    pub role: Option<&'a str>,
    /// 'off' | 'auto' | 'on'.
    pub workbench: &'a str,
    pub workbench_profile: Option<&'a str>,
}

/// The profile an agent actually runs with, honoring THE setting.
/// off → none · on → explicit pick, else best fit, else 'dev' · auto → fit
/// (an explicit pick wins outright — the admin chose it).
pub async fn resolve_workbench(
    pg: &PgPool,
    agent: &WorkbenchAgent<'_>,
) -> Result<Option<WorkbenchProfile>, String> {
    if agent.workbench == "off" {
        return Ok(None);
    }
    let profiles: Vec<WorkbenchProfile> = list_profiles(pg)
        .await?
        .into_iter()
        .filter(|p| p.enabled)
        .collect();
    let find = |slug: &str| profiles.iter().find(|p| p.slug == slug);
    let best_fit = || {
        profiles
            .iter()
            .find(|p| fits(p, agent.department, agent.role))
    };
    if agent.workbench == "on" {
        let picked = agent
            .workbench_profile
            .and_then(find)
            .or_else(best_fit)
            .or_else(|| find("dev"));
        return Ok(picked.cloned());
    }
    // auto: an explicit profile pick wins outright (the admin chose it); with
    // no pick, the autoAttach fit rules decide.
    if let Some(pick) = agent.workbench_profile.and_then(find) {
        return Ok(Some(pick.clone()));
    }
    Ok(best_fit().cloned())
}

/// Per-agent workbench mount switch (fleet.defs.$id PATCH). `profile` follows
/// TS's `undefined`-means-leave-it: `None` here skips the second column, so a
/// caller that only flips the switch keeps the stored profile pick.
pub async fn set_agent_workbench(
    pg: &PgPool,
    id: &str,
    workbench: &str,
    profile: Option<Option<&str>>,
) -> Result<(), sqlx::Error> {
    sqlx::query("update agent_defs set workbench = $1, updated_at = now() where id = $2::uuid")
        .bind(workbench)
        .bind(id)
        .execute(pg)
        .await?;
    if let Some(profile) = profile {
        sqlx::query(
            "update agent_defs set workbench_profile = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(profile)
        .bind(id)
        .execute(pg)
        .await?;
    }
    Ok(())
}

/// Per-agent workbench tuning: the harness pick + effort→model overrides —
/// the knobs the agent-view dropdowns write. `models` is the RAW PATCH OBJECT
/// (TS's `Partial<Record<'light'|'standard'|'heavy', string|null>>`): an
/// absent key leaves the stored override, null or '' clears it, a value sets
/// it. Only the three effort keys are read; anything else a caller smuggles
/// in is ignored.
pub async fn set_agent_workbench_tuning(
    pg: &PgPool,
    id: &str,
    harness: Option<Option<&str>>,
    models: Option<&Map<String, Value>>,
) -> Result<(), sqlx::Error> {
    if let Some(harness) = harness {
        sqlx::query(
            "update agent_defs set workbench_harness = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(harness)
        .bind(id)
        .execute(pg)
        .await?;
    }
    if let Some(models) = models {
        let cur: Option<(Option<Value>,)> =
            sqlx::query_as("select workbench_models from agent_defs where id = $1::uuid")
                .bind(id)
                .fetch_optional(pg)
                .await?;
        let mut next: Map<String, Value> = cur
            .and_then(|(m,)| m)
            .and_then(|m| m.as_object().cloned())
            .unwrap_or_default();
        for k in ["light", "standard", "heavy"] {
            let Some(v) = models.get(k) else {
                continue;
            };
            match v.as_str() {
                Some("") | None => {
                    next.remove(k);
                }
                Some(v) => {
                    next.insert(k.to_string(), Value::String(v.to_string()));
                }
            }
        }
        sqlx::query(
            "update agent_defs set workbench_models = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(Value::Object(next))
        .bind(id)
        .execute(pg)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(slug: &str, departments: &[&str], roles: &[&str]) -> WorkbenchProfile {
        WorkbenchProfile {
            slug: slug.into(),
            name: slug.into(),
            description: String::new(),
            image: String::new(),
            env: Map::new(),
            mounts: Vec::new(),
            harnesses: Vec::new(),
            auto_attach: AutoAttach {
                departments: departments.iter().map(|d| d.to_string()).collect(),
                roles: roles.iter().map(|r| r.to_string()).collect(),
            },
            auto_attach_wire: json!({}),
            config: Map::new(),
            enabled: true,
        }
    }

    #[test]
    fn mount_strings_default_deny() {
        // Named volumes and sane absolute mounts pass (mount roots default to
        // the fleet dir; the exact fleet dir path is env-dependent, so probe
        // the branches that don't depend on it).
        assert_eq!(mount_error("state:/opt/data"), None);
        assert_eq!(mount_error("state:/opt/data:ro"), None);
        assert_eq!(mount_error("state:/opt/data:rw,z"), None);
        // Shape errors.
        assert_eq!(
            mount_error("nocolon"),
            Some("expected \"source:/dest[:mode]\"".into())
        );
        assert_eq!(
            mount_error("a:b:c:d"),
            Some("expected \"source:/dest[:mode]\"".into())
        );
        assert_eq!(
            mount_error("state:/opt/data: rw"),
            Some("unknown mode \" rw\"".into())
        );
        assert_eq!(
            mount_error("state:opt"),
            Some("destination must be an absolute path inside the container".into())
        );
        assert_eq!(
            mount_error("state:/"),
            Some("destination must be an absolute path inside the container".into())
        );
        assert_eq!(mount_error(":/opt/x"), Some("source required".into()));
        // A non-absolute source must be a legal volume name.
        assert_eq!(
            mount_error("bad name:/x"),
            Some("source must be an absolute host path or a named volume".into())
        );
        assert_eq!(
            mount_error("/proc:/opt/x:ro").unwrap(),
            "/proc would hand the container the host".to_string()
        );
        // /etc is not a denied source, just outside the roots — the operator
        // may widen TALARIA_WORKBENCH_MOUNT_ROOTS past it deliberately.
        assert!(
            mount_error("/etc:/opt/x:ro")
                .unwrap()
                .contains("outside the allowed mount roots")
        );
        assert!(
            mount_error("/:/opt/x")
                .unwrap()
                .starts_with("the host root")
        );
        assert!(
            mount_error("/etc/passwd:/x")
                .unwrap()
                .contains("outside the allowed mount roots")
        );
    }

    #[test]
    fn fit_rules_match_department_exact_and_role_substring() {
        let p = profile("dev", &["engineering"], &["engineer", "developer"]);
        assert!(fits(&p, "engineering", None));
        assert!(
            fits(&p, "Engineering", None),
            "department is case-insensitive"
        );
        assert!(!fits(&p, "research", None));
        assert!(
            fits(&p, "research", Some("Senior Engineer")),
            "role substring"
        );
        assert!(
            fits(&p, "research", Some("DEVELOPER")),
            "role case-insensitive"
        );
        assert!(!fits(&p, "research", Some("writer")));
        assert!(!fits(&p, "research", None));
    }

    #[test]
    fn profile_rows_decode_the_seed_shape() {
        let row: ProfileTuple = (
            "dev".into(),
            "Coding workbench".into(),
            Some("sandbox".into()),
            None,
            json!({ "TALARIA_WORKBENCH": "dev" }),
            None,
            json!(["opencode"]),
            json!({ "departments": ["engineering"] }),
            None,
            true,
        );
        let p = profile_of(row).unwrap();
        assert_eq!(p.image, "");
        assert!(p.mounts.is_empty(), "null mounts decode to none");
        assert!(p.config.is_empty());
        assert_eq!(p.auto_attach.departments, vec!["engineering".to_string()]);
        assert!(p.auto_attach.roles.is_empty(), "missing roles default");
    }
}
