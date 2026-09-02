// WHAT TALARIA ITSELF CAN SUPPLY, right now, on this install.
// The DISPATCH half — the supply half
// (the measured, minute-cached advertisement) lives in
// capability_reach.rs with the reach model, which is why PLATFORM_SERVER lives
// there and is re-exported here rather than spelled twice.
//
// WHY IT IS ITS OWN FILE. `capability_reach.rs` answers "can the run reach this
// capability" and must stay ignorant of HOW any particular tool works — it reads
// a registry and matches names. This module is the other half: it knows that
// `search` is SearXNG over HTTP, and it runs the tool a supplier named.
// Keeping them apart is what lets capability_reach have no dependency on the
// tool implementations, and lets this module import them freely.
//
// RUN ONE OF TALARIA'S OWN TOOLS — the half whose absence once made the supply
// half a lie. `platform_supply` advertised `{ server: 'talaria', tool:
// 'web_search' }` and every dispatcher in the tree sent tool calls to
// `callMcpTool`, which looks the server up in the MCP registry and throws
// `MCP server "talaria" is not registered`. The tool loop caught that, fed the
// model `The search tool failed: ...` as the tool RESULT, and the model — having
// dutifully called the tool it was offered — answered from memory anyway.
// Nothing crashed. The sweep recorded a search stage that ran, called its tool,
// and produced no sources.
//
// That is the exact failure this module exists to prevent: a supplier we
// cannot honor is worse than no supplier, because None refuses honestly and
// this returns a confident uncited answer. Advertising and dispatch ship
// together, one import apart, so they cannot drift.
//
// Shaped like `McpToolResult` — `{ text, structured }` — because every caller
// already speaks it and a platform tool should be indistinguishable from a
// registered one at the call site.

use serde_json::{Map, Value};
use sqlx::PgPool;

use crate::search::{SearchDeps, search_web};

/// Spelled once, in capability_reach (where the supply half lives); this is
/// its platform-facing alias.
pub use crate::capability_reach::PLATFORM_SERVER;

/// Is this supplier Talaria itself, rather than a registered MCP server? Every
/// caller that dispatches a tool call has to ask, because the two go to
/// completely different places.
pub fn is_platform_server(server: &str) -> bool {
    server == PLATFORM_SERVER
}

/// One tool result, flattened to the two things a caller needs: the text the
/// model should see, and whatever structured payload the tool also returned
/// (None is the wire's `structured: null`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlatformToolResult {
    pub text: String,
    pub structured: Option<Value>,
}

