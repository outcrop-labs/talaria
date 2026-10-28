// THE SEARCH URL, ASKED FROM WHERE THE AGENTS SIT.
//
// The workspace search URL has two audiences. Research dials it from the app
// process, and every agent's Hermes dials it from its own container as
// SEARXNG_URL. Admin → Search used to probe only the first: a SearXNG bound to
// the host's 127.0.0.1 answered the app (green dot) while every agent, dialling
// `host.docker.internal`, got "connection refused"; a compose hostname only the
// app's networks resolve did the same. The rendered URL also switches Hermes's
// keyless public tiers off, so an unreachable one is not a degraded search, it
// is no search at all.
//
// So this probe runs the app's own canary (`talaria_search::search_reachable`:
// a real query, an empty result set counts as a failure, the 403 sentence for
// SearXNG's JSON-off default) with ONE edge swapped: the fetch runs inside a
// running managed agent, over `docker exec`, with that agent's network
// namespace, DNS and proxy environment, against the URL exactly as the render
// hands it to the agents (the loopback rewrite included). The URL travels as
// an argv element of a fixed Python program, never through a shell. The agent
// is the first running managed agent by slug: every fleet agent sits on the
// same fleet network with the same host mapping, so one answers for all.
//
// "Nothing to probe" is not a failure, and the answer keeps it apart: no URL
// rendered to the agents, or no agent running, is `ok: None` with the reason,
// never a red dot.

use std::sync::{Arc, Mutex};

use sqlx::PgPool;
use talaria_search::{FetchReply, SearchDeps, search_reachable};

/// What the probe from inside an agent found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReach {
    /// Some(true) the agents' URL answered with results; Some(false) it did
    /// not; None there was nothing to probe from or to (see `error`).
    pub ok: Option<bool>,
    /// The sentence: the failure, or why nothing was probed. None only when
    /// there is no search URL at all (the app-side answer already says so)
    /// or the probe passed.
    pub error: Option<String>,
    /// The agent the probe ran in (its display name), when one did.
    pub probed_from: Option<String>,
    /// The URL as the agents dial it, after the render's loopback rewrite.
    pub url: Option<String>,
}

impl AgentReach {
    fn nothing(error: Option<String>, url: Option<String>) -> Self {
        Self {
            ok: None,
            error,
            probed_from: None,
            url,
        }
    }
}

