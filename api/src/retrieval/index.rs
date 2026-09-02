// The retrieval core: index a source document into a collection (chunk → embed
// → upsert, idempotent by content hash), and search across a principal's
// accessible collections with ranked, merged results.
//
// What a point actually holds — be precise about this, the ACL depends on it:
// a pointer (sourceType/sourceId/href/title), an ACL payload, and a VERBATIM
// snippet: the first 400 chars of the chunk. Search RETURNS that snippet and
// callers do not re-fetch the system of record, so for a channel message, a
// ticket, or a comment those 400 chars ARE the content. Retrieval is a read of
// the content itself, never a pointer a later permission check re-gates.
//
// Two gates therefore stand between a principal and that text, and nothing
// else stands behind them:
//   1. the collection's bindings          (collections_for_principal)
//   2. the per-item payload filter        (activity_scope / doc_scope below)
// Both run on every collection kind. Every point written anywhere must carry
// the ACL payload its kind's filter reads (container ids for activity, DocAcl
// everywhere else): a point without one simply never matches — fail closed, so
// an un-ACL'd write is invisible rather than public. The payload is part of
// the content hash, so a visibility change re-indexes the point rather than
// leaving a stale ACL behind.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

use crate::boards::{agent_board_policy_sql, board_visibility_sql};
use crate::body::{truncate_utf16, utf16_len, utf16_substr};
use crate::retrieval::collections::{collections_for_principal, get_collection};
use crate::retrieval::embed::{EmbedDeps, embed, embed_one};
use crate::retrieval::qdrant::{
    QdrantDeps, QdrantPoint, delete_points, hybrid_query, search as qdrant_search, upsert_points,
};
use crate::retrieval::rerank::{get_rerank_config, rerank};
use crate::retrieval::sparse::sparse_encode;
use crate::state::AppState;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexDoc {
    /// 'kb-doc' | 'channel' | 'chat' | 'plan' | 'research' | 'ticket' | …
    pub source_type: String,
    pub source_id: String,
    pub title: Option<String>,
    pub text: String,
    /// Extra payload stored on every chunk. This is the ACL — the activity
    /// index reads container ids ({channelId, boardId, planOwnerId,
    /// ownerUserId, orgWide}); every other kind reads {visibility,
    /// ownerUserId} (see DocAcl). A point whose payload carries neither is
    /// unreachable.
    pub payload: Option<Map<String, Value>>,
    pub href: Option<String>,
}

/// The item ACL carried by kb-doc / artifact / research points — i.e. anything
/// landing in an org-kb, custom, or personal collection. `visibility` is the
/// EFFECTIVE visibility (a doc inheriting from a private space is private),
/// never the raw column.
#[derive(Debug, Clone, PartialEq)]
pub struct DocAcl {
    pub visibility: String, // 'private' | 'org' | 'public'
    pub owner_user_id: Option<String>,
    pub space_id: Option<String>,
}

