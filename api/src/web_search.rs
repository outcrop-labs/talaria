// HOW THIS DEPLOYMENT SEARCHES THE WEB — one answer, for every caller.
//
// THE PROBLEM THIS ENDS. Search is resolved the way the whole capability
// model says it should be — the org's registered MCP servers first,
// Talaria's own SearXNG as the floor underneath — and every caller goes
// through that resolution: the `web_search` tool every agent reaches,
// research, and the fitness sweep alike. Nobody is nailed to the
// self-hosted instance while better keys sit registered — the working
// agents are the last callers who should be stuck on it.
//
// SEARXNG IS THE FLOOR, NOT THE CEILING, and that is the whole design. A
// fresh install with no keys and no registry has working web search on day
// one. An org that registers a real provider upgrades every caller at once
// — agents, research, the fitness sweep — with no code change and nothing
// to migrate, because they all ask this file.
//
// THE WALKER (`results_from_payload`) normalises a registered tool's
// payload into `SearchResult`s, shape-agnostically. The resolver AROUND it
// (`search_the_web`, which asks the capability model who supplies search
// and routes to SearXNG or a registered server) lives here too. SearXNG's
// own rows go through `search::results_from` instead — its shape is known,
// so it is narrowed rather than walked.

use std::collections::HashSet;

use serde_json::Value;

use crate::body::truncate_utf16;
use crate::search::SearchResult;

/// A REGISTERED TOOL'S PAYLOAD, NORMALISED — and it has to be shape-agnostic,
/// because every provider spells a result differently and none of them owes us
/// SearXNG's field names. Walks the whole tree for anything carrying an http URL
/// and takes the neighbouring title and snippet, whatever they are called.
///
/// A RESULT WITHOUT A URL IS DROPPED, the same rule `search::results_from`
/// applies to our own engine: an uncitable result in a research pipeline is
/// worse than one fewer result, because `ungrounded_ref` grounds every claim
/// against these.
///
/// BOTH KINDS OF DESCENT BUMP THE DEPTH — array items at `depth + 1` and object
/// values at `depth + 1` alike — so the budget is a measure of how DEEP the
/// payload nests, not of how wide. A flat page of results and a `data.hits[]`
/// envelope both arrive within two or three.
pub fn results_from_payload(payload: &Value, engine: &str, cap: usize) -> Vec<SearchResult> {
    let mut out: Vec<SearchResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    fn first_string<'a>(o: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
        keys.iter().find_map(|k| o.get(*k).and_then(Value::as_str))
    }

    fn first_http_url(o: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|k| {
            o.get(*k)
                .and_then(Value::as_str)
                .filter(|s| is_http_url(s))
                .map(str::to_string)
        })
    }

    fn walk(
        node: &Value,
        depth: usize,
        engine: &str,
        cap: usize,
        out: &mut Vec<SearchResult>,
        seen: &mut HashSet<String>,
    ) {
        if out.len() >= cap || depth > 6 || node.is_null() {
            return;
        }
        if let Some(items) = node.as_array() {
            for item in items {
                walk(item, depth + 1, engine, cap, out, seen);
            }
            return;
        }
        let Some(o) = node.as_object() else {
            return;
        };
        if let Some(url) = first_http_url(o, &["url", "link", "href", "source"])
            && !seen.contains(&url)
        {
            seen.insert(url.clone());
            // An EMPTY-STRING title stands (it is a string the server sent);
            // only the absence of any of the three spellings falls back to
            // the URL.
            let title = first_string(o, &["title", "name", "heading"])
                .unwrap_or(&url)
                .to_string();
            let snippet =
                first_string(o, &["snippet", "description", "text", "content", "summary"])
                    .map(|s| truncate_utf16(s, 600).to_string())
                    .unwrap_or_default();
            out.push(SearchResult {
                title,
                url,
                snippet,
                engine: engine.to_string(),
            });
        }
        for v in o.values() {
            walk(v, depth + 1, engine, cap, out, seen);
        }
    }

    walk(payload, 0, engine, cap, &mut out, &mut seen);
    out
}

/// Case-insensitive scheme check, so an upper-case scheme is still a web URL.
/// `get(..n)` rather than slicing: a prefix cut mid-codepoint is a non-URL,
/// not a panic.
fn is_http_url(s: &str) -> bool {
    s.get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
        || s.get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
}

// ── The resolver half ───────────────────────────────────────────────────────
//
// The pieces it composes: DbReach reads servers + providers + platform,
// `search::search_web` is the SearXNG client, and `call_mcp_tool` speaks to
// a registered server.

#[derive(Debug, Clone, serde::Serialize)]
pub struct WebSearch {
    pub results: Vec<SearchResult>,
    /// WHO ACTUALLY ANSWERED. Reported rather than hidden because "we
    /// searched" and "we searched with Exa" are different claims, and an
    /// admin debugging a thin result set needs to know which engine to go and
    /// look at. Null means Talaria's own instance.
    pub via: Option<crate::capability_reach::Supplier>,
}

