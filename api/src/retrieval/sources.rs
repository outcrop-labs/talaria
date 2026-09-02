// Convenience indexers that target the auto collections by kind, so callers
// (channel posts, KB doc saves, and so on) don't resolve collection ids
// themselves.
//
// All fire-and-forget friendly — indexing must never block the write it
// follows. The index/unindex legs swallow their errors (`let _ =`), while
// the resolution lookups (auto_collection_id & co.) PROPAGATE theirs — the
// surrounding fire-and-forget belongs to the caller, not this module.

use serde_json::{Map, json};
use sqlx::{PgPool, Row};

use crate::retrieval::collections::{
    PersonalOpts, RagCollection, ensure_auto_collections, ensure_personal_collection,
    personal_collection_for,
};
use crate::retrieval::embed::EmbedDeps;
use crate::retrieval::index::{DocAcl, IndexDoc, index_document, unindex_document};
use crate::retrieval::qdrant::{QdrantDeps, delete_by_filter};

/// The auto collection of a kind ('activity' | 'org-kb'), ensuring the auto
/// pair exists on a miss (first-ever write after boot, or the health sweep is
/// late) and re-reading. None when it still doesn't exist — the caller skips.
async fn auto_collection_id(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    kind: &str,
) -> Result<Option<String>, String> {
    let select = || async {
        sqlx::query_scalar::<_, String>(
            "select id::text from rag_collections where kind = $1 and auto limit 1",
        )
        .bind(kind)
        .fetch_optional(pg)
        .await
        .map_err(|e| e.to_string())
    };
    if let Some(id) = select().await? {
        return Ok(Some(id));
    }
    // The ensure is best-effort: embeddings being down must not turn a
    // fire-and-forget index call into an error.
    let _ = ensure_auto_collections(pg, qd, ed).await;
    select().await
}

pub async fn index_activity(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    doc: &IndexDoc,
) -> Result<(), String> {
    let Some(id) = auto_collection_id(pg, qd, ed, "activity").await? else {
        return Ok(());
    };
    let _ = index_document(pg, qd, ed, &id, doc).await;
    Ok(())
}

/// Index into the org brain. Everything here is org-visible by construction —
/// stamp it so the document-scope filter can see that (a point with no
/// visibility matches nothing). An explicit acl (org research reports carry
/// one) overrides the default.
pub async fn index_org_kb(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    doc: &IndexDoc,
    acl: Option<DocAcl>,
) -> Result<(), String> {
    let Some(id) = auto_collection_id(pg, qd, ed, "org-kb").await? else {
        return Ok(());
    };
    let acl = acl.unwrap_or(DocAcl {
        visibility: "org".into(),
        owner_user_id: None,
        space_id: None,
    });
    let mut doc = doc.clone();
    let mut payload = doc.payload.take().unwrap_or_default();
    for (k, v) in acl.to_map() {
        payload.insert(k, v);
    }
    doc.payload = Some(payload);
    let _ = index_document(pg, qd, ed, &id, &doc).await;
    Ok(())
}

/// Index into a user's PRIVATE brain — their personal collection, retrievable
/// only by them and their personal assistant. Created lazily (binding the PA
/// when they have one), so history starts landing without any setup step. The
/// item ACL is stamped here too: the collection binding already limits the
/// brain to owner + PA, and the payload says the same thing at item level, so
/// a mis-bound brain still can't hand a private item to a stranger.
///
/// Unlike its siblings the WHOLE body is swallowed — a user without a
/// personal collection yet must never fail the write that triggered
/// indexing.
pub async fn index_personal(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    user_id: &str,
    doc: &IndexDoc,
) {
    let _ = index_personal_inner(pg, qd, ed, user_id, doc).await;
}