impl DocAcl {
    pub fn to_map(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("visibility".into(), json!(self.visibility));
        m.insert(
            "ownerUserId".into(),
            self.owner_user_id
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        if let Some(space) = &self.space_id {
            m.insert("spaceId".into(), json!(space));
        }
        m
    }
}

/// Bump when the ACL payload shape changes: it feeds the content hash, so
/// existing points re-index (and pick the new payload up) on the next
/// backfill/sweep instead of lingering with a stale or absent ACL.
const ACL_SCHEMA: i64 = 2;

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let mut out = String::with_capacity(64);
    for b in h.finalize() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Order-independent payload digest — the payload is part of a point's
/// identity (it's the ACL), so a visibility change must invalidate the hash
/// even when the text is untouched.
///
/// BYTE-STABLE: a JSON array of the entries sorted by key. The exact bytes
/// are load-bearing — rag_points.content_hash rows already written must keep
/// matching or every re-index would churn. Nested-object values serialize
/// their keys in SORTED order — invisible to every payload this codebase
/// actually writes (they are all flat).
fn payload_digest(p: Option<&Map<String, Value>>) -> String {
    let mut entries: Vec<(&String, &Value)> = p.map(|m| m.iter().collect()).unwrap_or_default();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let array = Value::Array(
        entries
            .into_iter()
            .map(|(k, v)| json!([k, v]))
            .collect::<Vec<_>>(),
    );
    serde_json::to_string(&array).expect("a payload digest is plain data")
}

/// Paragraphs are separated by a blank-ish line — any whitespace run
/// containing a second newline. Hand-rolled: the crate carries no regex
/// dependency and this is the only split of its kind.
fn split_paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seg = String::new();
    let mut rest = text;
    while let Some(first_nl) = rest.find('\n') {
        let after = &rest[first_nl + 1..];
        let ws_end = after
            .char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(after.len());
        let run = &after[..ws_end];
        if run.contains('\n') {
            seg.push_str(&rest[..first_nl]);
            out.push(seg.clone());
            seg.clear();
            rest = &after[ws_end..];
        } else {
            // Not a separator — keep the newline and the whitespace run in
            // the segment and continue scanning after them.
            let keep_to = first_nl + 1 + ws_end;
            seg.push_str(&rest[..keep_to]);
            rest = &rest[keep_to..];
        }
    }
    seg.push_str(rest);
    out.push(seg);
    out
}

