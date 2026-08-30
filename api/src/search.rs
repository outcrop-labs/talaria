// LIVE WEB SEARCH, SUPPLIED BY THE DEPLOYMENT. Port of ui/src/server/search.ts
// (the hand-rolled client the migration plan chose over the 0.1.0 crate).
//
// WHY TALARIA OWNS THIS RATHER THAN REGISTERING SOMEBODY'S MCP SERVER. The
// capability model already knows how to route around a model that cannot
// search: `supplier_for('search', …)` finds a registered tool, and the
// tool-search transport drives it so a blind model still produces cited
// findings. What was missing was a tool to find. Every option was either a
// hosted API with a key and a per-query bill, or a thin third-party MCP bridge —
// a whole extra process and an unvetted dependency in the request path, for
// about fifty lines of `fetch`.
//
// So the ENGINE is SearXNG and the CLIENT is this file. SearXNG is a metasearch
// aggregator: it fans one query across dozens of engines, needs no API key,
// costs nothing per query, and no query leaves the operator's infrastructure.
// Its JSON API returns `title/url/content/engine` per result, which is already
// the shape `web_search::results_from_payload` walks.
//
// THE LICENCE DECIDED THE ARCHITECTURE, so it is written down here rather than
// left for somebody to rediscover. Talaria is MIT; SearXNG is AGPL-3.0. Vendoring
// AGPL source into an MIT codebase propagates the terms, so SearXNG runs as its
// own container and Talaria talks to it over HTTP — the arrangement the fleet
// MCP service already uses. Two consequences worth keeping:
//
//   DO NOT PATCH SEARXNG. AGPL §13 obliges anyone OFFERING it as a network
//   service to offer its source to users of that service. Unmodified upstream
//   satisfies that by pointing at the upstream repository; a local patch means
//   publishing the patch.
//
//   DO NOT IMPORT ITS CODE. Configure it. Everything this file needs is on the
//   documented JSON API.
//
// TWO CONFIGURATION FACTS that cost an afternoon each if you meet them cold, and
// which docker/searxng/settings.yml sets for you: the JSON format is OFF by
// default and a request for it 403s until `search.formats` includes `json`; and
// the rate limiter throttles our own agents unless it is off for an internal
// instance.
//
// BOTH LIVE IN A MOUNTED settings.yml, NOT IN ENV VARS, and that is worth
// knowing before you try the obvious thing: SearXNG honours environment
// variables for a short allow-list only (base URL, secret key). Setting
// `SEARXNG_SEARCH_FORMATS` is silently ignored, and the instance goes on
// answering 403 while looking correctly configured.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;

use crate::gateway::provider::http;
use crate::gateway::settings::get_setting;

/// Where SearXNG lives. Env first so a self-host can point at an instance it
/// already runs; the setting is the in-app override; the default is the service
/// name in Talaria's own compose file.
pub const SEARCH_URL_KEY: &str = "search_url";
const DEFAULT_URL: &str = "http://127.0.0.1:8888";

/// `searchUrl`: env leg first, then the setting, then the compose default.
pub async fn search_url(pg: &PgPool) -> String {
    let configured = match std::env::var("SEARXNG_URL") {
        Ok(v) => Some(v),
        Err(_) => get_setting(pg, SEARCH_URL_KEY, Value::String(String::new()))
            .await
            .as_str()
            .map(String::from),
    };
    resolve_search_url(configured)
}

