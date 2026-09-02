// Embeddings client — talks to the self-hosted TEI (text-embeddings-inference)
// service. Configurable URL; the docker-hostname → localhost fallback lets it
// work whether Talaria runs on the host (dev) or on ai_default (prod).

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::HttpFetch;

const EMBED_FALLBACK: &str = "http://127.0.0.1:8055";

/// The configured base, read per call; one trailing slash stripped.
fn embed_url() -> String {
    let raw = std::env::var("TALARIA_EMBED_URL").unwrap_or_else(|_| "http://embeddings:80".into());
    raw.strip_suffix('/').unwrap_or(&raw).to_string()
}

/// THE STICKY BASE — process state. The configured URL may be a
/// docker-internal hostname that doesn't resolve from the host (dev);
/// resolving it FAILS SLOWLY — multi-second DNS timeouts — so remember which
/// base actually answered and go straight there afterwards. Without this,
/// every single embed call (search, indexing, health probes) paid the stall
/// before falling back.
///
/// The real edge shares ONE sticky across every `real_deps()` call (a
/// process-wide LazyLock); tests inject their own so they cannot fight over
/// the process's memory of what answered.
static REAL_STICKY: std::sync::LazyLock<Arc<Mutex<Option<String>>>> =
    std::sync::LazyLock::new(|| Arc::new(Mutex::new(None)));

/// The probed dimension, cached for the process.
static REAL_DIM: std::sync::LazyLock<Arc<std::sync::atomic::AtomicUsize>> =
    std::sync::LazyLock::new(|| Arc::new(std::sync::atomic::AtomicUsize::new(0)));

pub struct EmbedDeps {
    pub fetch: HttpFetch,
    pub base: Arc<dyn Fn() -> String + Send + Sync>,
    pub sticky: Arc<Mutex<Option<String>>>,
    pub dim_cache: Arc<std::sync::atomic::AtomicUsize>,
}

pub fn real_deps() -> EmbedDeps {
    EmbedDeps {
        fetch: super::real_http(),
        base: Arc::new(embed_url),
        sticky: REAL_STICKY.clone(),
        dim_cache: REAL_DIM.clone(),
    }
}

/// Whether the docker-bare fallback applies. The parse is ANCHORED here
/// (unlike qdrant's): a base carrying a path never falls back, and the
/// port, when present, must be digits.
fn is_docker_bare(base: &str) -> bool {
    let Some(rest) = base
        .strip_prefix("http://")
        .or_else(|| base.strip_prefix("https://"))
    else {
        return false;
    };
    if rest.contains('/') {
        return false;
    }
    let mut parts = rest.splitn(2, ':');
    let host = parts.next().unwrap_or("");
    let port = parts.next();
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    !host.is_empty() && !host.contains('.') && host != "localhost" && port_ok
}

/// Fetch against TEI with the hostname fallback, remembering the winner.
/// On success the attempted base becomes sticky; on failure a docker-bare
/// base retries the loopback publishing (and THAT becomes sticky when it
/// answers); anything else FORGETS the base that stopped answering — a sticky
/// base that no longer resolves is worse than paying the probe again.
async fn embed_fetch(
    deps: &EmbedDeps,
    timeout_ms: u64,
    path: &str,
    method: &str,
    body: Option<&Value>,
) -> Result<(u16, String), String> {
    let primary = (deps.base)();
    let sticky = deps
        .sticky
        .lock()
        .expect("the sticky base is never held across await")
        .clone();
    let base = sticky.unwrap_or_else(|| primary.clone());
    match (deps.fetch)(method, &format!("{base}{path}"), body, &[], timeout_ms).await {
        Ok(r) => {
            *deps
                .sticky
                .lock()
                .expect("the sticky base is never held across await") = Some(base);
            Ok(r)
        }
        Err(err) => {
            // Bare docker hostnames don't resolve from the host; TEI is
            // published on 127.0.0.1:8055 there. Only fall back when the base
            // looks docker-bare (and isn't already the fallback — the
            // infinite-loop guard).
            if is_docker_bare(&base) && base != EMBED_FALLBACK {
                let r = (deps.fetch)(
                    method,
                    &format!("{EMBED_FALLBACK}{path}"),
                    body,
                    &[],
                    timeout_ms,
                )
                .await?;
                *deps
                    .sticky
                    .lock()
                    .expect("the sticky base is never held across await") =
                    Some(EMBED_FALLBACK.into());
                return Ok(r);
            }
            *deps
                .sticky
                .lock()
                .expect("the sticky base is never held across await") = None;
            Err(err)
        }
    }
}

