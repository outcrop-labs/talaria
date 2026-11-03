// THE CANARY — a throwaway probe of a candidate Hermes image, rendered from
// the same chassis every agent is, before any agent runs the image
// (talaria-hermes-channel drives it; docs/ARCHITECTURE.md, "The fleet").
//
// It is an agent with nothing to do and nothing to reach: the chassis
// service with the candidate image, the config an agent of this fleet
// renders (the rollout's first agent's, or a minimal one) with the
// candidate's own `_config_version` stamped, no MCP servers, no messaging
// platforms, no users, no Talaria credential, no gateway key, and a model
// base URL nothing listens on. It is asked only non-model questions — its
// health, its API server — so it spends no tokens; and it runs on the fleet
// network so the probe that asks is the gateway's own dial.
//
// Its own compose project (`<fleet project>-canary`), file and state volume:
// `down -v` takes all of it away, and nothing it does touches an agent's
// container, volume or lock.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::{
    RENDERED_HEADER, RENDERED_HEADER_TAIL, fleet_config_version, pin_write_root, read_chassis,
    yaml11_emit,
};

/// The canary's one service.
pub const CANARY_SERVICE: &str = "canary";

/// Where the canary's model calls would go: the discard port on its own
/// loopback, where nothing answers. A call that somehow left the canary
/// would fail at once, and could not reach a provider or spend.
pub const CANARY_DEAD_LLM_URL: &str = "http://127.0.0.1:9/v1";

/// The model name the canary's API server answers as.
pub const CANARY_MODEL_NAME: &str = "talaria-canary";

/// The canary's compose project.
pub fn canary_project() -> String {
    format!("{}-canary", talaria_fleet_layout::fleet_project())
}

/// Its container — named, so the probe knows what to ask.
pub fn canary_container() -> String {
    format!("{}-{CANARY_SERVICE}", canary_project())
}

/// Its directory under the fleet dir.
pub fn canary_dir() -> PathBuf {
    talaria_fleet_layout::fleet_dir().join("canary")
}

/// What [`render_canary`] wrote.
#[derive(Debug, Clone)]
pub struct CanaryFiles {
    pub compose: PathBuf,
    pub env_file: PathBuf,
    pub project: String,
    pub container: String,
    /// The canary's API server bearer, minted for this canary alone.
    pub api_key: String,
}

impl CanaryFiles {
    /// `docker compose …` arguments addressing the canary's project.
    pub fn compose_args(&self, extra: &[&str]) -> Vec<String> {
        let mut args = vec![
            "compose".to_string(),
            "-p".to_string(),
            self.project.clone(),
            "-f".to_string(),
            self.compose.to_string_lossy().into_owned(),
            "--env-file".to_string(),
            self.env_file.to_string_lossy().into_owned(),
        ];
        args.extend(extra.iter().map(|s| s.to_string()));
        args
    }
}

/// The canary's config.yaml: an agent's rendered config (`base`, its text),
/// or a minimal one, with what could reach anything taken out and the
/// candidate's own schema version stamped.
pub fn canary_config(base: Option<&str>, config_version: u64) -> String {
    let routed = canary_routed(base, config_version);
    let body = yaml11_emit(&Value::Object(routed));
    format!(
        "{RENDERED_HEADER}{CANARY_MODEL_NAME} v0{RENDERED_HEADER_TAIL}\n{}{}...\n",
        body,
        if body.ends_with('\n') { "" } else { "\n" }
    )
}

/// An agent's rendered config.yaml as data (its first YAML document), or
/// None when it does not parse to a mapping.
pub fn rendered_config(text: &str) -> Option<Map<String, Value>> {
    let first = text.split("\n...").next().unwrap_or(text);
    serde_yaml_ng::from_str::<Value>(first)
        .ok()
        .and_then(|v| v.as_object().cloned())
}

/// The canary's config as data: what [`canary_config`] writes. The channel's
/// inspection hands it to a candidate's own config migration, so the keys
/// Talaria writes are the ones checked.
pub fn canary_routed(base: Option<&str>, config_version: u64) -> Map<String, Value> {
    let mut routed: Map<String, Value> = base
        .and_then(|text| {
            // A rendered config is two YAML documents at worst (the `...`
            // end marker); the first is the config.
            let first = text.split("\n...").next().unwrap_or(text);
            serde_yaml_ng::from_str::<Value>(first).ok()
        })
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    if !routed.contains_key("model") {
        routed.insert(
            "model".into(),
            json!({
                "model": CANARY_MODEL_NAME,
                "base_url": "${LLM_BASE_URL}",
                "provider": "custom",
                "api_key": "${LLM_API_KEY}",
            }),
        );
    }
    // No tool server, no messaging platform but the API server, no
    // scheduled work: the canary answers its health and nothing else.
    routed.insert("mcp_servers".into(), json!({}));
    if let Some(Value::Object(platforms)) = routed.get_mut("platforms") {
        platforms.retain(|name, _| name == "api_server");
    }
    fleet_config_version(&mut routed, config_version);
    routed
}

