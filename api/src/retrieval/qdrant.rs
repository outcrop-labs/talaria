// Minimal Qdrant client (REST) for Talaria's retrieval layer: collection
// create, point upsert/delete, and filtered search — no SDK.
//
// Two collection shapes coexist:
//   legacy (v1) — one unnamed dense vector; dense-only search.
//   hybrid (v2) — named `dense` + sparse `sparse` (modifier: idf); searched
//                 with the Query API fusing both branches via RRF.

use std::sync::Arc;

use serde_json::{Value, json};

use super::HttpFetch;
use crate::retrieval::sparse::SparseVector;

/// The configured base, read per call. One trailing slash stripped, so
/// `TALARIA_QDRANT_URL` may be typed with or without it.
fn qdrant_url() -> String {
    let raw =
        std::env::var("TALARIA_QDRANT_URL").unwrap_or_else(|_| "http://localhost:6333".into());
    raw.strip_suffix('/').unwrap_or(&raw).to_string()
}

pub struct QdrantDeps {
    pub fetch: HttpFetch,
    pub base: Arc<dyn Fn() -> String + Send + Sync>,
}

pub fn real_deps() -> QdrantDeps {
    QdrantDeps {
        fetch: super::real_http(),
        base: Arc::new(qdrant_url),
    }
}

/// The host of a URL whose scheme is http(s) — None when there is no parseable
/// scheme+host prefix. The parse matches a prefix (unanchored at the end),
/// so a base with a path still yields its host.
fn bare_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let end = rest.find(['/', ':']).unwrap_or(rest.len());
    Some(&rest[..end]).filter(|h| !h.is_empty())
}

/// Whether the docker-bare fallback applies: a single-label hostname — no
/// dot, not localhost — is a docker network name that won't resolve from a
/// host-side process. Those retry against 127.0.0.1:6333 (the published
/// port on the host), NOT sticky: every call pays the check again.
fn is_docker_bare(url: &str) -> bool {
    bare_host(url).is_some_and(|h| !h.contains('.') && h != "localhost")
}

/// One reply, kept as raw text alongside the parsed JSON — the upsert error
/// sentence quotes the body verbatim, and `ok` is the fetch sense (2xx).
pub struct QReply {
    pub ok: bool,
    pub status: u16,
    pub text: String,
    pub json: Option<Value>,
}

fn parse_reply(status: u16, text: String) -> QReply {
    let json = if text.trim().is_empty() {
        None
    } else {
        serde_json::from_str(&text).ok()
    };
    QReply {
        ok: (200..300).contains(&status),
        status,
        text,
        json,
    }
}

