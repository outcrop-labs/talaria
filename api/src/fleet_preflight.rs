// CAN AN AGENT REACH TALARIA? Asked from where the AGENT stands.
//
// THE INCIDENT THIS EXISTS FOR. Every managed agent was reported healthy — the
// container was up, `/health` answered 200, the roster was green — while not a
// single one could call a tool. Its log said so once, at startup, and nothing
// read it:
//
//     WARNING tools.mcp_tool: Failed to connect to MCP server 'talaria'
//
// The host firewall (`ufw`, default-deny INPUT) had no rule admitting the
// docker bridges to the app's port. Container→host traffic is INPUT, not
// FORWARD, so Docker's own rules said nothing about it, and a DROP (rather
// than a REJECT) turns the failure into a timeout: the agent hangs rather
// than erroring, and every tool call quietly does nothing.
//
// WHY THE EXISTING ALERT MISSED IT, which is the part worth internalising.
// `alerts.ts` probes the toolkit at `127.0.0.1:5280` and it answered perfectly,
// all day, because the APP can always reach it — the app is the thing listening.
// Reachability is not a property of a service, it is a property of a PATH, and
// the only way to test the agent's path is to stand where the agent stands.
//
// So this runs a throwaway container ON THE FLEET NETWORK and asks it. That is
// expensive — a container start, ~1s — so it is deliberately NOT on the alerts
// poll. It runs when the fleet changes (render, up) and its verdict is cached
// for the alerts panel to read. Port of ui/src/server/fleet-preflight.ts.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::time::Duration;

use crate::fleet_layout;
use crate::gateway::settings::{get_setting, set_setting};

const KEY: &str = "fleet_preflight";
/// Small, always present locally (the fleet pulls far larger images), and it has
/// a shell with a TCP-capable builtin — no python, no curl assumed.
const PROBE_IMAGE: &str = "busybox:latest";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightResult {
    pub ok: bool,
    /// The URL as an AGENT would write it, not as the app would.
    pub target: String,
    pub detail: String,
    pub at: String,
}

/// One CLI probe with execFile's contract: true only when the binary runs and
/// exits zero. A timeout and a failure are the same answer to every caller
/// here (`has`, `reaches`, `resolvesExternally` all degrade to "absent"),
/// which is why this is its own helper and not fleet_docker's `docker` —
/// that one reports output and stderr; these only ever report WHETHER.
async fn exec_ok(bin: &str, args: &[&str], timeout: Duration) -> bool {
    let run = tokio::time::timeout(timeout, async {
        tokio::process::Command::new(bin)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .await
    });
    matches!(run.await, Ok(Ok(out)) if out.status.success())
}

/// The host:port an agent is configured to reach Talaria on. Derived from
/// MCP_GW_BASE — the same origin the renderer stamps into agent config — so
/// the probe stands where the agents stand. The hardcoded host.docker.internal
/// this replaces cried wolf on containerized instances (app on the fleet
/// network, maybe behind a proxy with no published host port): every agent
/// called tools fine while the preflight reported the app unreachable.
///
/// TS keeps two identical fns (appTarget/mcpTarget) — they once probed
/// different ports; agents moved to the gateway URL on the app port and both
/// now derive from the same base. One definition here, over a pure core so
/// the derivation is testable without owning the process env.
fn agent_reach_target() -> Result<String, String> {
    target_of(&fleet_layout::mcp_gw_base())
}

fn target_of(base: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(base).map_err(|_| "Invalid URL".to_string())?;
    let port = url
        .port_or_known_default()
        .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
    Ok(format!("{}:{port}", url.host_str().unwrap_or_default()))
}

/// THE REMEDY, in the syntax of whatever this host actually runs.
///
/// Deliberately not hardcoded to ufw: this ships to any distro, and a Fedora
/// operator handed a `ufw` command reasonably concludes the diagnosis is wrong.
/// Detection is by binary presence + service state, and the fallback names the
/// PROPERTY to satisfy rather than a command, because an operator running a
/// hand-rolled nftables ruleset knows their own syntax better than we do.
///
/// The durable answer is the last sentence: an app on the fleet network is
/// container→container traffic, which Docker DOES manage, and no host firewall
/// is in the path at all. `TALARIA_GATEWAY_SELF_URL` exists for exactly that.
async fn firewall_remedy() -> String {
    let port = fleet_layout::app_port();
    let bridges = "172.16.0.0/12";
    let cmd = if exec_ok("ufw", &["status"], Duration::from_secs(3)).await {
        format!("sudo ufw allow from {bridges} to any port {port} proto tcp")
    } else if exec_ok("firewall-cmd", &["--state"], Duration::from_secs(3)).await {
        format!(
            "sudo firewall-cmd --permanent --add-rich-rule='rule family=ipv4 source address={bridges} port port={port} protocol=tcp accept' && sudo firewall-cmd --reload"
        )
    } else if exec_ok("nft", &["list", "ruleset"], Duration::from_secs(3)).await {
        format!("sudo nft add rule inet filter input ip saddr {bridges} tcp dport {port} accept")
    } else {
        format!("admit {bridges} to tcp/{port} on the INPUT chain, however this host manages it")
    };
    format!(
        "Admit the docker bridges to the host: {cmd}. Do the same for the toolkit port. \
         Or remove the host from the path entirely by running Talaria on the agents' network and setting \
         TALARIA_GATEWAY_SELF_URL to its service DNS, which is container→container and needs no firewall rule."
    )
}