async fn index_personal_inner(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    user_id: &str,
    doc: &IndexDoc,
) -> Result<(), String> {
    let pa = sqlx::query_scalar::<_, String>(
        "select model from agent_defs where owner_user_id = $1::uuid limit 1",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    let col = ensure_personal_collection(
        pg,
        qd,
        ed,
        user_id,
        &PersonalOpts {
            name: None,
            agent_model: pa.as_deref(),
        },
    )
    .await?;
    let acl = DocAcl {
        visibility: "private".into(),
        owner_user_id: Some(user_id.to_string()),
        space_id: None,
    };
    let mut doc = doc.clone();
    let mut payload = doc.payload.take().unwrap_or_default();
    for (k, v) in acl.to_map() {
        payload.insert(k, v);
    }
    doc.payload = Some(payload);
    index_document(pg, qd, ed, &col.id, &doc).await.map(|_| ())
}

pub async fn unindex_personal(
    pg: &PgPool,
    qd: &QdrantDeps,
    user_id: &str,
    source_type: &str,
    source_id: &str,
) -> Result<(), String> {
    let Some(col) = personal_collection_for(pg, user_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let _ = unindex_document(pg, qd, &col.id, source_type, source_id).await;
    Ok(())
}

pub async fn unindex_org_kb(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    source_type: &str,
    source_id: &str,
) -> Result<(), String> {
    let Some(id) = auto_collection_id(pg, qd, ed, "org-kb").await? else {
        return Ok(());
    };
    let _ = unindex_document(pg, qd, &id, source_type, source_id).await;
    Ok(())
}

// ── KB doc ↔ RAG sync ───────────────────────────────────────────────────────
// A doc lives in exactly one RAG collection, decided by its visibility:
//   private              → the owner's personal collection (owner + their agent only)
//   org/public + official → the org brain (grounds everyone)
//   org/public draft     → nowhere (readable in the UI, not auto-grounding)
// Every save/visibility/official change re-runs this, removing the doc from
// the collections it no longer belongs to — so nothing leaks or goes stale.

#[derive(Debug, Clone)]
pub struct KbDocSync {
    pub id: String,
    pub space_id: Option<String>,
    pub title: String,
    pub body: String,
    /// EFFECTIVE visibility — resolved through the doc's space when the doc
    /// inherits (EFFECTIVE_DOC_SELECT below for bulk paths). Never the raw
    /// kb_docs.visibility column: perms_inherited
    /// defaults true and visibility defaults 'org', so a doc sitting in a
    /// PRIVATE space reads 'org' raw — which is exactly how backfills used to
    /// push private docs into the org brain.
    pub visibility: String, // 'private' | 'org' | 'public'
    pub official: bool,
    pub owner_user_id: Option<String>,
}

/// Bulk-path effective-visibility select: kb_docs with their effective
/// visibility folded in, for callers that sync many docs at once
/// (backfill, sweep, space resync) and can't afford a per-doc lookup. Append a
/// `where` clause; the doc alias is `d`, the space `s`. (uuid columns are
/// ::text-cast — the crate's standing convention; sqlx reads no uuid type.)
pub const EFFECTIVE_DOC_SELECT: &str = r#"
  select d.id::text as id, d.space_id::text as "spaceId", d.title, d.body, d.official,
         d.owner_user_id::text as "ownerUserId",
         case when d.perms_inherited then coalesce(s.visibility, d.visibility) else d.visibility end as visibility
  from kb_docs d left join kb_spaces s on s.id = d.space_id"#;

/// The custom collection a KB space feeds (curation: bind a space to a brain
/// on Admin → Retrieval), if any.
async fn space_brain(pg: &PgPool, space_id: Option<&str>) -> Result<Option<String>, sqlx::Error> {
    let Some(sid) = space_id else { return Ok(None) };
    sqlx::query_scalar::<_, String>(
        "select s.rag_collection_id::text from kb_spaces s \
         join rag_collections c on c.id = s.rag_collection_id \
         where s.id = $1::uuid",
    )
    .bind(sid)
    .fetch_optional(pg)
    .await
}

pub async fn sync_kb_doc(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    doc: &KbDocSync,
) -> Result<(), String> {
    let org_id = auto_collection_id(pg, qd, ed, "org-kb").await?;
    let personal: Option<RagCollection> = match &doc.owner_user_id {
        Some(uid) => personal_collection_for(pg, uid)
            .await
            .map_err(|e| e.to_string())?,
        None => None,
    };
    // Per-doc routing wins over the space default: 'auto' | 'none' | <brain id>.
    // A garbage routing value fails the uuid cast and rejects the sync.
    let routing =
        sqlx::query_scalar::<_, String>("select rag_routing from kb_docs where id = $1::uuid")
            .bind(&doc.id)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    let route = routing.unwrap_or_else(|| "auto".into());
    let brain = if route != "auto" && route != "none" {
        sqlx::query_scalar::<_, String>(
            "select id::text from rag_collections where id = $1::uuid and kind = 'custom'",
        )
        .bind(&route)
        .fetch_optional(pg)
        .await
        .map_err(|e| e.to_string())?
    } else if route == "auto" {
        space_brain(pg, doc.space_id.as_deref())
            .await
            .map_err(|e| e.to_string())?
    } else {
        None
    };

    // Clear from every possible home first (idempotent), then place it. Custom
    // homes are cleared wholesale so re-routing can't leave stale copies.
    if let Some(org) = &org_id {
        let _ = unindex_document(pg, qd, org, "kb-doc", &doc.id).await;
    }
    if let Some(p) = &personal {
        let _ = unindex_document(pg, qd, &p.id, "kb-doc", &doc.id).await;
    }
    let customs = sqlx::query_scalar::<_, String>(
        "select id::text from rag_collections where kind = 'custom'",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    for c in &customs {
        let _ = unindex_document(pg, qd, c, "kb-doc", &doc.id).await;
    }

    if route == "none" {
        return Ok(()); // deliberately unindexed, everywhere
    }

    // The item ACL rides with the doc into whichever brain it lands in, so the
    // document-scope filter can re-check it at query time. spaceId is present
    // even when null — the payload feeds the content hash, so key presence
    // must be stable or an indexed doc churns on its next re-sync.
    let mut payload = Map::new();
    payload.insert("visibility".into(), json!(doc.visibility));
    payload.insert("ownerUserId".into(), json!(doc.owner_user_id));
    payload.insert("spaceId".into(), json!(doc.space_id));
    let idx = IndexDoc {
        source_type: "kb-doc".into(),
        source_id: doc.id.clone(),
        title: Some(doc.title.clone()),
        text: format!("{}\n\n{}", doc.title, doc.body),
        payload: Some(payload),
        href: Some(format!("/knowledge/{}", doc.id)),
    };
    if doc.visibility == "private" {
        // Privacy trumps routing: a private doc only ever reaches its owner's
        // personal brain, whatever the routing says.
        if let Some(p) = &personal {
            let _ = index_document(pg, qd, ed, &p.id, &idx).await;
        }
    } else if let Some(brain) = &brain {
        // Explicit assignment (doc- or space-level) — access governed by the
        // brain's own bindings.
        let _ = index_document(pg, qd, ed, brain, &idx).await;
    } else if doc.official
        && let Some(org) = &org_id
    {
        let _ = index_document(pg, qd, ed, org, &idx).await;
    }
    Ok(())
}

/// Remove a doc from every KB collection it might be in (on delete).
pub async fn unindex_kb_doc(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    id: &str,
    owner_user_id: Option<&str>,
) -> Result<(), String> {
    let org_id = auto_collection_id(pg, qd, ed, "org-kb").await?;
    if let Some(org) = &org_id {
        let _ = unindex_document(pg, qd, org, "kb-doc", id).await;
    }
    if let Some(uid) = owner_user_id
        && let Some(personal) = personal_collection_for(pg, uid)
            .await
            .map_err(|e| e.to_string())?
    {
        let _ = unindex_document(pg, qd, &personal.id, "kb-doc", id).await;
    }
    let customs = sqlx::query_scalar::<_, String>(
        "select id::text from rag_collections where kind = 'custom'",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    for c in &customs {
        let _ = unindex_document(pg, qd, c, "kb-doc", id).await;
    }
    Ok(())
}

/// Re-route every doc in a space through sync_kb_doc — run after (re)binding a
/// space to a brain, so existing docs move immediately.
pub async fn resync_space_docs(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    space_id: &str,
) -> Result<usize, String> {
    // AssertSqlSafe: the interpolated fragment is this crate's EFFECTIVE_DOC_SELECT.
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{EFFECTIVE_DOC_SELECT} where d.space_id = $1::uuid"
    )))
    .bind(space_id)
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let docs: Vec<KbDocSync> = rows.iter().map(kb_doc_of).collect();
    for d in &docs {
        let _ = sync_kb_doc(pg, qd, ed, d).await;
    }
    Ok(docs.len())
}

pub(crate) fn kb_doc_of(r: &sqlx::postgres::PgRow) -> KbDocSync {
    KbDocSync {
        id: r.try_get("id").unwrap_or_default(),
        space_id: r.try_get("spaceId").unwrap_or(None),
        title: r.try_get("title").unwrap_or_default(),
        body: r.try_get("body").unwrap_or_default(),
        visibility: r.try_get("visibility").unwrap_or_default(),
        official: r.try_get("official").unwrap_or(false),
        owner_user_id: r.try_get("ownerUserId").unwrap_or(None),
    }
}

pub async fn unindex_activity(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    source_type: &str,
    source_id: &str,
) -> Result<(), String> {
    let Some(id) = auto_collection_id(pg, qd, ed, "activity").await? else {
        return Ok(());
    };
    let _ = unindex_document(pg, qd, &id, source_type, source_id).await;
    Ok(())
}

// ── Domain shapers ──────────────────────────────────────────────────────────
// Tickets and comments are board-scoped (payload.boardId), so the
// activity_scope board filter already governs who can retrieve them.

pub struct TicketSrc<'a> {
    pub id: &'a str,
    pub board_id: &'a str,
    pub ticket_ref: Option<&'a str>,
    pub title: &'a str,
    pub description: Option<&'a str>,
}