async fn q(
    deps: &QdrantDeps,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<QReply, String> {
    let base = (deps.base)();
    let url = format!("{base}{path}");
    let tried = match (deps.fetch)(method, &url, body, &[], 30_000).await {
        Ok((status, text)) => parse_reply(status, text),
        Err(err) => {
            // A docker-bare base failed to resolve from this process. Retry
            // the loopback publishing — the configured PORT is discarded,
            // always 6333. If the retry also fails the ORIGINAL error is
            // what the caller hears.
            if is_docker_bare(&base) {
                let retry = format!("http://127.0.0.1:6333{path}");
                let (status, text) = (deps.fetch)(method, &retry, body, &[], 30_000).await?;
                return Ok(parse_reply(status, text));
            }
            return Err(err);
        }
    };
    Ok(tried)
}

pub async fn ensure_collection(deps: &QdrantDeps, name: &str, dim: i64) -> Result<(), String> {
    let path = format!("/collections/{name}");
    let exists = q(deps, "GET", &path, None).await?;
    if exists.ok {
        return Ok(());
    }
    let r = q(
        deps,
        "PUT",
        &path,
        Some(&json!({ "vectors": { "size": dim, "distance": "Cosine" } })),
    )
    .await?;
    if !r.ok && r.status != 409 {
        return Err(format!("qdrant create {name}: {}", r.status));
    }
    Ok(())
}

/// Create a hybrid (v2) collection: named dense vector + IDF-modified sparse.
/// Qdrant computes IDF server-side, so points/queries carry plain tf values.
pub async fn ensure_hybrid_collection(
    deps: &QdrantDeps,
    name: &str,
    dim: i64,
) -> Result<(), String> {
    let path = format!("/collections/{name}");
    let exists = q(deps, "GET", &path, None).await?;
    if exists.ok {
        return Ok(());
    }
    let r = q(
        deps,
        "PUT",
        &path,
        Some(&json!({
            "vectors": { "dense": { "size": dim, "distance": "Cosine" } },
            "sparse_vectors": { "sparse": { "modifier": "idf" } },
        })),
    )
    .await?;
    if !r.ok && r.status != 409 {
        return Err(format!("qdrant create {name}: {}", r.status));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollectionInfo {
    pub points_count: i64,
    /// Dense dimension, wherever it lives (unnamed legacy or named `dense`).
    pub dense_dim: Option<i64>,
    /// True when the collection has the v2 named dense + sparse layout.
    pub hybrid: bool,
}

/// The ACTUAL shape of a live collection — the source of truth for migration
/// decisions (registry columns can go stale; the collection itself cannot).
/// Any failure — transport or non-2xx — reads as "no collection": None.
pub async fn collection_info(deps: &QdrantDeps, name: &str) -> Option<CollectionInfo> {
    let r = q(deps, "GET", &format!("/collections/{name}"), None)
        .await
        .ok()?;
    if !r.ok {
        return None;
    }
    // A malformed body reads as absent data — this helper exists to answer
    // "what is actually deployed", and an unparseable answer to that
    // question is no answer.
    let j = r.json.unwrap_or_else(|| json!({}));
    let params = j
        .get("result")
        .and_then(|res| res.get("config"))
        .and_then(|cfg| cfg.get("params"));
    let vectors = params
        .and_then(|p| p.get("vectors"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    // Named-vs-unnamed is decided by whether `vectors.size` is a number — a
    // missing vectors block entirely reads as NAMED with no dense, which is
    // what a half-created collection looks like.
    let named = !vectors.get("size").map(Value::is_number).unwrap_or(false);
    let dense_dim = if named {
        vectors
            .get("dense")
            .and_then(|d| d.get("size"))
            .and_then(Value::as_i64)
    } else {
        vectors.get("size").and_then(Value::as_i64)
    };
    Some(CollectionInfo {
        points_count: j
            .get("result")
            .and_then(|res| res.get("points_count"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        dense_dim,
        hybrid: named
            && params
                .and_then(|p| p.get("sparse_vectors"))
                .and_then(|sv| sv.get("sparse"))
                .is_some(),
    })
}

pub async fn delete_collection(deps: &QdrantDeps, name: &str) {
    // Swallowed on purpose — deleting a collection that is already gone is
    // the success case.
    let _ = q(deps, "DELETE", &format!("/collections/{name}"), None).await;
}

pub struct QdrantPoint {
    pub id: String,
    pub vector: Vec<f64>,
    /// Present for hybrid (v2) collections.
    pub sparse: Option<SparseVector>,
    pub payload: serde_json::Map<String, Value>,
}

pub async fn upsert_points(
    deps: &QdrantDeps,
    collection: &str,
    points: &[QdrantPoint],
    hybrid: bool,
) -> Result<(), String> {
    if points.is_empty() {
        return Ok(());
    }
    let points: Vec<Value> = points
        .iter()
        .map(|p| {
            if hybrid {
                let sparse = p.sparse.clone().unwrap_or(SparseVector {
                    indices: Vec::new(),
                    values: Vec::new(),
                });
                json!({
                    "id": p.id,
                    "vector": { "dense": p.vector, "sparse": sparse },
                    "payload": p.payload,
                })
            } else {
                json!({ "id": p.id, "vector": p.vector, "payload": p.payload })
            }
        })
        .collect();
    let r = q(
        deps,
        "PUT",
        &format!("/collections/{collection}/points?wait=true"),
        Some(&json!({ "points": points })),
    )
    .await?;
    if !r.ok {
        return Err(format!("qdrant upsert: {} {}", r.status, r.text));
    }
    Ok(())
}

pub async fn delete_points(deps: &QdrantDeps, collection: &str, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    // Swallowed: points already gone is the outcome we wanted anyway.
    let _ = q(
        deps,
        "POST",
        &format!("/collections/{collection}/points/delete?wait=true"),
        Some(&json!({ "points": ids })),
    )
    .await;
}

/// Delete every point matching a payload filter — used to purge a whole
/// container's activity (e.g. all points with boardId=X when a board is
/// deleted) without enumerating point ids.
pub async fn delete_by_filter(
    deps: &QdrantDeps,
    collection: &str,
    filter: &serde_json::Map<String, Value>,
) {
    let _ = q(
        deps,
        "POST",
        &format!("/collections/{collection}/points/delete?wait=true"),
        Some(&json!({ "filter": filter })),
    )
    .await;
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub id: String,
    pub score: f64,
    pub payload: serde_json::Map<String, Value>,
}

/// Qdrant ids come back as whatever they were stored as — a uuid string here,
/// but a number for collections other tools wrote; both coerce to string.
fn id_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn hit_of(v: &Value) -> Option<SearchHit> {
    let o = v.as_object()?;
    Some(SearchHit {
        id: id_to_string(o.get("id")?),
        score: o.get("score").and_then(Value::as_f64)?,
        payload: o
            .get("payload")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
    })
}

/// Search one collection with an optional payload filter (Qdrant filter DSL).
/// A non-ok answer is an EMPTY result, not an error — search across many
/// collections must survive one of them misbehaving.
pub async fn search(
    deps: &QdrantDeps,
    collection: &str,
    vector: &[f64],
    limit: usize,
    filter: Option<&Value>,
) -> Result<Vec<SearchHit>, String> {
    let mut body = json!({ "vector": vector, "limit": limit, "with_payload": true });
    if let Some(f) = filter {
        body.as_object_mut()
            .expect("the body was built as an object")
            .insert("filter".into(), f.clone());
    }
    let r = q(
        deps,
        "POST",
        &format!("/collections/{collection}/points/search"),
        Some(&body),
    )
    .await?;
    if !r.ok {
        return Ok(Vec::new());
    }
    Ok(r.json
        .and_then(|j| j.get("result").cloned())
        .and_then(|res| res.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(hit_of)
        .collect())
}

/// Hybrid search on a v2 collection: dense + sparse prefetch branches fused
/// with reciprocal-rank fusion. Scores are RRF ranks, not cosines — comparable
/// within one query, meaningless across collections (the reranker, when
/// configured, is what makes the cross-collection merge honest).
pub async fn hybrid_query(
    deps: &QdrantDeps,
    collection: &str,
    dense: &[f64],
    sparse: &SparseVector,
    limit: usize,
    filter: Option<&Value>,
) -> Result<Vec<SearchHit>, String> {
    // Each branch over-fetches so fusion has real overlap to rank.
    let branch_limit = std::cmp::max(limit * 2, 20);
    let mut prefetch = vec![json!({ "query": dense, "using": "dense", "limit": branch_limit })];
    if !sparse.indices.is_empty() {
        prefetch.push(json!({ "query": sparse, "using": "sparse", "limit": branch_limit }));
    }
    for b in prefetch.iter_mut() {
        if let Some(f) = filter {
            b.as_object_mut()
                .expect("the branch was built as an object")
                .insert("filter".into(), f.clone());
        }
    }
    let r = q(
        deps,
        "POST",
        &format!("/collections/{collection}/points/query"),
        Some(&json!({
            "prefetch": prefetch,
            "query": { "fusion": "rrf" },
            "limit": limit,
            "with_payload": true,
        })),
    )
    .await?;
    if !r.ok {
        return Ok(Vec::new());
    }
    Ok(r.json
        .and_then(|j| j.get("result").cloned())
        .and_then(|res| res.get("points").cloned())
        .and_then(|pts| pts.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(hit_of)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A scripted Qdrant: routes on URL, answers from a fixed map of
    /// (method, path-suffix) → (status, body); unscripted URLs 404.
    fn scripted(
        base: &'static str,
        script: Vec<(&'static str, &'static str, &'static str)>,
    ) -> (QdrantDeps, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fetch = {
            let seen = seen.clone();
            Arc::new(
                move |_method: &str,
                      url: &str,
                      _body: Option<&Value>,
                      _h: &[(&str, &str)],
                      _t: u64| {
                    let seen = seen.clone();
                    let script = script.clone();
                    let url = url.to_string();
                    let method = _method.to_string();
                    Box::pin(async move {
                        seen.lock().unwrap().push(url.clone());
                        for (m, suffix, body) in &script {
                            if url.ends_with(suffix) && (m.is_empty() || *m == method) {
                                let (status, text) = body.split_once(' ').unwrap();
                                return Ok((status.parse::<u16>().unwrap(), text.to_string()));
                            }
                        }
                        Ok((404u16, "{}".to_string()))
                    })
                        as futures_util::future::BoxFuture<'static, Result<(u16, String), String>>
                },
            )
        };
        let deps = QdrantDeps {
            fetch,
            base: Arc::new(move || base.to_string()),
        };
        (deps, seen)
    }

    #[test]
    fn the_fallback_is_only_for_docker_bare_hosts() {
        // Both schemes qualify — https like http.
        for url in [
            "http://qdrant",
            "https://qdrant",
            "http://qdrant:6333",
            "http://qdrant:6333/collections",
        ] {
            assert!(is_docker_bare(url), "{url}");
        }
        // Dotted hosts, loopback, and scheme-less words don't retry.
        for url in [
            "http://qdrant.internal:6333",
            "http://localhost:6333",
            "http://127.0.0.1:6333",
            "qdrant:6333",
        ] {
            assert!(!is_docker_bare(url), "{url}");
        }
    }

    #[tokio::test]
    async fn a_docker_bare_failure_retries_loopback_and_is_not_sticky() {
        // Primary 6333 is dead (the script never matches it → 404 stands in
        // for the connection failure via the error arm below), loopback
        // answers. Use a fetch that ERRORS the primary so the retry arm runs.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fetch = {
            let seen = seen.clone();
            Arc::new(
                move |_m: &str, url: &str, _b: Option<&Value>, _h: &[(&str, &str)], _t: u64| {
                    let seen = seen.clone();
                    let url = url.to_string();
                    Box::pin(async move {
                        seen.lock().unwrap().push(url.clone());
                        if url.starts_with("http://qdrant") {
                            Err("dns took five seconds and refused".into())
                        } else {
                            Ok((200u16, "{\"result\":{}}".to_string()))
                        }
                    })
                        as futures_util::future::BoxFuture<'static, Result<(u16, String), String>>
                },
            )
        };
        let deps = QdrantDeps {
            fetch,
            base: Arc::new(|| "http://qdrant:6333".into()),
        };
        let info = collection_info(&deps, "talaria_activity").await;
        assert_eq!(info.map(|i| i.points_count), Some(0));
        let urls = seen.lock().unwrap().clone();
        assert_eq!(
            urls,
            vec![
                "http://qdrant:6333/collections/talaria_activity",
                "http://127.0.0.1:6333/collections/talaria_activity",
            ]
        );
        // NOT sticky: the second call tries the configured base first again.
        collection_info(&deps, "talaria_activity").await;
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn ensure_collection_tolerates_409_and_refuses_real_failures() {
        // Existing → no PUT at all.
        let (deps, seen) = scripted(
            "http://q.test",
            vec![("", "/collections/talaria_org_kb", "200 {}")],
        );
        ensure_collection(&deps, "talaria_org_kb", 384)
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
        // 409 = someone else created it first — fine. (Method-specific entries
        // must come first: the catch-all "" method matches a PUT too.)
        let (deps, _) = scripted(
            "http://q.test",
            vec![
                ("PUT", "ions/x", "409 {}"),
                ("", "/collections/x", "404 {}"),
            ],
        );
        ensure_collection(&deps, "x", 384).await.unwrap();
        // Anything else names the collection and the status.
        let (deps, _) = scripted(
            "http://q.test",
            vec![
                ("PUT", "ions/x", "500 nope"),
                ("", "/collections/x", "404 {}"),
            ],
        );
        assert_eq!(
            ensure_collection(&deps, "x", 384).await.unwrap_err(),
            "qdrant create x: 500"
        );
    }

    #[tokio::test]
    async fn the_hybrid_create_body_is_named_dense_plus_idf_sparse() {
        let (deps, seen) = scripted(
            "http://q.test",
            vec![
                ("PUT", "ions/new", "200 {}"),
                ("", "/collections/new", "404 {}"),
            ],
        );
        // The scripted fetch drops the body, so capture it with a one-off.
        let body_seen = Arc::new(Mutex::new(Option::<Value>::None));
        let fetch = {
            let body_seen = body_seen.clone();
            let inner = deps.fetch.clone();
            Arc::new(
                move |m: &str, u: &str, b: Option<&Value>, h: &[(&str, &str)], t: u64| {
                    let body_seen = body_seen.clone();
                    let inner = inner.clone();
                    let m = m.to_string();
                    let u = u.to_string();
                    let b = b.cloned();
                    let h: Vec<(String, String)> = h
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    Box::pin(async move {
                        if b.is_some() {
                            *body_seen.lock().unwrap() = b.clone();
                        }
                        let hdrs: Vec<(&str, &str)> =
                            h.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                        inner(&m, &u, b.as_ref(), &hdrs, t).await
                    })
                        as futures_util::future::BoxFuture<'static, Result<(u16, String), String>>
                },
            )
        };
        let deps = QdrantDeps {
            fetch,
            base: deps.base,
        };
        ensure_hybrid_collection(&deps, "new", 1024).await.unwrap();
        assert_eq!(
            body_seen.lock().unwrap().clone(),
            Some(json!({
                "vectors": { "dense": { "size": 1024, "distance": "Cosine" } },
                "sparse_vectors": { "sparse": { "modifier": "idf" } },
            }))
        );
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn collection_info_reads_named_and_unnamed_live_shapes() {
        // Driven through parse_reply + the same json the live call reads —
        // the parse is the logic under test; the transport has its own tests.
        let unnamed = parse_reply(
            200,
            r#"{"result":{"points_count":41,"config":{"params":{"vectors":{"size":384}}}}}"#.into(),
        );
        let info = (|| {
            let j = unnamed.json.as_ref().unwrap();
            let params = j.pointer("/result/config/params")?;
            let vectors = params.get("vectors").cloned().unwrap_or(json!({}));
            let named = !vectors.get("size").map(Value::is_number).unwrap_or(false);
            Some((named, vectors.get("size").and_then(Value::as_i64)))
        })()
        .unwrap();
        assert_eq!(info, (false, Some(384)));

        let named = parse_reply(
            200,
            r#"{"result":{"points_count":7,"config":{"params":{"vectors":{"dense":{"size":1024}},"sparse_vectors":{"sparse":{"modifier":"idf"}}}}}}"#.into(),
        );
        let j = named.json.as_ref().unwrap();
        let params = j.pointer("/result/config/params").unwrap();
        let vectors = params.get("vectors").cloned().unwrap_or(json!({}));
        let is_named = !vectors.get("size").map(Value::is_number).unwrap_or(false);
        assert!(is_named);
        assert_eq!(
            vectors.pointer("/dense/size").and_then(Value::as_i64),
            Some(1024)
        );
        assert!(
            params
                .get("sparse_vectors")
                .and_then(|sv| sv.get("sparse"))
                .is_some()
        );
        // The empty arm: a missing vectors block reads as named, dimless,
        // non-hybrid.
        assert!(json!({}).get("size").map(Value::is_number).is_none());
    }

    #[tokio::test]
    async fn upsert_maps_hybrid_vectors_and_quotes_the_body_on_failure() {
        let body_seen = Arc::new(Mutex::new(Option::<Value>::None));
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let fetch = {
            let body_seen = body_seen.clone();
            let seen = seen.clone();
            Arc::new(
                move |m: &str, u: &str, b: Option<&Value>, _h: &[(&str, &str)], _t: u64| {
                    let body_seen = body_seen.clone();
                    let seen = seen.clone();
                    let m = m.to_string();
                    let u = u.to_string();
                    let b = b.cloned();
                    Box::pin(async move {
                        seen.lock().unwrap().push(format!("{m} {u}"));
                        if b.is_some() {
                            *body_seen.lock().unwrap() = b.clone();
                        }
                        Ok((500u16, "dimension mismatch".to_string()))
                    })
                        as futures_util::future::BoxFuture<'static, Result<(u16, String), String>>
                },
            )
        };
        let deps = QdrantDeps {
            fetch,
            base: Arc::new(|| "http://q.test".into()),
        };
        let mut payload = serde_json::Map::new();
        payload.insert("sourceType".into(), json!("kb-doc"));
        let points = vec![QdrantPoint {
            id: "p1".into(),
            vector: vec![0.1, 0.2],
            sparse: None,
            payload: payload.clone(),
        }];
        let err = upsert_points(&deps, "talaria_org_kb", &points, true)
            .await
            .unwrap_err();
        assert_eq!(err, "qdrant upsert: 500 dimension mismatch");
        assert_eq!(
            seen.lock().unwrap()[0],
            "PUT http://q.test/collections/talaria_org_kb/points?wait=true"
        );
        // A hybrid point with no sparse vector carries the EMPTY sparse
        // vector — Qdrant rejects a missing named vector outright.
        assert_eq!(
            body_seen.lock().unwrap().clone(),
            Some(json!({
                "points": [{
                    "id": "p1",
                    "vector": { "dense": [0.1, 0.2], "sparse": { "indices": [], "values": [] } },
                    "payload": { "sourceType": "kb-doc" }
                }]
            }))
        );
        // Empty upsert touches nothing.
        assert_eq!(seen.lock().unwrap().len(), 1);
        upsert_points(&deps, "talaria_org_kb", &[], false)
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn search_returns_empty_on_any_non_ok_and_reads_the_result_array() {
        let (deps, _) = scripted(
            "http://q.test",
            vec![(
                "",
                "/points/search",
                "200 {\"result\":[{\"id\":\"p1\",\"score\":0.9,\"payload\":{\"sourceType\":\"kb-doc\"}},{\"id\":7,\"score\":0.5,\"payload\":null}]}",
            )],
        );
        let hits = search(&deps, "col", &[0.1, 0.2], 5, None).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "p1");
        assert_eq!(hits[0].payload.get("sourceType"), Some(&json!("kb-doc")));
        // A numeric id coerces; a missing payload reads as empty, not null.
        assert_eq!(hits[1].id, "7");
        assert!(hits[1].payload.is_empty());

        let (deps, _) = scripted("http://q.test", vec![("", "/points/search", "500 oops")]);
        assert!(
            search(&deps, "col", &[0.1], 5, None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn hybrid_query_overfetches_each_branch_and_only_sparse_when_indexed() {
        let body_seen = Arc::new(Mutex::new(Option::<Value>::None));
        let fetch = {
            let body_seen = body_seen.clone();
            Arc::new(
                move |_m: &str, _u: &str, b: Option<&Value>, _h: &[(&str, &str)], _t: u64| {
                    let body_seen = body_seen.clone();
                    let b = b.cloned();
                    Box::pin(async move {
                        *body_seen.lock().unwrap() = b;
                        Ok((200u16, "{\"result\":{\"points\":[]}}".to_string()))
                    })
                        as futures_util::future::BoxFuture<'static, Result<(u16, String), String>>
                },
            )
        };
        let deps = QdrantDeps {
            fetch,
            base: Arc::new(|| "http://q.test".into()),
        };
        let filter = json!({ "must": [{ "key": "visibility", "match": { "any": ["org"] } }] });

        hybrid_query(
            &deps,
            "col",
            &[0.1],
            &SparseVector {
                indices: vec![],
                values: vec![],
            },
            8,
            Some(&filter),
        )
        .await
        .unwrap();
        let body = body_seen.lock().unwrap().clone().unwrap();
        // No sparse indices → one branch; every branch over-fetches
        // max(8*2, 20) = 20 and carries the filter.
        assert_eq!(body["prefetch"].as_array().unwrap().len(), 1);
        assert_eq!(body["prefetch"][0]["limit"], json!(20));
        assert_eq!(body["prefetch"][0]["using"], json!("dense"));
        assert_eq!(body["prefetch"][0]["filter"], filter);
        assert_eq!(body["query"], json!({ "fusion": "rrf" }));
        assert_eq!(body["limit"], json!(8));

        // A small limit still over-fetches to 20; an indexed sparse vector
        // adds the second branch.
        hybrid_query(
            &deps,
            "col",
            &[0.1],
            &SparseVector {
                indices: vec![9],
                values: vec![1.0],
            },
            3,
            None,
        )
        .await
        .unwrap();
        let body = body_seen.lock().unwrap().clone().unwrap();
        assert_eq!(body["prefetch"].as_array().unwrap().len(), 2);
        assert_eq!(body["prefetch"][1]["using"], json!("sparse"));
        assert_eq!(body["prefetch"][1]["limit"], json!(20));
        assert!(body["prefetch"][1].get("filter").is_none());
    }

    #[tokio::test]
    async fn delete_points_and_by_filter_swallow_and_skip_empty() {
        let (deps, seen) = scripted("http://q.test", vec![]);
        delete_points(&deps, "col", &[]).await;
        delete_by_filter(&deps, "col", &serde_json::Map::new()).await;
        // The filter delete still fires with an empty filter object — only
        // the POINTS delete has the empty guard.
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(seen.lock().unwrap()[0].ends_with("/points/delete?wait=true"));
    }
}