/// One TCP connect from inside a container on the fleet network.
///
/// `nc -z` rather than an HTTP request on purpose: the question is whether the
/// packets arrive at all. An HTTP 401 or 405 would prove reachability just as
/// well as a 200, and conflating "refused" with "unauthorised" is how a firewall
/// problem gets misread as a credentials problem.
async fn reaches(network: &str, target: &str, timeout_sec: u64) -> bool {
    let Some((host, port)) = target.split_once(':') else {
        return false;
    };
    let probe = format!("nc -z -w {timeout_sec} {host} {port}");
    exec_ok(
        "docker",
        &[
            "run",
            "--rm",
            "--network",
            network,
            "--add-host",
            "host.docker.internal:host-gateway",
            PROBE_IMAGE,
            "sh",
            "-c",
            &probe,
        ],
        Duration::from_secs(timeout_sec + 20),
    )
    .await
}

/// THE RESOLVERS A RENDERED AGENT ACTUALLY USES — the chassis pins `dns:` per
/// service (see its "External DNS" block for why docker's inherited upstream
/// cannot be trusted), so the probe must carry the same config or it would test
/// a path no agent takes. AGENT_DNS_1/_2 live in fleet/.env; the defaults are
/// the chassis template's.
async fn agent_dns() -> Vec<String> {
    let text = tokio::fs::read_to_string(fleet_layout::fleet_env())
        .await
        .unwrap_or_default();
    dns_from_env_text(&text)
}

fn dns_from_env_text(text: &str) -> Vec<String> {
    let pick = |key: &str, fallback: &str| {
        regex::Regex::new(&format!("(?m)^{key}=(\\S+)"))
            .ok()
            .and_then(|re| re.captures(text).and_then(|c| c.get(1).map(|m| m.as_str())))
            .map(str::to_string)
            .unwrap_or_else(|| fallback.to_string())
    };
    vec![
        pick("AGENT_DNS_1", "1.1.1.1"),
        pick("AGENT_DNS_2", "1.0.0.1"),
    ]
}

/// Can a container on the fleet network resolve an EXTERNAL name? THE SECOND
/// SILENT PATH: the browser toolset fetches its engine from npm on first use
/// and every web tool resolves remote hosts, so agents without external DNS
/// come up green and quietly lose their browser — which is exactly how the
/// built-in browser shipped dead while every health check passed. Probed with
/// the same explicit resolvers the chassis gives agents.
async fn resolves_externally(network: &str, dns: &[String], timeout_sec: u64) -> bool {
    let (Some(primary), Some(secondary)) = (dns.first(), dns.get(1)) else {
        return false;
    };
    exec_ok(
        "docker",
        &[
            "run",
            "--rm",
            "--network",
            network,
            "--dns",
            primary,
            "--dns",
            secondary,
            PROBE_IMAGE,
            "sh",
            "-c",
            "nslookup registry.npmjs.org >/dev/null 2>&1",
        ],
        Duration::from_secs(timeout_sec + 20),
    )
    .await
}

/// The last verdict, for surfaces that must not start a container to render.
/// None when the probe has never run — which is NOT the same as a failure and
/// must not be reported as one.
pub async fn last_fleet_preflight(pg: &PgPool) -> Option<PreflightResult> {
    serde_json::from_value(get_setting(pg, KEY, serde_json::Value::Null).await).ok()
}

/// Ask, from the fleet network, whether Talaria is reachable and the internet
/// resolvable. Never throws: a docker that will not run is its own alert
/// elsewhere, and a preflight that takes the caller down with it is worse than
/// no preflight.
pub async fn run_fleet_preflight(pg: &PgPool) -> PreflightResult {
    // `new Date().toISOString()` — via the house epoch helper (this crate
    // keeps no clock; time is read here, at the edge, exactly once).
    let at = crate::agent_auth::epoch_ms_to_iso(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    );
    let result = match probe(&at).await {
        Ok(r) => r,
        // The shape of the TS catch: name the failure, keep the target, and
        // still write a verdict — an unreachable preflight is itself an answer
        // somebody reads later.
        Err(e) => PreflightResult {
            ok: false,
            target: agent_reach_target().unwrap_or_else(|_| fleet_layout::mcp_gw_base()),
            detail: format!("preflight could not run: {e}"),
            at,
        },
    };
    let value = serde_json::to_value(&result).unwrap_or_default();
    let _ = set_setting(pg, KEY, &value).await;
    result
}