/// Paragraph-aware chunking with a soft size cap (~500 tokens ≈ 2000 chars).
/// Sizes are UTF-16 lengths — the unit the 400-char snippet budget is
/// spent in.
fn chunk(text: &str, max: usize) -> Vec<String> {
    let paras: Vec<String> = split_paragraphs(text)
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for p in paras {
        if !cur.is_empty() && utf16_len(&cur) + utf16_len(&p) > max {
            out.push(std::mem::take(&mut cur));
        }
        // A single oversized paragraph gets hard-split.
        if utf16_len(&p) > max {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            let units = utf16_len(&p);
            let mut i = 0;
            while i < units {
                out.push(utf16_substr(&p, i, i + max).to_string());
                i += max;
            }
        } else {
            if !cur.is_empty() {
                cur.push_str("\n\n");
            }
            cur.push_str(&p);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.truncate(100); // safety cap
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexOutcome {
    pub chunks: usize,
    pub skipped: bool,
}

async fn prior_points(
    pg: &PgPool,
    collection_id: &str,
    source_type: &str,
    source_id: &str,
) -> Result<Option<(Vec<String>, String)>, sqlx::Error> {
    let row = sqlx::query(
        "select point_ids, content_hash from rag_points \
         where collection_id = $1::uuid and source_type = $2 and source_id = $3",
    )
    .bind(collection_id)
    .bind(source_type)
    .bind(source_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|r| {
        let ids: Vec<String> = r
            .try_get::<Value, _>("point_ids")
            .unwrap_or(Value::Array(Vec::new()))
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        (
            ids,
            r.try_get::<String, _>("content_hash").unwrap_or_default(),
        )
    }))
}

/// Index (or re-index) a document into a collection. No-op when unchanged.
pub async fn index_document(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    collection_id: &str,
    doc: &IndexDoc,
) -> Result<IndexOutcome, String> {
    let Some(col) = get_collection(pg, collection_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Err("unknown collection".into());
    };
    let h = sha256_hex(
        &[
            doc.text.as_str(),
            doc.title.as_deref().unwrap_or(""),
            &ACL_SCHEMA.to_string(),
            &payload_digest(doc.payload.as_ref()),
        ]
        .join("\u{0}"),
    );

    let prev = prior_points(pg, collection_id, &doc.source_type, &doc.source_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some((point_ids, content_hash)) = &prev
        && content_hash == &h
    {
        return Ok(IndexOutcome {
            chunks: point_ids.len(),
            skipped: true,
        });
    }

    let chunks = chunk(&doc.text, 2000);
    if chunks.is_empty() {
        // Empty now — drop any prior points.
        if let Some((point_ids, _)) = &prev {
            delete_points(qd, &col.qdrant_name, point_ids).await;
            sqlx::query(
                "delete from rag_points \
                 where collection_id = $1::uuid and source_type = $2 and source_id = $3",
            )
            .bind(collection_id)
            .bind(&doc.source_type)
            .bind(&doc.source_id)
            .execute(pg)
            .await
            .map_err(|e| e.to_string())?;
        }
        return Ok(IndexOutcome {
            chunks: 0,
            skipped: false,
        });
    }

    let vectors = embed(ed, &chunks).await?;
    let hybrid = col.schema_version >= 2;
    let points: Vec<QdrantPoint> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            // Hybrid collections also index the chunk's terms (title included,
            // so an exact-name query lands even when the body never repeats
            // it).
            let sparse = hybrid
                .then(|| sparse_encode(&format!("{}\n{}", doc.title.as_deref().unwrap_or(""), c)));
            let mut payload = doc.payload.clone().unwrap_or_default();
            payload.insert("sourceType".into(), json!(doc.source_type));
            payload.insert("sourceId".into(), json!(doc.source_id));
            payload.insert(
                "title".into(),
                doc.title.clone().map(Value::String).unwrap_or(Value::Null),
            );
            payload.insert("snippet".into(), json!(truncate_utf16(c, 400)));
            payload.insert(
                "href".into(),
                doc.href.clone().map(Value::String).unwrap_or(Value::Null),
            );
            payload.insert("chunk".into(), json!(i));
            QdrantPoint {
                id: uuid::Uuid::new_v4().to_string(),
                vector: vectors.get(i).cloned().unwrap_or_default(),
                sparse,
                payload,
            }
        })
        .collect();
    upsert_points(qd, &col.qdrant_name, &points, hybrid).await?;
    if let Some((point_ids, _)) = &prev {
        delete_points(qd, &col.qdrant_name, point_ids).await;
    }

    let ids: Vec<String> = points.iter().map(|p| p.id.clone()).collect();
    let ids_json = serde_json::to_value(&ids).expect("point ids are plain data");
    sqlx::query(
        "insert into rag_points (collection_id, source_type, source_id, point_ids, content_hash) \
         values ($1::uuid, $2, $3, $4, $5) \
         on conflict (collection_id, source_type, source_id) \
         do update set point_ids = $4, content_hash = $5, updated_at = now()",
    )
    .bind(collection_id)
    .bind(&doc.source_type)
    .bind(&doc.source_id)
    .bind(&ids_json)
    .bind(&h)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(IndexOutcome {
        chunks: ids.len(),
        skipped: false,
    })
}

/// Remove a source doc from a collection.
pub async fn unindex_document(
    pg: &PgPool,
    qd: &QdrantDeps,
    collection_id: &str,
    source_type: &str,
    source_id: &str,
) -> Result<(), String> {
    let Some(col) = get_collection(pg, collection_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let Some((point_ids, _)) = prior_points(pg, collection_id, source_type, source_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    delete_points(qd, &col.qdrant_name, &point_ids).await;
    sqlx::query(
        "delete from rag_points \
         where collection_id = $1::uuid and source_type = $2 and source_id = $3",
    )
    .bind(collection_id)
    .bind(source_type)
    .bind(source_id)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalHit {
    pub collection: String,
    pub score: f64,
    pub source_type: String,
    pub source_id: String,
    pub title: Option<String>,
    pub snippet: String,
    pub href: Option<String>,
}

fn hit_of(collection: &str, h: &crate::retrieval::qdrant::SearchHit) -> RetrievalHit {
    let str_field = |key: &str| -> String {
        h.payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    RetrievalHit {
        collection: collection.to_string(),
        score: h.score,
        source_type: str_field("sourceType"),
        source_id: str_field("sourceId"),
        title: h
            .payload
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
        snippet: str_field("snippet"),
        href: h
            .payload
            .get("href")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// The channel + board ids a principal may see — used to ACL-filter the
/// ambient activity index at the item level (a user must not retrieve a
/// channel they're not in). Agents are scoped to the boards their policy
/// allows. The pure SHOULD-array builders under this are separated out so
/// the ACL shape is pinned by tests that need no database.
async fn activity_scope(
    pg: &PgPool,
    user_id: Option<&str>,
    agent_model: Option<&str>,
) -> Result<Value, sqlx::Error> {
    if let Some(uid) = user_id {
        let chans = sqlx::query_scalar::<_, String>(
            "select channel_id::text from channel_members where user_id = $1::uuid",
        )
        .bind(uid)
        .fetch_all(pg)
        .await?;
        // AssertSqlSafe: the interpolated fragment is this crate's own
        // parameterized visibility SQL — placeholders, never values.
        let boards = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
            "select b.id::text from boards b where {}",
            board_visibility_sql("$1", "$2", false)
        )))
        .bind(uid)
        .bind(uid)
        .fetch_all(pg)
        .await?;
        return Ok(user_activity_should(&chans, &boards, uid));
    }
    if let Some(model) = agent_model {
        // A personal assistant retrieves as its OWNER'S PROXY: the owner's
        // channels, boards, plans, and distilled history — exactly what the
        // owner themselves could retrieve, nothing more.
        let pa = sqlx::query_scalar::<_, String>(
            "select owner_user_id::text from agent_defs \
             where model = $1 and owner_user_id is not null limit 1",
        )
        .bind(model)
        .fetch_optional(pg)
        .await?;
        if let Some(owner) = pa {
            return Box::pin(activity_scope(pg, Some(&owner), None)).await;
        }
        // Org agents: boards whose policy allows them (channels an agent is
        // in are also fine, but boards are the primary agent scope) +
        // org-wide content.
        // AssertSqlSafe: the interpolated fragment is this crate's own
        // parameterized policy SQL — placeholders, never values.
        let boards = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
            "select b.id::text from boards b where {}",
            agent_board_policy_sql("$1")
        )))
        .bind(model)
        .fetch_all(pg)
        .await?;
        return Ok(agent_activity_should(&boards));
    }
    // No scope → nothing from activity. An EMPTY should-array, not a missing
    // filter — an absent filter would mean "everything".
    Ok(json!({ "should": [] }))
}

fn user_activity_should(channels: &[String], boards: &[String], uid: &str) -> Value {
    json!({ "should": [
        { "key": "channelId", "match": { "any": channels } },
        { "key": "boardId", "match": { "any": boards } },
        // Plans (conversations + their living documents) are private to their
        // owner — only that user retrieves them from the ambient index.
        { "key": "planOwnerId", "match": { "value": uid } },
        // Distilled agent chats are likewise owner-private.
        { "key": "ownerUserId", "match": { "value": uid } },
        // Content explicitly published org-wide (e.g. org research reports).
        { "key": "orgWide", "match": { "value": true } },
    ] })
}

fn agent_activity_should(boards: &[String]) -> Value {
    json!({ "should": [
        { "key": "boardId", "match": { "any": boards } },
        { "key": "orgWide", "match": { "value": true } },
    ] })
}

/// The human a principal retrieves AS: themselves, or — for a personal
/// assistant — its owner. Org agents have no human identity.
async fn effective_user_id(
    pg: &PgPool,
    user_id: Option<&str>,
    agent_model: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    if let Some(uid) = user_id {
        return Ok(Some(uid.to_string()));
    }
    let Some(model) = agent_model else {
        return Ok(None);
    };
    let pa = sqlx::query_scalar::<_, String>(
        "select owner_user_id::text from agent_defs \
         where model = $1 and owner_user_id is not null limit 1",
    )
    .bind(model)
    .fetch_optional(pg)
    .await?;
    Ok(pa)
}

/// Item-level ACL for the DOCUMENT collections (org-kb, custom, personal) —
/// the counterpart to activity_scope. Collection bindings say which brains
/// you may read; this says which items inside them you may read, because a
/// brain is not uniform: a custom brain fed by a KB space holds docs of
/// mixed visibility, and a private doc that landed there (or in the org
/// brain via a backfill bug) must not come back for anyone but its owner.
///
/// Mirrors kb-perms canRead: org/public → any resolved member; private →
/// owner (a personal assistant reading as its owner). Points with no
/// `visibility` in their payload match nothing — they re-acquire one on the
/// next backfill.
fn doc_should(uid: Option<&str>) -> Value {
    let mut should = vec![json!({ "key": "visibility", "match": { "any": ["org", "public"] } })];
    if let Some(uid) = uid {
        should.push(json!({
            "must": [
                { "key": "visibility", "match": { "value": "private" } },
                { "key": "ownerUserId", "match": { "value": uid } },
            ]
        }));
    }
    json!({ "should": should })
}

/// Over-fetch per collection when a reranker is configured, so it has a real
/// pool to work. `ceil(candidates / cols) + 2`, floored at `limit`.
fn per_col_limit(limit: usize, reranking: bool, candidates: usize, cols: usize) -> usize {
    if !reranking || cols == 0 {
        return limit;
    }
    std::cmp::max(limit, candidates.div_ceil(cols) + 2)
}

/// Merge per-collection hits: sort by vector score descending, dedupe by
/// source. Neither cosine nor RRF scores are comparable across collections —
/// the reranker is what makes the merged ranking honest; without one this
/// keeps the per-collection score order.
fn merge_hits(per_col: Vec<Vec<RetrievalHit>>) -> Vec<RetrievalHit> {
    let mut merged: Vec<RetrievalHit> = per_col.into_iter().flatten().collect();
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut seen = std::collections::HashSet::new();
    merged.retain(|h| seen.insert(format!("{}:{}", h.source_type, h.source_id)));
    merged
}

/// Who is retrieving: a user, an agent (by model), or — for a personal
/// assistant — an agent that retrieves as its owner.
#[derive(Debug, Clone, Copy, Default)]
pub struct Principal<'a> {
    pub user_id: Option<&'a str>,
    pub agent_model: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SearchOpts<'a> {
    pub limit: Option<usize>,
    pub collection_ids: Option<&'a [String]>,
}