/// The precedence, pure. The env leg is resolved by `??`, not truthiness — an
/// env var that is SET BUT EMPTY suppresses the setting read and falls straight
/// to the default, exactly as `process.env.SEARXNG_URL ?? getSetting(...)` does
/// (the caller only reaches the setting leg when the env var is UNSET). The
/// default leg is `||`, so any empty resolution lands on the service name. ONE
/// trailing slash comes off — `replace(/\/$/, '')` — not all of them, because
/// that is the TS spelling and `http://host/` is the only shape operators type.
fn resolve_search_url(configured: Option<String>) -> String {
    let base = configured
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string());
    base.strip_suffix('/').unwrap_or(&base).to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    /// The engine's own snippet. Empty rather than null — every consumer
    /// concatenates it, and a null in that position is a `"null"` in a prompt.
    pub snippet: String,
    /// Which upstream engine produced it. Kept because a result every engine
    /// agrees on is worth more than one only a single engine returned, and
    /// because it is how an operator notices an engine has stopped answering.
    pub engine: String,
}

/// RESULTS PER QUERY. Ten is what a research stage can actually read: the
/// synthesis turn has to fit them all in one prompt alongside the question, and
/// a model handed fifty passages cites the first five.
pub const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 25;

/// How long we wait on the search engine. SearXNG fans out to a dozen upstreams
/// and returns when they do, so this is a real ceiling rather than a formality —
/// and it must be well under a harness turn budget, because a search that
/// outlives the turn that asked for it is a timeout charged to the model.
const TIMEOUT_MS: Duration = Duration::from_millis(20_000);

/// ONE SEARCH'S TWO INJECTED EDGES, the same seams search.test.ts drives: the
/// HTTP call and the URL resolution. Both default to the real thing.
#[derive(Clone)]
pub struct SearchDeps {
    pub fetch: FetchFn,
    pub url: UrlFn,
}

pub type FetchFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<FetchReply, String>> + Send + Sync>;
pub type UrlFn = Arc<dyn Fn(PgPool) -> BoxFuture<'static, String> + Send + Sync>;

/// The narrowed fetch reply: search needs the status and the body text and
/// nothing else (the headers that matter are SearXNG's JSON, not its response
/// headers).
#[derive(Clone)]
pub struct FetchReply {
    pub status: u16,
    pub body: String,
}

/// The real edges: the operator-infrastructure client (`http()`) under a
/// per-request 20s timeout, and the env→setting→default URL precedence.
pub fn real_deps() -> SearchDeps {
    SearchDeps {
        fetch: Arc::new(|url: String| {
            Box::pin(async move {
                let res = http()
                    .get(&url)
                    .header("accept", "application/json")
                    .timeout(TIMEOUT_MS)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let status = res.status().as_u16();
                let body = res.text().await.map_err(|e| e.to_string())?;
                Ok(FetchReply { status, body })
            })
        }),
        url: Arc::new(|pg: PgPool| Box::pin(async move { search_url(&pg).await })),
    }
}

/// ONE SEARCH. Fails with a sentence a model can act on — the caller is a tool
/// handler and its error text goes straight into an agent's transcript, so
/// "error sending request" is a dead end and "the search service did not
/// answer" is not.
///
/// `limit` stays an f64 until the clamp because the TS one is a JS number all
/// the way to `slice(0, limit)`: a fractional 2.5 truncates to two results, and
/// a negative clamps up to one rather than wrapping a usize.
pub async fn search_web(
    pg: &PgPool,
    query: &str,
    limit: Option<f64>,
    deps: &SearchDeps,
) -> Result<Vec<SearchResult>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("a search needs a query".into());
    }
    let limit = (limit.unwrap_or(DEFAULT_LIMIT as f64))
        .min(MAX_LIMIT as f64)
        .max(1.0) as usize;
    let base = (deps.url)(pg.clone()).await;

    // URLSearchParams: space is `+`, everything else percent-encoded — which is
    // exactly `form_urlencoded`'s house style, so the request bytes match.
    let params = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", q)
        .append_pair("format", "json")
        .finish();
    let res = match (deps.fetch)(format!("{base}/search?{params}")).await {
        Ok(r) => r,
        Err(msg) => {
            // The parenthetical is the transport's own message; the sentence
            // around it is what the model reads. (Wording of the parenthetical
            // is the one place Rust's error text differs from Node's.)
            return Err(format!(
                "the search service at {base} did not answer ({msg}). Tell whoever asked \
                 that live search is unavailable right now rather than answering from memory."
            ));
        }
    };
    if res.status == 403 {
        // THE ONE MISCONFIGURATION EVERYONE HITS, named exactly, because the
        // generic "403" sends an operator looking for an auth problem that does
        // not exist.
        return Err(
            "the search service refused the JSON format (403). SearXNG ships with JSON off: \
             add `json` to `search.formats` in its settings.yml and restart it."
                .into(),
        );
    }
    if !(200..300).contains(&res.status) {
        return Err(format!("the search service answered {}", res.status));
    }
    // A 200 whose body is not JSON is an empty answer, not a failure — the TS
    // `.catch(() => null)` reads the same way.
    let body: Value = serde_json::from_str(&res.body).unwrap_or(Value::Null);
    Ok(results_from(&body).into_iter().take(limit).collect())
}