/// Embed one or many texts. TEI returns a vector per input.
pub async fn embed(deps: &EmbedDeps, inputs: &[String]) -> Result<Vec<Vec<f64>>, String> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let (status, text) = embed_fetch(
        deps,
        30_000,
        "/embed",
        "POST",
        Some(&json!({ "inputs": inputs, "truncate": true })),
    )
    .await?;
    if !(200..300).contains(&status) {
        return Err(format!("embeddings {status}"));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// One text in, one vector out — an empty array back from the service is an
/// explicit error rather than a silent nothing; callers treat embed failure
/// as "indexing didn't happen" either way.
pub async fn embed_one(deps: &EmbedDeps, text: &str) -> Result<Vec<f64>, String> {
    Ok(embed(deps, &[text.to_string()])
        .await?
        .into_iter()
        .next()
        .ok_or("the embedding service returned no vector")?)
}

/// The model's vector dimension, probed once per process.
pub async fn embed_dim(deps: &EmbedDeps) -> Result<usize, String> {
    let cached = deps.dim_cache.load(std::sync::atomic::Ordering::Relaxed);
    if cached != 0 {
        return Ok(cached);
    }
    let dim = embed_one(deps, "dimension probe").await?.len();
    deps.dim_cache
        .store(dim, std::sync::atomic::Ordering::Relaxed);
    Ok(dim)
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbedInfo {
    pub model_id: String,
    pub dim: usize,
}

/// What the embedding service is serving RIGHT NOW (TEI /info + a live dim
/// probe — never the process cache, since migration decisions hang on it).
/// Any failure reads as "unknown": None.
pub async fn embed_info(deps: &EmbedDeps) -> Option<EmbedInfo> {
    let (status, text) = embed_fetch(deps, 5_000, "/info", "GET", None).await.ok()?;
    let j: Value = if (200..300).contains(&status) {
        serde_json::from_str(&text).ok()?
    } else {
        json!({})
    };
    // The dim comes from a LIVE embed call, deliberately not the cache: a
    // restarted service serving a different model must report differently.
    let dim = embed(deps, &["dimension probe".to_string()])
        .await
        .ok()?
        .first()?
        .len();
    Some(EmbedInfo {
        model_id: j
            .get("model_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        dim,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn deps(
        base: &'static str,
        script: Vec<(&'static str, u16, &'static str)>, // (url-prefix, status, body)
    ) -> (EmbedDeps, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fetch = {
            let seen = seen.clone();
            Arc::new(
                move |_m: &str, url: &str, _b: Option<&Value>, _h: &[(&str, &str)], _t: u64| {
                    let seen = seen.clone();
                    let script = script.clone();
                    let url = url.to_string();
                    Box::pin(async move {
                        seen.lock().unwrap().push(url.clone());
                        for (prefix, status, body) in &script {
                            if url.starts_with(prefix) {
                                return Ok((*status, body.to_string()));
                            }
                        }
                        Err("connection refused".to_string())
                    })
                        as futures_util::future::BoxFuture<'static, Result<(u16, String), String>>
                },
            )
        };
        let d = EmbedDeps {
            fetch,
            base: Arc::new(move || base.to_string()),
            sticky: Arc::new(Mutex::new(None)),
            dim_cache: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        (d, seen)
    }

    #[test]
    fn the_fallback_needs_a_bare_docker_host_and_no_path() {
        for base in ["http://embeddings", "http://embeddings:80"] {
            assert!(is_docker_bare(base), "{base}");
        }
        // The anchor: a path disqualifies; dotted hosts, loopback, and other
        // schemes never fall back. A non-digit port fails the parse too.
        for base in [
            "http://embeddings:80/x",
            "http://tei.internal:80",
            "http://localhost",
            "http://127.0.0.1:8055",
            "embeddings:80",
            "http://embeddings:x",
        ] {
            assert!(!is_docker_bare(base), "{base}");
        }
    }

    #[tokio::test]
    async fn an_answering_primary_becomes_sticky_and_skips_the_probe_forever() {
        let (d, seen) = deps(
            "http://embeddings:80",
            vec![("http://embeddings:80", 200, "[[0.1,0.2]]")],
        );
        embed_one(&d, "first").await.unwrap();
        embed_one(&d, "second").await.unwrap();
        // Both went straight to the primary; the sticky memory recorded it.
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(
            *d.sticky.lock().unwrap(),
            Some("http://embeddings:80".into())
        );
    }

    #[tokio::test]
    async fn a_dead_docker_bare_base_falls_back_and_sticks_to_it() {
        // Only the loopback publishing answers.
        let (d, seen) = deps(
            "http://embeddings:80",
            vec![(EMBED_FALLBACK, 200, "[[0.5]]")],
        );
        embed_one(&d, "first").await.unwrap();
        // First call: primary (fails) then fallback. Second: straight there.
        embed_one(&d, "second").await.unwrap();
        let urls = seen.lock().unwrap().clone();
        assert_eq!(
            urls,
            vec![
                "http://embeddings:80/embed".to_string(),
                format!("{EMBED_FALLBACK}/embed"),
                format!("{EMBED_FALLBACK}/embed"),
            ]
        );
        assert_eq!(*d.sticky.lock().unwrap(), Some(EMBED_FALLBACK.into()));
    }

    #[tokio::test]
    async fn a_base_that_stops_answering_is_forgotten() {
        // Nothing answers at all.
        let (d, _) = deps("http://embeddings:80", vec![]);
        assert!(embed_one(&d, "x").await.is_err());
        assert_eq!(*d.sticky.lock().unwrap(), None);
        // And a NON-docker-bare base that fails never falls back — one
        // attempt, straight to the error.
        let (d, seen) = deps("http://127.0.0.1:8055", vec![]);
        assert!(embed_one(&d, "x").await.is_err());
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn embed_posts_the_truncate_body_and_names_the_status_on_failure() {
        let bodies = Arc::new(Mutex::new(Vec::<Option<Value>>::new()));
        let fetch = {
            let bodies = bodies.clone();
            Arc::new(
                move |_m: &str, _u: &str, b: Option<&Value>, _h: &[(&str, &str)], _t: u64| {
                    let bodies = bodies.clone();
                    let b = b.cloned();
                    Box::pin(async move {
                        bodies.lock().unwrap().push(b);
                        Ok((503u16, "overloaded".to_string()))
                    })
                        as futures_util::future::BoxFuture<'static, Result<(u16, String), String>>
                },
            )
        };
        let d = EmbedDeps {
            fetch,
            base: Arc::new(|| "http://embeddings:80".into()),
            sticky: Arc::new(Mutex::new(None)),
            dim_cache: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        assert_eq!(
            embed(&d, &["a".into(), "b".into()]).await.unwrap_err(),
            "embeddings 503"
        );
        assert_eq!(
            bodies.lock().unwrap()[0],
            Some(json!({ "inputs": ["a", "b"], "truncate": true }))
        );
        // Empty input is answered locally, no call at all.
        assert_eq!(embed(&d, &[]).await.unwrap(), Vec::<Vec<f64>>::new());
        assert_eq!(bodies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_dimension_is_probed_once_and_cached_for_the_process() {
        let (d, seen) = deps(
            "http://embeddings:80",
            vec![("http://embeddings:80", 200, "[[0.1,0.2,0.3]]")],
        );
        assert_eq!(embed_dim(&d).await.unwrap(), 3);
        assert_eq!(embed_dim(&d).await.unwrap(), 3);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn info_reads_the_model_id_and_a_live_dimension() {
        let (d, seen) = deps(
            "http://embeddings:80",
            vec![
                (
                    "http://embeddings:80/info",
                    200,
                    "{\"model_id\":\"bge-small-en-v1.5\"}",
                ),
                ("http://embeddings:80/embed", 200, "[[0.1,0.2]]"),
            ],
        );
        let info = embed_info(&d).await.unwrap();
        assert_eq!(
            info,
            EmbedInfo {
                model_id: "bge-small-en-v1.5".into(),
                dim: 2
            }
        );
        // The live probe never touched the process cache…
        assert_eq!(d.dim_cache.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(seen.lock().unwrap().len(), 2);
        // …and a failed info probe answers None without the embed leg.
        let (d, seen) = deps("http://embeddings:80", vec![]);
        assert!(embed_info(&d).await.is_none());
        assert_eq!(seen.lock().unwrap().len(), 2); // primary + fallback both refused
        // A non-ok /info still reports a dimension with an unknown model.
        let (d, _) = deps(
            "http://embeddings:80",
            vec![
                ("http://embeddings:80/info", 404, "nope"),
                ("http://embeddings:80", 200, "[[0.1]]"),
            ],
        );
        assert_eq!(embed_info(&d).await.unwrap().model_id, "unknown");
    }
}