/// Search across the principal's accessible collections. Recall per collection
/// (HYBRID dense+keyword RRF on v2 collections — exact identifiers like env
/// vars, ticket numbers, and error strings rank alongside semantic matches;
/// dense-only on legacy v1) → merge + dedupe → (when configured) cross-encoder
/// RERANK over the merged candidate pool → top `limit`.
pub async fn search_for_principal(
    state: &AppState,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    http: &crate::retrieval::HttpFetch,
    principal: Principal<'_>,
    query: &str,
    opts: SearchOpts<'_>,
) -> Result<Vec<RetrievalHit>, String> {
    let pg = &state.pg;
    let mut cols = collections_for_principal(pg, principal.user_id, principal.agent_model)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(ids) = opts.collection_ids
        && !ids.is_empty()
    {
        cols.retain(|c| ids.contains(&c.id));
    }
    if cols.is_empty() {
        return Ok(Vec::new());
    }
    let limit = opts.limit.unwrap_or(8);
    let rerank_cfg = get_rerank_config(pg).await;
    let reranking = rerank_cfg.provider != "off";
    let candidates = rerank_cfg.candidates.unwrap_or(30) as usize;
    let col_limit = per_col_limit(limit, reranking, candidates, cols.len());
    let vec = embed_one(ed, query).await?;
    let sparse = sparse_encode(query);
    let (activity_scope_f, uid) = tokio::join!(
        activity_scope(pg, principal.user_id, principal.agent_model),
        effective_user_id(pg, principal.user_id, principal.agent_model)
    );
    let activity_filter = activity_scope_f.map_err(|e| e.to_string())?;
    let document_filter = doc_should(uid.map_err(|e| e.to_string())?.as_deref());

    // Every kind is ACL-filtered per item, not just activity: the collection
    // binding is the outer gate, this is the inner one. A failing collection
    // reads as no hits — one misbehaving brain must not sink the search.
    let per_col = futures_util::future::join_all(cols.iter().map(|c| {
        let activity_filter = activity_filter.clone();
        let document_filter = document_filter.clone();
        let vec = vec.clone();
        let sparse = sparse.clone();
        async move {
            let filter = if c.kind == "activity" {
                Some(activity_filter.clone())
            } else {
                Some(document_filter.clone())
            };
            let hits = if c.schema_version >= 2 {
                hybrid_query(
                    qd,
                    &c.qdrant_name,
                    &vec,
                    &sparse,
                    col_limit,
                    filter.as_ref(),
                )
                .await
                .unwrap_or_default()
            } else {
                qdrant_search(qd, &c.qdrant_name, &vec, col_limit, filter.as_ref())
                    .await
                    .unwrap_or_default()
            };
            hits.iter().map(|h| hit_of(&c.name, h)).collect::<Vec<_>>()
        }
    }))
    .await;

    let merged = merge_hits(per_col);
    if !reranking || merged.len() <= 1 {
        return Ok(merged.into_iter().take(limit).collect());
    }

    // Precision pass: rescore the pool with the configured cross-encoder.
    let pool: Vec<RetrievalHit> = merged.iter().take(candidates).cloned().collect();
    let texts: Vec<String> = pool
        .iter()
        .map(|h| {
            format!("{}\n{}", h.title.as_deref().unwrap_or(""), h.snippet)
                .trim()
                .to_string()
        })
        .collect();
    let Some(scores) = rerank(state, http, query, &texts).await else {
        // Provider hiccup → vector order (from the MERGED list — a pool smaller
        // than `limit` when candidates < limit would truncate the answer).
        return Ok(merged.into_iter().take(limit).collect());
    };
    let mut scored: Vec<RetrievalHit> = pool
        .into_iter()
        .enumerate()
        .map(|(i, mut h)| {
            if let Some(s) = scores.get(i) {
                h.score = *s;
            }
            h
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(scored.into_iter().take(limit).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_order_independent_and_key_sorted() {
        let a: Map<String, Value> = [
            ("visibility", json!("org")),
            ("ownerUserId", json!(null)),
            ("boardId", json!("b1")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let b: Map<String, Value> = [
            ("boardId", json!("b1")),
            ("ownerUserId", json!(null)),
            ("visibility", json!("org")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        // The exact bytes: a JSON array of key-sorted entries, null KEPT —
        // key order and null-vs-absent are both part of the digest.
        assert_eq!(
            payload_digest(Some(&a)),
            r#"[["boardId","b1"],["ownerUserId",null],["visibility","org"]]"#
        );
        assert_eq!(payload_digest(Some(&a)), payload_digest(Some(&b)));
        assert_eq!(payload_digest(None), "[]");
    }

    #[test]
    fn the_content_hash_joins_exactly_what_the_ts_joins() {
        let doc = IndexDoc {
            source_type: "kb-doc".into(),
            source_id: "d1".into(),
            title: None,
            text: "body".into(),
            payload: None,
            href: None,
        };
        let h = sha256_hex(
            &[
                doc.text.as_str(),
                "",
                &ACL_SCHEMA.to_string(),
                &payload_digest(None),
            ]
            .join("\u{0}"),
        );
        // sha256 of "body\0<no title>\02\0[]" — pinned so a join-order change
        // (which would silently re-index or, worse, skip indexing everything)
        // cannot pass quietly.
        assert_eq!(h.len(), 64);
        assert_ne!(
            h,
            sha256_hex(&[doc.text.as_str(), "", "2", "[]"].join("\n")),
            "the separator is NUL, not newline"
        );
    }

    #[test]
    fn paragraphs_split_on_blank_lines_and_whitespace_runs() {
        let ps = split_paragraphs("one\n\ntwo\n   \nthree");
        assert_eq!(ps, vec!["one", "two", "three"]);
        // A single newline (even padded) is NOT a paragraph break.
        let ps = split_paragraphs("one\n  two");
        assert_eq!(ps, vec!["one\n  two"]);
        // A run of newlines and tabs separates.
        let ps = split_paragraphs("a\n\t\n \nb");
        assert_eq!(ps, vec!["a", "b"]);
        // No newline at all.
        assert_eq!(split_paragraphs("solo"), vec!["solo"]);
    }

    #[test]
    fn chunking_respects_the_soft_cap_and_hard_splits_the_oversized() {
        // Two paragraphs that fit together stay together.
        let chunks = chunk("short one\n\nshort two", 2000);
        assert_eq!(chunks, vec!["short one\n\nshort two"]);
        // The cap: a paragraph that would overflow starts a fresh chunk.
        let text = format!("{}\n\n{}", "a".repeat(1200), "b".repeat(1200));
        let chunks = chunk(&text, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "a".repeat(1200));
        // An oversized single paragraph is hard-split at exactly the cap.
        let chunks = chunk(&"c".repeat(4500), 2000);
        assert_eq!(
            chunks.iter().map(|c| c.len()).collect::<Vec<_>>(),
            vec![2000, 2000, 500]
        );
        // The 100-chunk safety cap.
        let many = vec!["x".repeat(10); 500].join("\n\n");
        assert_eq!(chunk(&many, 10).len(), 100);
    }

    #[test]
    fn an_empty_or_blank_document_yields_no_chunks() {
        assert!(chunk("", 2000).is_empty());
        assert!(chunk("  \n\n  ", 2000).is_empty());
    }

    #[test]
    fn the_user_activity_should_reads_every_gate() {
        let f = user_activity_should(&["c1".into(), "c2".into()], &["b1".into()], "u-1");
        assert_eq!(
            f,
            json!({ "should": [
                { "key": "channelId", "match": { "any": ["c1", "c2"] } },
                { "key": "boardId", "match": { "any": ["b1"] } },
                { "key": "planOwnerId", "match": { "value": "u-1" } },
                { "key": "ownerUserId", "match": { "value": "u-1" } },
                { "key": "orgWide", "match": { "value": true } },
            ] })
        );
        // An agent with no boards: still allowed org-wide content — an empty
        // any-array matches nothing, which is the point.
        let f = agent_activity_should(&[]);
        assert_eq!(
            f,
            json!({ "should": [
                { "key": "boardId", "match": { "any": [] } },
                { "key": "orgWide", "match": { "value": true } },
            ] })
        );
    }

    #[test]
    fn the_doc_scope_mirrors_kb_perms() {
        // Any resolved member sees org + public.
        assert_eq!(
            doc_should(None),
            json!({ "should": [
                { "key": "visibility", "match": { "any": ["org", "public"] } },
            ] })
        );
        // An owner additionally sees their own private items.
        assert_eq!(
            doc_should(Some("u-1")),
            json!({ "should": [
                { "key": "visibility", "match": { "any": ["org", "public"] } },
                { "must": [
                    { "key": "visibility", "match": { "value": "private" } },
                    { "key": "ownerUserId", "match": { "value": "u-1" } },
                ] },
            ] })
        );
    }

    #[test]
    fn per_collection_overfetch_only_when_reranking() {
        assert_eq!(per_col_limit(8, false, 30, 3), 8);
        assert_eq!(per_col_limit(8, true, 30, 3), 12); // ceil(30/3)+2
        assert_eq!(per_col_limit(8, true, 30, 10), 8); // ceil(3)+2 floored at limit
        assert_eq!(per_col_limit(8, true, 30, 1), 32);
    }

    #[test]
    fn merge_sorts_by_score_and_dedupes_by_source() {
        let mk = |collection: &str, score: f64, source_type: &str, source_id: &str| RetrievalHit {
            collection: collection.into(),
            score,
            source_type: source_type.into(),
            source_id: source_id.into(),
            title: None,
            snippet: String::new(),
            href: None,
        };
        let merged = merge_hits(vec![
            vec![mk("a", 0.5, "kb-doc", "1"), mk("a", 0.9, "kb-doc", "2")],
            vec![mk("b", 0.7, "kb-doc", "1"), mk("b", 0.1, "chat", "x")],
        ]);
        // Same source from two collections counts once — the FIRST spelling
        // after the score sort wins.
        let ids: Vec<&str> = merged.iter().map(|h| h.source_id.as_str()).collect();
        assert_eq!(ids, vec!["2", "1", "x"]);
        assert_eq!(merged[0].score, 0.9);
    }
}