/// The canary's service block, from the chassis service.
fn canary_service(
    chassis_service: &Value,
    image: &str,
    api_key: &str,
    files: &CanaryPaths,
    host_dial: bool,
) -> Result<Value, String> {
    let mut svc = chassis_service
        .as_object()
        .cloned()
        .ok_or_else(|| "chassis.yml service block is not a mapping".to_string())?;
    for k in [
        "build",
        "depends_on",
        "profiles",
        "env_file",
        "secrets",
        "ports",
        "labels",
    ] {
        svc.remove(k);
    }
    // The candidate exactly, not inside `${HERMES_IMAGE:-…}`: it is the
    // image under test whatever the operator's override says.
    svc.insert("image".into(), json!(image));
    svc.insert("container_name".into(), json!(files.container));
    svc.insert("restart".into(), json!("no"));
    svc.entry("stop_grace_period")
        .or_insert_with(|| json!(format!("{}s", talaria_fleet_docker::AGENT_STOP_GRACE_SECS)));
    let mut env: Map<String, Value> = svc
        .get("environment")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    // Nothing that names a credential or a reachable model survives.
    env.retain(|k, _| {
        !(k.contains("KEY") || k.contains("TOKEN") || k.contains("SECRET") || k.contains("PASS"))
    });
    env.insert("LLM_BASE_URL".into(), json!(CANARY_DEAD_LLM_URL));
    env.insert("LLM_API_KEY".into(), json!(""));
    env.insert("API_SERVER_KEY".into(), json!(api_key));
    env.insert("API_SERVER_MODEL_NAME".into(), json!(CANARY_MODEL_NAME));
    pin_write_root(&mut env);
    svc.insert("environment".into(), Value::Object(env));
    svc.insert(
        "volumes".into(),
        json!([
            "canary-data:/opt/data",
            format!("{}:/opt/data/config.yaml:ro", files.config.display()),
            format!("{}:/opt/data/SOUL.md:ro", files.soul.display()),
            format!(
                "{}:/opt/skills:ro",
                talaria_fleet_layout::fleet_dir().join("skills").display()
            ),
            format!(
                "{}:/opt/data/plugins/talaria-events:ro",
                talaria_fleet_layout::fleet_dir()
                    .join("plugins")
                    .join("talaria-events")
                    .display()
            ),
        ]),
    );
    svc.insert("labels".into(), json!({ "talaria.canary": "1" }));
    if host_dial {
        // An ephemeral loopback port: the probe reads which with `docker port`.
        svc.insert("ports".into(), json!(["127.0.0.1::8642"]));
    }
    svc.insert("networks".into(), json!(["fleet"]));
    Ok(Value::Object(svc))
}

struct CanaryPaths {
    config: PathBuf,
    soul: PathBuf,
    container: String,
}

/// Write the canary's compose file, env file, config and soul for `image`
/// at its own `config_version`, the config shaped from `base` (an agent's
/// rendered config.yaml text) when there is one.
pub async fn render_canary(
    image: &str,
    config_version: u64,
    base: Option<&str>,
) -> Result<CanaryFiles, String> {
    let chassis = read_chassis().await?;
    let dir = canary_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let paths = CanaryPaths {
        config: dir.join("config.yaml"),
        soul: dir.join("SOUL.md"),
        container: canary_container(),
    };
    tokio::fs::write(&paths.config, canary_config(base, config_version))
        .await
        .map_err(|e| format!("{}: {e}", paths.config.display()))?;
    tokio::fs::write(
        &paths.soul,
        "You are Talaria's canary: a throwaway probe of a Hermes image. You take no turns.\n",
    )
    .await
    .map_err(|e| format!("{}: {e}", paths.soul.display()))?;
    let api_key = format!("canary-{}", uuid_hex());
    let svc = canary_service(
        &chassis.service,
        image,
        &api_key,
        &paths,
        !talaria_fleet_layout::dial_is_container(),
    )?;
    let network = chassis
        .network
        .as_ref()
        .map(|n| n.name.clone())
        .unwrap_or_else(|| "talaria".into());
    let compose = json!({
        "name": canary_project(),
        "services": { CANARY_SERVICE: svc },
        "volumes": { "canary-data": {} },
        "networks": { "fleet": { "external": true, "name": network } },
    });
    let text =
        serde_yaml_ng::to_string(&compose).map_err(|e| format!("canary compose emit: {e}"))?;
    let compose_path = dir.join("docker-compose.yml");
    // 0600: it carries the canary's own bearer.
    talaria_fleet_brain::write_atomic(
        &compose_path,
        format!("# Generated by Talaria — the Hermes image canary. Do not hand-edit.\n{text}")
            .as_bytes(),
        0o600,
    )
    .await
    .map_err(|e| format!("{}: {e}", compose_path.display()))?;
    // Its own, empty env file: the fleet's (every agent's credential) is
    // never read for the canary.
    let env_file = dir.join(".env");
    talaria_fleet_brain::write_atomic(&env_file, b"", 0o600)
        .await
        .map_err(|e| format!("{}: {e}", env_file.display()))?;
    Ok(CanaryFiles {
        compose: compose_path,
        env_file,
        project: canary_project(),
        container: paths.container,
        api_key,
    })
}