/// SearXNG's payload, narrowed. Defensive because the shape is an aggregate of
/// a dozen engines and a single one returning something odd must cost that
/// result, not the search.
pub fn results_from(body: &Value) -> Vec<SearchResult> {
    let Some(rows) = body.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let Some(r) = row.as_object() else {
            continue;
        };
        let url = r.get("url").and_then(Value::as_str).unwrap_or("");
        // A RESULT WITHOUT A URL CANNOT BE CITED, and an uncitable result in a
        // research pipeline is worse than one fewer result — `ungrounded_ref`
        // grounds every claim against these.
        if url.is_empty() || seen.contains(url) {
            continue;
        }
        seen.insert(url.to_string());
        out.push(SearchResult {
            title: r
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(url)
                .to_string(),
            url: url.to_string(),
            snippet: r
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            engine: r
                .get("engine")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        });
    }
    out
}

/// Is a search engine reachable at all? Used by the setup check and by the
/// admin surface, so "search is unavailable" is something an operator is told
/// once rather than discovering through a model's excuse.
///
/// "REACHABLE" MEANS "FINDS THINGS", NOT "ANSWERS THE PHONE", and the difference
/// is the whole reason this function needed changing. A SearXNG whose every
/// general engine is CAPTCHA-walled — the DEFAULT state of a fresh self-hosted
/// instance, see the engines block in docker/searxng/settings.template.yml —
/// returns HTTP 200 with valid JSON and `results: []`. A check that asked only
/// whether the call threw would report healthy while search found nothing for
/// anybody, and `platform_supply` would go on advertising a `web_search` tool
/// that could not search. A model handed an empty result set does not fail
/// loudly; it answers from memory in the same confident voice.
///
/// So a canary query that comes back EMPTY is a failure, and the sentence names
/// the actual cause rather than the symptom. A false negative here costs a
/// capability tag; a false positive costs an uncited research report.
pub async fn search_reachable(pg: &PgPool, deps: &SearchDeps) -> SearchReach {
    let url = (deps.url)(pg.clone()).await;
    match search_web(pg, CANARY, Some(1.0), deps).await {
        Ok(hits) if hits.is_empty() => {
            let error = Some(format!(
                "the search service at {url} answered but found nothing, which usually means \
                     every engine is refusing it. Ask it which: `curl -s '{url}/search?q=test&format=json' \
                     | jq .unresponsive_engines` (a fresh SearXNG defaults to engines that CAPTCHA a \
                     self-hosted instance)."
            ));
            SearchReach {
                ok: false,
                url,
                error,
            }
        }
        Ok(_) => SearchReach {
            ok: true,
            url,
            error: None,
        },
        Err(error) => SearchReach {
            ok: false,
            url,
            error: Some(error),
        },
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchReach {
    pub ok: bool,
    pub url: String,
    pub error: Option<String>,
}

/// THE CANARY QUERY, and it is deliberately a common word rather than "talaria".
/// A health check has to fail only when SEARCH is broken — a rare term that
/// genuinely has no hits on a small indie index would make a working instance
/// report itself down.
const CANARY: &str = "wikipedia";

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The lazy pool dials nothing — the url edge is always injected in these
    /// tests, so no query ever runs.
    fn lazy_pg() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://search-test@localhost:5432/search-test")
            .expect("a lazy pool connects to nothing")
    }

    /// search.test.ts's deps: a fixed URL and a scripted fetch, with every call
    /// recorded so a test can say what the client actually asked for.
    struct Scripted {
        reply: Result<FetchReply, String>,
        calls: Mutex<Vec<String>>,
    }

    impl Scripted {
        fn ok(status: u16, body: &str) -> Arc<Self> {
            Arc::new(Self {
                reply: Ok(FetchReply {
                    status,
                    body: body.into(),
                }),
                calls: Mutex::new(Vec::new()),
            })
        }
        fn dead(msg: &str) -> Arc<Self> {
            Arc::new(Self {
                reply: Err(msg.into()),
                calls: Mutex::new(Vec::new()),
            })
        }
        fn deps(self: &Arc<Self>) -> SearchDeps {
            let s = self.clone();
            SearchDeps {
                fetch: Arc::new(move |url: String| {
                    let s = s.clone();
                    Box::pin(async move {
                        s.calls.lock().unwrap().push(url);
                        s.reply.clone().map(|r| FetchReply {
                            status: r.status,
                            body: r.body.clone(),
                        })
                    })
                }),
                url: Arc::new(|_pg| Box::pin(async { "http://search.test".to_string() })),
            }
        }
    }

    #[test]
    fn results_from_drops_unciteable_rows_and_never_hands_back_a_null_snippet() {
        let body = serde_json::json!({ "results": [
            { "url": "https://a.test", "title": "A", "content": "the snippet", "engine": "bing" },
            { "url": "https://b.test", "engine": "mojeek" },
            { "title": "no url is not a result", "engine": "bing" },
            { "url": "https://a.test", "title": "duplicate url" },
            { "url": "", "title": "blank url" },
            "not an object",
            42
        ] });
        let got = results_from(&body);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].snippet, "the snippet");
        assert_eq!(got[0].engine, "bing");
        // Title falls back to the URL; snippet and engine fall back to '' and
        // 'unknown' — a null in the prompt position is the bug this prevents.
        assert_eq!(got[1].title, "https://b.test");
        assert_eq!(got[1].snippet, "");
        assert_eq!(got[1].engine, "mojeek");
        assert!(results_from(&serde_json::json!(null)).is_empty());
        assert!(results_from(&serde_json::json!({ "nope": true })).is_empty());
    }

    #[tokio::test]
    async fn a_dead_engine_is_a_sentence_the_model_can_act_on() {
        let s = Scripted::dead("connection refused");
        let err = search_web(&lazy_pg(), "node 24", None, &s.deps())
            .await
            .unwrap_err();
        assert!(
            err.starts_with(
                "the search service at http://search.test did not answer (connection refused)."
            ),
            "{err}"
        );
        assert!(err.ends_with("rather than answering from memory."), "{err}");
    }

    #[tokio::test]
    async fn the_403_names_the_one_misconfiguration_everyone_hits() {
        let s = Scripted::ok(403, "");
        let err = search_web(&lazy_pg(), "node 24", None, &s.deps())
            .await
            .unwrap_err();
        assert!(
            err.contains("refused the JSON format (403)")
                && err.contains("`json` to `search.formats`"),
            "{err}"
        );
        // Every other non-2xx is just its status.
        let s = Scripted::ok(504, "");
        assert_eq!(
            search_web(&lazy_pg(), "node 24", None, &s.deps())
                .await
                .unwrap_err(),
            "the search service answered 504"
        );
    }

    #[tokio::test]
    async fn a_blank_query_never_reaches_the_engine_and_the_limit_clamps() {
        let s = Scripted::ok(200, r#"{"results":[]}"#);
        assert_eq!(
            search_web(&lazy_pg(), "   ", None, &s.deps())
                .await
                .unwrap_err(),
            "a search needs a query"
        );
        assert!(s.calls.lock().unwrap().is_empty());
        // 50 asks for ten; -3 and 2.5 clamp/truncate the TS way.
        let body = format!(
            r#"{{"results":[{}]}}"#,
            (0..25)
                .map(|i| format!(r#"{{"url":"https://s{i}.test","title":"s"}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );
        let s = Scripted::ok(200, &body);
        assert_eq!(
            search_web(&lazy_pg(), "q", Some(50.0), &s.deps())
                .await
                .unwrap()
                .len(),
            25
        );
        assert_eq!(
            search_web(&lazy_pg(), "q", Some(-3.0), &s.deps())
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            search_web(&lazy_pg(), "q", Some(2.5), &s.deps())
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            search_web(&lazy_pg(), "q", None, &s.deps())
                .await
                .unwrap()
                .len(),
            10
        );
        // The request itself: form-encoded, `format=json`, nothing else.
        assert_eq!(
            s.calls.lock().unwrap()[0],
            "http://search.test/search?q=q&format=json"
        );
    }

    #[tokio::test]
    async fn reachable_distinguishes_dead_empty_and_working() {
        // DEAD: the transport error becomes the probe's error sentence.
        let s = Scripted::dead("fetch failed");
        let r = search_reachable(&lazy_pg(), &s.deps()).await;
        assert!(!r.ok);
        assert_eq!(r.url, "http://search.test");
        assert!(
            r.error
                .as_deref()
                .is_some_and(|e| e.starts_with("the search service at"))
        );

        // EMPTY 200: answers the phone, finds nothing — the CAPTCHA-walled
        // self-host, named for what it actually is.
        let s = Scripted::ok(200, r#"{"results":[]}"#);
        let r = search_reachable(&lazy_pg(), &s.deps()).await;
        assert!(!r.ok);
        assert!(
            r.error
                .as_deref()
                .is_some_and(|e| e.contains("unresponsive_engines")),
            "{r:?}"
        );

        // WORKING.
        let s = Scripted::ok(200, r#"{"results":[{"url":"https://wikipedia.org"}]}"#);
        let r = search_reachable(&lazy_pg(), &s.deps()).await;
        assert!(r.ok);
        assert_eq!(r.error, None);
        // And it asked for exactly one canary hit.
        assert_eq!(
            s.calls.lock().unwrap()[0],
            "http://search.test/search?q=wikipedia&format=json"
        );
    }

    #[test]
    fn form_encoding_matches_urlsearchparams_bytes() {
        let params = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("q", "node 24 & ?")
            .append_pair("format", "json")
            .finish();
        assert_eq!(params, "q=node+24+%26+%3F&format=json");
    }

    #[test]
    fn url_resolution_defaults_blanks_and_strips_one_slash() {
        // The env-empty case is the trap: set-but-empty is not nullish in TS, so
        // it suppresses the setting — here that arrives as Some("") resolving to
        // the default, which is the same observable behavior.
        assert_eq!(
            resolve_search_url(Some("http://env.test".into())),
            "http://env.test"
        );
        assert_eq!(
            resolve_search_url(Some("http://env.test/".into())),
            "http://env.test"
        );
        // ONE slash, not all of them: `//` is a path, and `replace(/\/$/, '')`
        // takes only the last.
        assert_eq!(
            resolve_search_url(Some("http://env.test//".into())),
            "http://env.test/"
        );
        assert_eq!(resolve_search_url(Some(String::new())), DEFAULT_URL);
        assert_eq!(resolve_search_url(Some("   ".into())), "   ");
        assert_eq!(resolve_search_url(None), DEFAULT_URL);
    }
}