pub fn ticket_doc(t: &TicketSrc<'_>) -> IndexDoc {
    let text = [Some(t.title), t.description]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut payload = Map::new();
    payload.insert("boardId".into(), json!(t.board_id));
    IndexDoc {
        source_type: "ticket".into(),
        source_id: t.id.into(),
        title: Some(format!("{}: {}", t.ticket_ref.unwrap_or("Ticket"), t.title)),
        text,
        payload: Some(payload),
        href: Some(format!("/boards/{}/{}", t.board_id, t.id)),
    }
}

pub struct CommentSrc<'a> {
    pub id: &'a str,
    pub task_id: &'a str,
    pub board_id: &'a str,
    pub ticket_ref: Option<&'a str>,
    pub author: &'a str,
    pub content: &'a str,
}

pub fn comment_doc(c: &CommentSrc<'_>) -> IndexDoc {
    let mut payload = Map::new();
    payload.insert("boardId".into(), json!(c.board_id));
    IndexDoc {
        source_type: "comment".into(),
        source_id: c.id.into(),
        // The middle dot is deliberate — the title is `${ref} · ${author}`.
        title: Some(format!(
            "{} · {}",
            c.ticket_ref.unwrap_or("Ticket"),
            c.author
        )),
        text: c.content.into(),
        payload: Some(payload),
        // The comment's href lands on its TICKET, not itself.
        href: Some(format!("/boards/{}/{}", c.board_id, c.task_id)),
    }
}