/// RUN ONE OF TALARIA'S OWN TOOLS. `web_search` is the one tool dispatched
/// here; `describe_image` is advertised when the vision role is filled but is
/// served by the vision route, never this dispatcher — so its branch below is
/// a loud refusal (an advertisement that cannot dispatch is the exact bug
/// this file exists to prevent); anything else is not ours to run.
///
/// The search engine edge is injected rather than hard-wired so the tool is
/// testable against a scripted SearXNG; the caller that has no opinion passes
/// `real_deps()`. Errors are `Err` rather than a shaped refusal: the caller is
/// the tool loop, whose catch turns any failure into `The search tool failed: …`
/// — the model is told, the run survives, the sources list stays honest.
pub async fn call_platform_tool(
    pg: &PgPool,
    tool: &str,
    args: &Map<String, Value>,
    deps: &SearchDeps,
) -> Result<PlatformToolResult, String> {
    if tool == "web_search" {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("");
        if query.trim().is_empty() {
            return Ok(PlatformToolResult {
                text: r#"web_search needs a "query" — say what to look up."#.into(),
                structured: None,
            });
        }
        // `typeof args.limit === 'number'` — only a JSON number counts; a
        // string "10" is absent, not parsed.
        let limit = args.get("limit").and_then(Value::as_f64);
        let results = search_web(pg, query, limit, deps).await?;
        if results.is_empty() {
            return Ok(PlatformToolResult {
                text: format!("No results for \"{query}\"."),
                // An EMPTY ARRAY, not null: the search ran and found nothing,
                // which is a different fact from the tool not answering.
                structured: Some(Value::Array(Vec::new())),
            });
        }
        // BOTH FORMS, because the two readers want different things: the model
        // reads `text`, and the source walker reads `structured` to build the
        // citation list. Returning only prose is how a search stage produces
        // findings nobody can trace back to a URL.
        let text = results
            .iter()
            .enumerate()
            .map(|(i, r)| format!("{}. {}\n   {}\n   {}", i + 1, r.title, r.url, r.snippet))
            .collect::<Vec<_>>()
            .join("\n\n");
        let structured = Some(serde_json::to_value(&results).expect("SearchResult is plain data"));
        return Ok(PlatformToolResult { text, structured });
    }

    if tool == "describe_image" {
        // The vision harness (vision.rs, serving /api/vision/describe)
        // is never dispatched through here — a caller reaching this branch routed
        // a supplier to the wrong surface, so the honest answer is a refusal
        // that names the gap rather than a half-answer.
        return Err(
            "describe_image runs on the vision harness, never through the platform tools — \
             this supplier should not have been reachable from this stage"
                .into(),
        );
    }

    Err(format!("\"{tool}\" is not one of Talaria's own tools"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    /// A scripted SearXNG: fixed URL leg, fixed 200 body. No database query
    /// ever runs (the url edge is injected, so the lazy pool is never dialed).
    fn engine(body: &'static str) -> SearchDeps {
        SearchDeps {
            fetch: Arc::new(move |_url| {
                Box::pin(async move {
                    Ok(crate::search::FetchReply {
                        status: 200,
                        body: body.into(),
                    })
                })
            }),
            url: Arc::new(|_pg| Box::pin(async { "http://search.test".to_string() })),
        }
    }

    fn lazy_pg() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://platform-test@localhost:5432/platform-test")
            .expect("a lazy pool connects to nothing")
    }

    #[tokio::test]
    async fn web_search_answers_in_both_forms() {
        let deps = engine(
            r#"{"results":[
                {"url":"https://a.test","title":"A page","content":"the passage","engine":"bing"}
            ]}"#,
        );
        let args = json!({ "query": "node 24 end of life" });
        let out = call_platform_tool(&lazy_pg(), "web_search", args.as_object().unwrap(), &deps)
            .await
            .unwrap();
        assert_eq!(out.text, "1. A page\n   https://a.test\n   the passage");
        // The structured form carries the full row — the source walker reads
        // url, title, snippet straight off it.
        assert_eq!(
            out.structured,
            Some(json!([
                { "title": "A page", "url": "https://a.test", "snippet": "the passage", "engine": "bing" }
            ]))
        );
    }

    #[tokio::test]
    async fn a_blank_query_is_a_shaped_answer_not_a_search() {
        let deps = engine(r#"{"results":[]}"#);
        for args in [json!({}), json!({ "query": "   " }), json!({ "query": 7 })] {
            let out =
                call_platform_tool(&lazy_pg(), "web_search", args.as_object().unwrap(), &deps)
                    .await
                    .unwrap();
            assert_eq!(
                out.text,
                "web_search needs a \"query\" — say what to look up."
            );
            assert_eq!(out.structured, None);
        }
    }

    #[tokio::test]
    async fn no_results_is_an_empty_array_not_null() {
        let deps = engine(r#"{"results":[]}"#);
        let args = json!({ "query": "obscure" });
        let out = call_platform_tool(&lazy_pg(), "web_search", args.as_object().unwrap(), &deps)
            .await
            .unwrap();
        assert_eq!(out.text, "No results for \"obscure\".");
        assert_eq!(out.structured, Some(Value::Array(Vec::new())));
    }

    #[tokio::test]
    async fn an_engine_failure_is_an_error_the_tool_loop_can_catch() {
        let deps = SearchDeps {
            fetch: Arc::new(|_url| Box::pin(async { Err("connection refused".to_string()) })),
            url: Arc::new(|_pg| Box::pin(async { "http://search.test".to_string() })),
        };
        let args = json!({ "query": "anything" });
        let err = call_platform_tool(&lazy_pg(), "web_search", args.as_object().unwrap(), &deps)
            .await
            .unwrap_err();
        // The tool loop wraps this in `The search tool failed: …` — the shape
        // it needs is a sentence, and the caller adds the frame.
        assert!(
            err.starts_with("the search service at http://search.test did not answer"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn describe_image_refuses_loudly_and_nothing_else_is_ours() {
        let deps = engine(r#"{"results":[]}"#);
        let err = call_platform_tool(
            &lazy_pg(),
            "describe_image",
            json!({}).as_object().unwrap(),
            &deps,
        )
        .await
        .unwrap_err();
        assert!(err.contains("vision harness"), "{err}");
        assert!(err.contains("never through the platform tools"), "{err}");
        let err = call_platform_tool(
            &lazy_pg(),
            "make_coffee",
            json!({}).as_object().unwrap(),
            &deps,
        )
        .await
        .unwrap_err();
        assert_eq!(err, "\"make_coffee\" is not one of Talaria's own tools");
    }

    #[test]
    fn the_platform_server_name_is_one_spelling() {
        assert_eq!(PLATFORM_SERVER, "talaria");
        assert!(is_platform_server("talaria"));
        assert!(!is_platform_server("exa"));
    }
}