/// The program the agent runs: one GET with Hermes's own SearXNG timeout
/// (15 s, `plugins/web/searxng/provider.py`), and a reply on stdout the probe
/// parses — `<status>\n<body>`, or `ERR <reason>` when the request never got
/// an HTTP answer. Always exit 0 on a network failure, so a non-zero exit
/// means the probe itself could not run (no python, the container went away),
/// which is not the search URL's fault and is not reported as if it were.
const PROBE_PY: &str = r#"import sys, urllib.request, urllib.error
try:
    req = urllib.request.Request(sys.argv[1], headers={"Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=15) as r:
        status, body = r.status, r.read(4000000)
except urllib.error.HTTPError as e:
    status, body = e.code, e.read(65536)
except Exception as e:
    print("ERR " + str(getattr(e, "reason", None) or e))
    sys.exit(0)
sys.stdout.write(str(status) + "\n")
sys.stdout.write(body.decode("utf-8", "replace"))
"#;

/// The exec's own bound: the program's 15 s request plus `docker exec`'s
/// start-up, with room to spare on a loaded host.
const PROBE_EXEC_MS: u64 = 30_000;

/// What the program printed, as the fetch edge answers it.
fn parse_probe_output(stdout: &str) -> Result<FetchReply, String> {
    if let Some(reason) = stdout.strip_prefix("ERR ") {
        return Err(reason.trim().to_string());
    }
    let (status, body) = stdout.split_once('\n').unwrap_or((stdout, ""));
    let status: u16 = status
        .trim()
        .parse()
        .map_err(|_| "the probe printed no HTTP status".to_string())?;
    Ok(FetchReply {
        status,
        body: body.to_string(),
    })
}

/// The first running managed agent, by slug: (department, display name).
async fn probe_agent(pg: &PgPool) -> Result<Option<(String, String)>, String> {
    let managed: Vec<(String, String)> = sqlx::query_as(
        "select department, display_name from agent_defs where managed and enabled order by slug",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    if managed.is_empty() {
        return Ok(None);
    }
    let departments: Vec<String> = managed.iter().map(|(d, _)| d.clone()).collect();
    let running = talaria_fleet_docker::running_departments(&departments).await?;
    Ok(managed.into_iter().find(|(d, _)| running.contains(d)))
}

/// Probe the workspace search URL from inside one running agent.
pub async fn agent_search_reachable(pg: &PgPool) -> AgentReach {
    let app_url = talaria_search::search_url(pg).await;
    if app_url.is_empty() {
        return AgentReach::nothing(None, None);
    }
    let Some(url) =
        talaria_fleet_layout::agent_search_url(&app_url, talaria_fleet_layout::dial_is_container())
    else {
        return AgentReach::nothing(
            Some(
                "not handed to the agents: a loopback address is the app container itself to \
                 an agent, so the render withholds it; give it an address the agents can reach"
                    .into(),
            ),
            None,
        );
    };
    let (department, display_name) = match probe_agent(pg).await {
        Ok(Some(agent)) => agent,
        Ok(None) => {
            return AgentReach::nothing(
                Some("no agent is running to probe from".into()),
                Some(url),
            );
        }
        Err(e) => {
            return AgentReach::nothing(
                Some(format!("could not find a running agent to probe from: {e}")),
                Some(url),
            );
        }
    };
    let container = talaria_fleet_docker::managed_container(pg, &department).await;
    // Set when the exec itself failed: the canary then reads it as a dead
    // endpoint, and this slot is how the answer tells the two apart.
    let exec_failed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let deps = {
        let exec_failed = exec_failed.clone();
        let container = container.clone();
        let url = url.clone();
        SearchDeps {
            fetch: Arc::new(move |full: String| {
                let exec_failed = exec_failed.clone();
                let container = container.clone();
                Box::pin(async move {
                    match talaria_fleet_docker::docker_exec_as(
                        &container,
                        Some("hermes"),
                        &["python3", "-c", PROBE_PY, &full],
                        None,
                        PROBE_EXEC_MS,
                    )
                    .await
                    {
                        Ok((stdout, _)) => parse_probe_output(&stdout),
                        Err(e) => {
                            let e = if e.is_empty() {
                                "the probe exited without a reason".to_string()
                            } else {
                                e
                            };
                            *exec_failed.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(e.clone());
                            Err(e)
                        }
                    }
                })
            }),
            url: Arc::new(move |_| {
                let url = url.clone();
                Box::pin(async move { url })
            }),
        }
    };
    let reach = search_reachable(pg, &deps).await;
    if let Some(e) = exec_failed.lock().unwrap_or_else(|p| p.into_inner()).take() {
        return AgentReach {
            ok: None,
            error: Some(format!("could not probe from {display_name}: {e}")),
            probed_from: Some(display_name),
            url: Some(url),
        };
    }
    AgentReach {
        ok: Some(reach.ok),
        error: reach.error,
        probed_from: Some(display_name),
        url: Some(url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_is_its_status_then_its_body() {
        let r = parse_probe_output("200\n{\"results\": []}").unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "{\"results\": []}");
        // A body with newlines of its own keeps them: only the first splits.
        let r = parse_probe_output("403\nForbidden\nline two").unwrap();
        assert_eq!(r.status, 403);
        assert_eq!(r.body, "Forbidden\nline two");
        // No body at all.
        let r = parse_probe_output("204\n").unwrap();
        assert_eq!((r.status, r.body.as_str()), (204, ""));
    }

    #[test]
    fn a_network_failure_is_the_reason_the_agent_got() {
        assert_eq!(
            parse_probe_output("ERR [Errno 111] Connection refused\n")
                .err()
                .as_deref(),
            Some("[Errno 111] Connection refused")
        );
        assert_eq!(
            parse_probe_output("ERR [Errno -2] Name or service not known\n")
                .err()
                .as_deref(),
            Some("[Errno -2] Name or service not known")
        );
    }

    #[test]
    fn output_that_is_neither_is_not_read_as_an_answer() {
        assert!(parse_probe_output("").is_err());
        assert!(parse_probe_output("Traceback (most recent call last):\n").is_err());
    }

    // The program is argv-driven: the URL is never part of the source text,
    // so no URL can change what runs.
    #[test]
    fn the_program_reads_its_url_from_argv() {
        assert!(PROBE_PY.contains("sys.argv[1]"));
        assert!(!PROBE_PY.contains("{url}"));
        assert!(PROBE_PY.contains("timeout=15"));
    }
}