/// 32 random hex digits, from the OS's randomness by way of the std hasher
/// seed (no new dependency for one bearer).
fn uuid_hex() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canary_config_keeps_the_agents_shape_and_drops_its_reach() {
        let base = "# Rendered by Talaria — ada-eng v3. Do not hand-edit; edit in Talaria.\n\
            model:\n  model: m1\n  base_url: ${LLM_BASE_URL}\n  provider: custom\n  api_key: ${LLM_API_KEY}\n\
            mcp_servers:\n  talaria:\n    url: http://x/api/mcp/gw/talaria\n    headers:\n      X-Api-Key: ${TALARIA_AGENT_KEY}\n\
            platforms:\n  api_server:\n    extra:\n      model_routes: {}\n  telegram:\n    token: ${CONFIG_TG}\n\
            agent:\n  disabled_toolsets:\n  - terminal\n_config_version: 51\n...\n";
        let text = canary_config(Some(base), 52);
        let back: Value = serde_yaml_ng::from_str(text.split("\n...").next().unwrap()).unwrap();
        assert_eq!(
            back["_config_version"],
            json!(52),
            "the candidate's own version"
        );
        assert_eq!(back["mcp_servers"], json!({}), "no tool server");
        assert!(back["platforms"].get("telegram").is_none(), "no channel");
        assert!(back["platforms"].get("api_server").is_some());
        assert_eq!(back["model"]["model"], json!("m1"), "the agent's own shape");
        assert_eq!(back["agent"]["disabled_toolsets"], json!(["terminal"]));
        assert!(!text.contains("TALARIA_AGENT_KEY") && !text.contains("CONFIG_TG"));
        assert!(text.ends_with("...\n"));

        // No agent to borrow from: a minimal config that still boots.
        let minimal = canary_config(None, 51);
        let back: Value = serde_yaml_ng::from_str(minimal.split("\n...").next().unwrap()).unwrap();
        assert_eq!(back["_config_version"], json!(51));
        assert_eq!(back["model"]["provider"], json!("custom"));
    }

    #[test]
    fn the_canary_service_holds_no_credential_and_no_reachable_model() {
        let chassis: Value = serde_yaml_ng::from_str(
            "image: ${HERMES_IMAGE:-x}\nports: ['1:2']\nlabels: {a: b}\nenv_file: [x]\n\
             environment:\n  LLM_BASE_URL: ${LLM_BASE_URL:-http://h/api/llm/v1}\n  LLM_API_KEY: ${LLM_API_KEY:-}\n\
             \x20 OPERATOR_TOKEN: ${OPERATOR_TOKEN}\n  API_SERVER_ENABLED: 'true'\n\
             healthcheck: {test: [CMD-SHELL, 'curl -sf http://localhost:8642/health || exit 1']}\n",
        )
        .unwrap();
        let paths = CanaryPaths {
            config: PathBuf::from("/f/canary/config.yaml"),
            soul: PathBuf::from("/f/canary/SOUL.md"),
            container: "p-canary-canary".into(),
        };
        let svc = canary_service(&chassis, "repo@sha256:abc", "canary-k", &paths, true).unwrap();
        assert_eq!(svc["image"], json!("repo@sha256:abc"));
        assert_eq!(svc["container_name"], json!("p-canary-canary"));
        assert_eq!(svc["restart"], json!("no"));
        let env = &svc["environment"];
        assert_eq!(env["LLM_BASE_URL"], json!(CANARY_DEAD_LLM_URL));
        assert_eq!(env["LLM_API_KEY"], json!(""));
        assert_eq!(env["API_SERVER_KEY"], json!("canary-k"));
        assert!(env.get("OPERATOR_TOKEN").is_none());
        assert_eq!(env["API_SERVER_ENABLED"], json!("true"));
        assert!(svc.get("env_file").is_none());
        assert_eq!(svc["ports"], json!(["127.0.0.1::8642"]));
        assert_eq!(svc["labels"], json!({ "talaria.canary": "1" }));
        assert!(
            svc["healthcheck"].is_object(),
            "the chassis's own health check"
        );
        let vols = svc["volumes"].as_array().unwrap();
        assert!(vols.iter().any(|v| v == "canary-data:/opt/data"));
        assert!(
            !vols.iter().any(|v| v.as_str().unwrap().contains("hermes-")),
            "never an agent's state volume"
        );
        // Container dial: no published port.
        let svc = canary_service(&chassis, "i", "k", &paths, false).unwrap();
        assert!(svc.get("ports").is_none());
    }
}
