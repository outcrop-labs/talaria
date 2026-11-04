// A REGISTERED SEARCH TOOL'S PAYLOAD, NORMALISED — the walker research's
// search loop runs over whatever an org's registered MCP search supplier
// (Exa, Tavily, …) sends back.
//
// WHAT THIS CRATE IS NOT ANY MORE. It used to hold a resolver too
// (`search_the_web`, behind `POST /api/search`), the door the agent-side
// `web_search` MCP tool called. That tool went on 2026-10-16: a fleet agent
// searches with Hermes's own `web_search`, which Talaria configures (Admin →
// Search sets `search_url`, rendered into every agent as `SEARXNG_URL`)
// rather than duplicates. Nothing else called the resolver, so it went too.
// Research resolves its own supplier (`talaria_capability_reach` plus the
// research harness's dispatch hook) and only borrows this walker.
//
// SearXNG's own rows go through `search::results_from` instead — its shape is
// known, so it is narrowed rather than walked.

use std::collections::HashSet;

use serde_json::Value;

use talaria_body::truncate_utf16;
use talaria_search::SearchResult;

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