async fn probe(at: &str) -> Result<PreflightResult, String> {
    let network = fleet_layout::fleet_network_name().await;
    let app = agent_reach_target()?;
    let mcp = agent_reach_target()?;
    let dns = agent_dns().await;
    let (app_ok, mcp_ok, dns_ok) = tokio::join!(
        reaches(&network, &app, 5),
        reaches(&network, &mcp, 5),
        resolves_externally(&network, &dns, 5),
    );

    if app_ok && mcp_ok && dns_ok {
        return Ok(PreflightResult {
            ok: true,
            target: app,
            detail: "agents can reach the app, the toolkit, and the internet".into(),
            at: at.into(),
        });
    }
    let mut parts: Vec<String> = Vec::new();
    if !(app_ok && mcp_ok) {
        let dead = [
            (!app_ok).then(|| format!("the app ({app})")),
            (!mcp_ok).then(|| format!("the MCP gateway ({mcp})")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" and ");
        let remedy = firewall_remedy().await;
        // Named concretely, because the generic version of this sentence is what
        // cost a day: the failure is on the HOST, in a place Docker's own rules
        // do not cover, and the fix is a firewall rule rather than anything in
        // Talaria.
        parts.push(format!(
            "a container on the \"{network}\" network cannot reach {dead}. Agents are running but cannot call a single tool. \
             Container→host traffic goes through the host's INPUT chain, which Docker does NOT manage, so a default-deny \
             firewall blocks it while every Docker rule still looks correct. {remedy}"
        ));
    }
    if !dns_ok {
        // The browser shipped dead exactly like this: every health check green,
        // every external name EAI_AGAIN/SERVFAIL, no error anywhere a person
        // would read.
        let names = dns.join(" / ");
        parts.push(format!(
            "a container on the \"{network}\" network cannot resolve external names through {names}. \
             Agents look healthy but the browser toolset (and every web lookup) is dead. \
             If this network blocks public resolvers, set AGENT_DNS_1/AGENT_DNS_2 in fleet/.env to one it can reach \
             (the host's upstream, or a corporate forwarder), then re-render and restart the agents."
        ));
    }
    Ok(PreflightResult {
        ok: false,
        target: app,
        detail: parts.join(" ALSO: "),
        at: at.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_derives_from_the_gateway_base_the_renderer_stamps() {
        // An explicit port rides through; a scheme default fills a missing one.
        assert_eq!(
            target_of("http://10.0.0.5:9000/api/mcp/gw").unwrap(),
            "10.0.0.5:9000"
        );
        assert_eq!(
            target_of("http://host.docker.internal:5273/api/mcp/gw").unwrap(),
            "host.docker.internal:5273"
        );
        assert_eq!(
            target_of("https://gw.internal.example").unwrap(),
            "gw.internal.example:443"
        );
        assert_eq!(
            target_of("http://gw.internal.example").unwrap(),
            "gw.internal.example:80"
        );
        // A malformed base is the TS `new URL` throw, not a guess.
        assert_eq!(target_of("not a url").unwrap_err(), "Invalid URL");
    }

    #[test]
    fn the_dns_pick_reads_the_fleet_env_and_falls_back_to_the_chassis_defaults() {
        assert_eq!(
            dns_from_env_text(""),
            vec!["1.1.1.1".to_string(), "1.0.0.1".to_string()]
        );
        assert_eq!(
            dns_from_env_text("HERMES_KEY_A=x\nAGENT_DNS_1=9.9.9.9\nOTHER=y\n"),
            vec!["9.9.9.9".to_string(), "1.0.0.1".to_string()]
        );
        // Mid-line matches don't count (the ^ anchor), empty values don't
        // either (\S+), and both pins can ride together.
        assert_eq!(
            dns_from_env_text("XAGENT_DNS_1=8.8.8.8\nAGENT_DNS_2=8.8.4.4\nAGENT_DNS_2=\n"),
            vec!["1.1.1.1".to_string(), "8.8.4.4".to_string()]
        );
    }

    #[test]
    fn the_verdict_serializes_in_the_shape_the_alerts_panel_reads() {
        let r = PreflightResult {
            ok: false,
            target: "host.docker.internal:5273".into(),
            detail: "preflight could not run: Invalid URL".into(),
            at: "2026-08-30T00:00:00.000Z".into(),
        };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            "{\"ok\":false,\"target\":\"host.docker.internal:5273\",\"detail\":\"preflight could not run: Invalid URL\",\"at\":\"2026-08-30T00:00:00.000Z\"}"
        );
        // A stored verdict decodes back; null (never run) must not become one.
        let back: PreflightResult =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert!(!back.ok);
        assert!(serde_json::from_value::<PreflightResult>(serde_json::Value::Null).is_err());
    }
}