pub async fn index_ticket(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    t: &TicketSrc<'_>,
) -> Result<(), String> {
    index_activity(pg, qd, ed, &ticket_doc(t)).await
}

pub async fn index_ticket_comment(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    c: &CommentSrc<'_>,
) -> Result<(), String> {
    index_activity(pg, qd, ed, &comment_doc(c)).await
}

/// The payload key a container purge runs on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActivityField {
    ChannelId,
    BoardId,
}

impl ActivityField {
    fn key(self) -> &'static str {
        match self {
            ActivityField::ChannelId => "channelId",
            ActivityField::BoardId => "boardId",
        }
    }
}

/// Purge all activity points for a container (e.g. a deleted channel or board)
/// by payload field, without enumerating each item. Leaves the rag_points
/// bookkeeping rows — they're harmless dead refs (UUID source ids never recur)
/// and search reads only from Qdrant.
pub async fn purge_activity_by_field(
    pg: &PgPool,
    qd: &QdrantDeps,
    field: ActivityField,
    value: &str,
) -> Result<(), String> {
    let name = sqlx::query_scalar::<_, String>(
        "select qdrant_name from rag_collections where kind = 'activity' and auto limit 1",
    )
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    if let Some(name) = name {
        let mut filter = Map::new();
        filter.insert(
            "must".into(),
            json!([{ "key": field.key(), "match": { "value": value } }]),
        );
        delete_by_filter(qd, &name, &filter).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_carries_its_ref_board_and_two_part_text() {
        let d = ticket_doc(&TicketSrc {
            id: "t1",
            board_id: "b1",
            ticket_ref: Some("TCK-41"),
            title: "Fix login",
            description: Some("  It 500s on SSO.  "),
        });
        assert_eq!(d.title.as_deref(), Some("TCK-41: Fix login"));
        assert_eq!(d.text, "Fix login\n\n  It 500s on SSO.  ");
        assert_eq!(d.source_type, "ticket");
        assert_eq!(d.source_id, "t1");
        assert_eq!(d.href.as_deref(), Some("/boards/b1/t1"));
        assert_eq!(
            d.payload.as_ref().unwrap().get("boardId"),
            Some(&json!("b1"))
        );
    }

    #[test]
    fn a_blank_description_collapses_and_the_ref_defaults() {
        let d = ticket_doc(&TicketSrc {
            id: "t2",
            board_id: "b1",
            ticket_ref: None,
            title: "Broken",
            description: Some("   "),
        });
        assert_eq!(d.title.as_deref(), Some("Ticket: Broken"));
        // The blank description is filtered out — no trailing "\n\n".
        assert_eq!(d.text, "Broken");
        // And a missing description is the same shape.
        let d = ticket_doc(&TicketSrc {
            id: "t2",
            board_id: "b1",
            ticket_ref: None,
            title: "Broken",
            description: None,
        });
        assert_eq!(d.text, "Broken");
    }

    #[test]
    fn a_comment_points_at_its_ticket() {
        let d = comment_doc(&CommentSrc {
            id: "c1",
            task_id: "t9",
            board_id: "b2",
            ticket_ref: Some("TCK-7"),
            author: "Sam",
            content: " repro'd on staging",
        });
        assert_eq!(d.title.as_deref(), Some("TCK-7 · Sam"));
        assert_eq!(d.text, " repro'd on staging");
        assert_eq!(d.source_type, "comment");
        assert_eq!(d.href.as_deref(), Some("/boards/b2/t9"));
        let d = comment_doc(&CommentSrc {
            id: "c1",
            task_id: "t9",
            board_id: "b2",
            ticket_ref: None,
            author: "Sam",
            content: "x",
        });
        assert_eq!(d.title.as_deref(), Some("Ticket · Sam"));
    }

    #[test]
    fn the_effective_select_folds_space_perms_not_raw_columns() {
        // The contract in one string: perms_inherited reads through the space,
        // a doc's own visibility wins otherwise.
        assert!(EFFECTIVE_DOC_SELECT.contains(
            "case when d.perms_inherited then coalesce(s.visibility, d.visibility) \
             else d.visibility end as visibility"
        ));
        assert!(EFFECTIVE_DOC_SELECT.contains("left join kb_spaces s on s.id = d.space_id"));
    }
}