/// Search the web with whatever this deployment has, best first.
///
/// ERRS RATHER THAN RETURNING EMPTY when nothing can search, because the
/// caller is a tool handler and its error text lands in an agent's transcript.
/// A model handed an empty result set answers from memory in a confident
/// voice; a model handed a sentence saying search is unavailable says so.
pub async fn search_the_web(
    state: &crate::state::AppState,
    query: &str,
    limit: Option<f64>,
) -> Result<WebSearch, String> {
    use crate::capability_platform::is_platform_server;
    use crate::capability_reach::{
        DbReach, PROVIDERS_KEY, Providers, ReachDeps, platform_supply, supplier_for,
    };
    use crate::gateway::settings::get_setting;
    use crate::search::{DEFAULT_LIMIT, real_deps, search_web};

    let pg = &state.pg;
    let reach = DbReach { pg };
    let (servers, providers, platform) = tokio::join!(
        reach.servers(),
        async {
            let raw = get_setting(pg, PROVIDERS_KEY, serde_json::json!({})).await;
            serde_json::from_value::<Providers>(raw).unwrap_or_default()
        },
        platform_supply(pg)
    );
    let supplier = supplier_for("search", &servers, &providers, &platform);

    // Nothing registered and nothing of our own that works — `platform_supply`
    // withholds SearXNG when its canary query comes back empty, which is what
    // a CAPTCHA-walled instance looks like. Refusing here is the honest answer.
    let Some(supplier) = supplier else {
        return Err(
            "live web search is not available in this workspace: no search provider is registered and this deployment has no working search engine of its own. Tell whoever asked that you could not search rather than answering from memory."
                .to_string(),
        );
    };

    // OUR OWN ENGINE GOES STRAIGHT TO THE CLIENT, not out through
    // `call_mcp_tool` — Talaria is in nobody's MCP registry, so routing it
    // there is the exact bug that made the platform supplier a lie the first
    // time.
    if is_platform_server(&supplier.server) {
        let results = search_web(
            pg,
            query,
            Some(limit.unwrap_or(DEFAULT_LIMIT as f64)),
            &real_deps(),
        )
        .await?;
        return Ok(WebSearch { results, via: None });
    }

    let mut args = serde_json::Map::new();
    args.insert("query".into(), Value::String(query.to_string()));
    args.insert(
        "limit".into(),
        Value::from(limit.unwrap_or(DEFAULT_LIMIT as f64) as i64),
    );
    let sb = state
        .secretbox()
        .await
        .map_err(|e| format!("secretbox unavailable: {e}"))?;
    let out = crate::mcp::registry::call_mcp_tool(pg, &sb, &supplier.server, &supplier.tool, &args)
        .await?;
    let payload = out
        .structured
        .clone()
        .unwrap_or(Value::String(out.text.clone()));
    let mut results = results_from_payload(&payload, &supplier.server, 25);
    let cap = limit.unwrap_or(DEFAULT_LIMIT as f64).max(0.0) as usize;
    results.truncate(cap);
    Ok(WebSearch {
        results,
        via: Some(supplier),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_envelopes_servers_actually_send() {
        // `data.hits[]` with a provider's own field names, plus a bare array of
        // `link`-spelled rows: one walker, both.
        let payload = json!({
            "data": { "hits": [
                { "name": "The page", "link": "https://a.test/one", "description": "what it says" },
                { "href": "https://b.test/two" }
            ] },
            "results": [
                { "url": "https://c.test/three", "title": "C", "snippet": "c" }
            ]
        });
        let got = results_from_payload(&payload, "exa", 25);
        let urls: Vec<&str> = got.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://a.test/one",
                "https://b.test/two",
                "https://c.test/three"
            ]
        );
        // The URL stands in for the title when no field carries one.
        assert_eq!(got[0].title, "The page");
        assert_eq!(got[1].title, "https://b.test/two");
        // Every row is tagged with the engine the caller named.
        assert!(got.iter().all(|r| r.engine == "exa"));
    }

    #[test]
    fn a_row_without_an_http_url_is_dropped_and_a_duplicate_is_one_result() {
        let payload = json!([
            { "url": "https://a.test", "title": "first spelling" },
            { "url": "https://a.test", "title": "second spelling" },
            { "url": "ftp://not-web.test", "title": "a scheme we cannot cite" },
            { "url": "/relative/path", "title": "not a URL at all" },
            { "title": "no url key anywhere" },
            "a bare string in the array"
        ]);
        let got = results_from_payload(&payload, "tool", 25);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "first spelling");
    }

    #[test]
    fn the_cap_bounds_the_walk_and_the_snippet() {
        let rows: Vec<Value> = (0..40)
            .map(|i| {
                json!({ "url": format!("https://s{i}.test"), "title": format!("s{i}"),
                        "snippet": "x".repeat(900) })
            })
            .collect();
        let got = results_from_payload(&Value::Array(rows), "tool", 12);
        assert_eq!(got.len(), 12);
        // The snippet clip counts UTF-16 units — 600 code units, not bytes.
        assert_eq!(got[0].snippet.len(), 600);
        assert!(got[0].snippet.chars().all(|c| c == 'x'));
    }

    #[test]
    fn deeply_nested_payloads_stop_at_the_depth_budget() {
        // Seven levels of wrapping with the URL at the bottom: past the budget,
        // because every descent (array AND object) costs one.
        let mut payload = json!({ "url": "https://deep.test", "title": "too deep" });
        for _ in 0..7 {
            payload = json!({ "next": payload });
        }
        assert!(results_from_payload(&payload, "tool", 25).is_empty());
        // Six levels down still arrives — the budget is `> 6`, not `>= 6`.
        let mut payload = json!({ "url": "https://deep.test", "title": "just enough" });
        for _ in 0..6 {
            payload = json!({ "next": payload });
        }
        assert_eq!(results_from_payload(&payload, "tool", 25).len(), 1);
    }
}
