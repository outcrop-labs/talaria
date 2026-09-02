// The KB engine — spaces/docs, comments, okf import, permissions.
pub mod comments;
pub mod okf;
pub mod perms;

// The knowledgebase — an Outline-style markdown drive. Spaces group docs; docs
// nest. Every save snapshots a version (reusing internal_versions). Marking a
// doc "official" indexes it into the org-kb RAG collection so agents ground on
// it; un-officializing / deleting removes it.
//
// Port of ui/src/server/kb.ts. The read half predates the write (the refs cone
// needed it in batch 3); batch 5 completed the plane in place. Field
// declaration order is WIRE order: these structs serialize straight to the
// JSON the TS routes stringified, and the TS column aliases are that order —
// getDoc puts `sort` before `ownerUserId`, the DOC_META list puts
// `ownerUserId` before `sort` (and drops body/okf), so the two shapes are two
// structs, not one struct reordered per call site.

use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;
use crate::email::email_escape;
use crate::internal_history::snapshot;
use crate::kb::perms::{EditorGrant, Guarded, ITEM_DOC, ITEM_SPACE, can_read, list_editors};
use crate::retrieval::embed::{self, EmbedDeps};
use crate::retrieval::qdrant::{self, QdrantDeps};
use crate::retrieval::sources::{
    EFFECTIVE_DOC_SELECT, KbDocSync, kb_doc_of, resync_space_docs, sync_kb_doc, unindex_kb_doc,
};

/// A KB space (kb.ts KbSpace) — full row shape, in SPACE_COLS wire order.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbSpace {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub body: String,
    pub visibility: String,
    pub public_slug: Option<String>,
    /// The space's generated OKF digest doc (Librarian-maintained).
    pub okf_doc_id: Option<String>,
    pub edit_policy: String,
    pub owner_user_id: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
}

/// A KB doc (kb.ts KbDoc) — full row shape, in getDoc's wire order.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbDoc {
    pub id: String,
    pub space_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub icon: Option<String>,
    pub body: String,
    /// Hidden agent-facing OKF concept (frontmatter + summary),
    /// Librarian-written.
    pub okf: Option<String>,
    pub kind: String,
    pub official: bool,
    pub visibility: String,
    pub public_slug: Option<String>,
    pub edit_policy: String,
    pub perms_inherited: bool,
    pub sort: i32,
    pub owner_user_id: Option<String>,
    /// RAG routing: 'auto' (space binding / org rules) | 'none' | a brain id.
    pub rag_routing: String,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub updated_at: String,
}

/// The list/tree shape (kb.ts KbDocMeta) — no body, no okf, and
/// `ownerUserId` BEFORE `sort` (the detail order swaps them).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbDocMeta {
    pub id: String,
    pub space_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub icon: Option<String>,
    pub kind: String,
    pub official: bool,
    pub visibility: String,
    pub public_slug: Option<String>,
    pub edit_policy: String,
    pub perms_inherited: bool,
    pub owner_user_id: Option<String>,
    pub sort: i32,
    pub rag_routing: String,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub updated_at: String,
}

// ── Spaces ────────────────────────────────────────────────────────────────────

const SPACE_COLS: &str = "id::text, name, description, icon, body, visibility, public_slug, \
                          okf_doc_id::text, edit_policy, owner_user_id::text, created_by::text, \
                          (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms";

#[derive(sqlx::FromRow)]
struct SpaceRow {
    id: String,
    name: String,
    description: Option<String>,
    icon: Option<String>,
    body: String,
    visibility: String,
    public_slug: Option<String>,
    okf_doc_id: Option<String>,
    edit_policy: String,
    owner_user_id: Option<String>,
    created_by: Option<String>,
    created_ms: i64,
}

impl From<SpaceRow> for KbSpace {
    fn from(r: SpaceRow) -> Self {
        let SpaceRow {
            id,
            name,
            description,
            icon,
            body,
            visibility,
            public_slug,
            okf_doc_id,
            edit_policy,
            owner_user_id,
            created_by,
            created_ms,
        } = r;
        KbSpace {
            id,
            name,
            description,
            icon,
            body,
            visibility,
            public_slug,
            okf_doc_id,
            edit_policy,
            owner_user_id,
            created_by,
            created_at: epoch_ms_to_iso(created_ms),
        }
    }
}

pub async fn list_spaces(pg: &PgPool) -> Result<Vec<KbSpace>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's SPACE_COLS column list.
    let sql = format!("select {SPACE_COLS} from kb_spaces order by name asc");
    let rows: Vec<SpaceRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(KbSpace::from).collect())
}

pub async fn get_space(pg: &PgPool, id: &str) -> Result<Option<KbSpace>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's SPACE_COLS column list.
    let sql = format!("select {SPACE_COLS} from kb_spaces where id = $1::uuid");
    let row: Option<SpaceRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(KbSpace::from))
}

/// createSpace input — `description`/`icon` absent vs null are the same thing
/// here (both insert null), so plain Options.
pub struct NewSpace {
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub created_by: String,
    pub owner_user_id: Option<String>,
}

pub async fn create_space(pg: &PgPool, input: &NewSpace) -> Result<KbSpace, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's SPACE_COLS column list.
    let sql = format!(
        "insert into kb_spaces (name, description, icon, created_by, owner_user_id) \
         values ($1, $2, $3, $4, $5::uuid) returning {SPACE_COLS}"
    );
    let row: SpaceRow = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(&input.name)
        .bind(&input.description)
        .bind(&input.icon)
        .bind(&input.created_by)
        .bind(&input.owner_user_id)
        .fetch_one(pg)
        .await?;
    Ok(KbSpace::from(row))
}

/// updateSpace patch — tri-state (`Option<Option<T>>`) where TS distinguishes
/// absent (`undefined`, no update) from explicit null (clear the column).
#[derive(Default)]
pub struct SpacePatch {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub icon: Option<Option<String>>,
    pub body: Option<String>,
    pub visibility: Option<String>,
    pub edit_policy: Option<String>,
}

pub async fn update_space(
    pg: &PgPool,
    id: &str,
    patch: &SpacePatch,
    actor: Option<&str>,
) -> Result<Option<KbSpace>, sqlx::Error> {
    if let Some(name) = &patch.name {
        sqlx::query("update kb_spaces set name = $2 where id = $1::uuid")
            .bind(id)
            .bind(name)
            .execute(pg)
            .await?;
    }
    if let Some(description) = &patch.description {
        sqlx::query("update kb_spaces set description = $2 where id = $1::uuid")
            .bind(id)
            .bind(description)
            .execute(pg)
            .await?;
    }
    if let Some(icon) = &patch.icon {
        sqlx::query("update kb_spaces set icon = $2 where id = $1::uuid")
            .bind(id)
            .bind(icon)
            .execute(pg)
            .await?;
    }
    if let Some(body) = &patch.body {
        sqlx::query("update kb_spaces set body = $2 where id = $1::uuid")
            .bind(id)
            .bind(body)
            .execute(pg)
            .await?;
    }
    if let Some(edit_policy) = &patch.edit_policy {
        sqlx::query("update kb_spaces set edit_policy = $2 where id = $1::uuid")
            .bind(id)
            .bind(edit_policy)
            .execute(pg)
            .await?;
    }
    if let Some(visibility) = &patch.visibility {
        sqlx::query("update kb_spaces set visibility = $2 where id = $1::uuid")
            .bind(id)
            .bind(visibility)
            .execute(pg)
            .await?;
        if visibility == "public" {
            sqlx::query(
                "update kb_spaces set public_slug = $2 where id = $1::uuid and public_slug is null",
            )
            .bind(id)
            .bind(random_slug())
            .execute(pg)
            .await?;
        }
    }
    let next = get_space(pg, id).await?;
    // Version the overview like a doc (name + body snapshot) on content change
    // — errors swallowed exactly as TS's `.catch(() => {})`.
    if let Some(space) = &next
        && (patch.body.is_some() || patch.name.is_some())
    {
        let _ = snapshot(
            pg,
            "kb-space",
            id,
            &format!("# {}\n\n{}", space.name, space.body),
            actor,
        )
        .await;
    }
    // A space's visibility IS the effective visibility of every doc that
    // inherits it, so re-place them: making a space private has to pull its
    // docs out of the org brain, and nothing else would (the docs' own rows
    // didn't change). Detached — TS's `void resyncSpaceDocs(id).catch(...)`.
    if patch.visibility.is_some() {
        let (pg, space_id) = (pg.clone(), id.to_string());
        tokio::spawn(async move {
            let qd = qdrant::real_deps();
            let ed = embed::real_deps();
            let _ = resync_space_docs(&pg, &qd, &ed, &space_id).await;
        });
    }
    Ok(next)
}

pub async fn get_public_space(pg: &PgPool, slug: &str) -> Result<Option<KbSpace>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's SPACE_COLS column list.
    let sql = format!(
        "select {SPACE_COLS} from kb_spaces where public_slug = $1 and visibility = 'public'"
    );
    let row: Option<SpaceRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(slug)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(KbSpace::from))
}

pub async fn delete_space(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    id: &str,
) -> Result<(), sqlx::Error> {
    // Unindex every doc in the space (org brain + any personal collections),
    // each best-effort, then drop the space.
    let docs: Vec<(String, Option<String>)> = sqlx::query_as(
        "select id::text, owner_user_id::text from kb_docs where space_id = $1::uuid",
    )
    .bind(id)
    .fetch_all(pg)
    .await?;
    for (doc_id, owner) in &docs {
        let _ = unindex_kb_doc(pg, qd, ed, doc_id, owner.as_deref()).await;
    }
    sqlx::query("delete from kb_spaces where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
}

// ── Docs ──────────────────────────────────────────────────────────────────────

const DOC_COLS: &str = "id::text, space_id::text, parent_id::text, title, icon, body, okf, kind, official, \
                        visibility, public_slug, edit_policy, perms_inherited, sort, owner_user_id::text, \
                        rag_routing, created_by::text, updated_by::text, \
                        (trunc(extract(epoch from updated_at) * 1000))::bigint as updated_ms";

#[derive(sqlx::FromRow)]
struct DocRow {
    id: String,
    space_id: String,
    parent_id: Option<String>,
    title: String,
    icon: Option<String>,
    body: String,
    okf: Option<String>,
    kind: String,
    official: bool,
    visibility: String,
    public_slug: Option<String>,
    edit_policy: String,
    perms_inherited: bool,
    sort: i32,
    owner_user_id: Option<String>,
    rag_routing: String,
    created_by: Option<String>,
    updated_by: Option<String>,
    updated_ms: i64,
}

impl From<DocRow> for KbDoc {
    fn from(r: DocRow) -> Self {
        let DocRow {
            id,
            space_id,
            parent_id,
            title,
            icon,
            body,
            okf,
            kind,
            official,
            visibility,
            public_slug,
            edit_policy,
            perms_inherited,
            sort,
            owner_user_id,
            rag_routing,
            created_by,
            updated_by,
            updated_ms,
        } = r;
        KbDoc {
            id,
            space_id,
            parent_id,
            title,
            icon,
            body,
            okf,
            kind,
            official,
            visibility,
            public_slug,
            edit_policy,
            perms_inherited,
            sort,
            owner_user_id,
            rag_routing,
            created_by,
            updated_by,
            updated_at: epoch_ms_to_iso(updated_ms),
        }
    }
}

const DOC_META_COLS: &str = "id::text, space_id::text, parent_id::text, title, icon, kind, official, \
                             visibility, public_slug, edit_policy, perms_inherited, owner_user_id::text, \
                             sort, rag_routing, created_by::text, updated_by::text, \
                             (trunc(extract(epoch from updated_at) * 1000))::bigint as updated_ms";

#[derive(sqlx::FromRow)]
struct DocMetaRow {
    id: String,
    space_id: String,
    parent_id: Option<String>,
    title: String,
    icon: Option<String>,
    kind: String,
    official: bool,
    visibility: String,
    public_slug: Option<String>,
    edit_policy: String,
    perms_inherited: bool,
    owner_user_id: Option<String>,
    sort: i32,
    rag_routing: String,
    created_by: Option<String>,
    updated_by: Option<String>,
    updated_ms: i64,
}

impl From<DocMetaRow> for KbDocMeta {
    fn from(r: DocMetaRow) -> Self {
        let DocMetaRow {
            id,
            space_id,
            parent_id,
            title,
            icon,
            kind,
            official,
            visibility,
            public_slug,
            edit_policy,
            perms_inherited,
            owner_user_id,
            sort,
            rag_routing,
            created_by,
            updated_by,
            updated_ms,
        } = r;
        KbDocMeta {
            id,
            space_id,
            parent_id,
            title,
            icon,
            kind,
            official,
            visibility,
            public_slug,
            edit_policy,
            perms_inherited,
            owner_user_id,
            sort,
            rag_routing,
            created_by,
            updated_by,
            updated_at: epoch_ms_to_iso(updated_ms),
        }
    }
}

pub async fn get_doc(pg: &PgPool, id: &str) -> Result<Option<KbDoc>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's DOC_COLS column list.
    let sql = format!("select {DOC_COLS} from kb_docs where id = $1::uuid");
    let row: Option<DocRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(KbDoc::from))
}

/// getPublicDoc — same shape as getDoc MINUS ragRouting (routing is internal;
/// a public reader learns nothing about the org's brain wiring).
pub async fn get_public_doc(pg: &PgPool, slug: &str) -> Result<Option<KbDoc>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's DOC_COLS column list.
    let sql =
        format!("select {DOC_COLS} from kb_docs where public_slug = $1 and visibility = 'public'");
    let row: Option<DocRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(slug)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(KbDoc::from))
}

pub async fn list_docs(pg: &PgPool, space_id: &str) -> Result<Vec<KbDocMeta>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's DOC_META_COLS list.
    let sql = format!(
        "select {DOC_META_COLS} from kb_docs where space_id = $1::uuid order by sort asc, title asc"
    );
    let rows: Vec<DocMetaRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(space_id)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(KbDocMeta::from).collect())
}

/// A starter body for OKF-structured agent docs (machine-consumable).
pub const OKF_TEMPLATE: &str = "---\ntitle:\nsummary:\ntags: []\nsource: talaria-kb\n---\n\n## Overview\n\n## Facts\n\n## Procedures\n";

/// createDoc input. `title` defaults 'Untitled', `kind` defaults 'human' (an
/// agent-kind doc starts from OKF_TEMPLATE).
pub struct NewDoc {
    pub space_id: String,
    pub parent_id: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub created_by: String,
    pub owner_user_id: Option<String>,
}

pub async fn create_doc(pg: &PgPool, input: &NewDoc) -> Result<KbDoc, sqlx::Error> {
    let kind = input.kind.as_deref().unwrap_or("human");
    let body = if kind == "agent" { OKF_TEMPLATE } else { "" };
    let id: (String,) = sqlx::query_as(
        "insert into kb_docs (space_id, parent_id, title, body, kind, created_by, updated_by, owner_user_id) \
         values ($1::uuid, $2::uuid, $3, $4, $5, $6, $6, $7::uuid) returning id::text",
    )
    .bind(&input.space_id)
    .bind(&input.parent_id)
    .bind(input.title.as_deref().unwrap_or("Untitled"))
    .bind(body)
    .bind(kind)
    .bind(&input.created_by)
    .bind(&input.owner_user_id)
    .fetch_one(pg)
    .await?;
    get_doc(pg, &id.0)
        .await?
        .ok_or_else(|| sqlx::Error::RowNotFound)
}

/// saveDoc patch — tri-state where TS distinguishes absent from explicit null
/// (icon, parentId).
#[derive(Default)]
pub struct DocPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub icon: Option<Option<String>>,
    pub visibility: Option<String>,
    pub edit_policy: Option<String>,
    pub perms_inherited: Option<bool>,
    pub parent_id: Option<Option<String>>,
}

pub async fn save_doc(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    id: &str,
    patch: &DocPatch,
    actor: &str,
) -> Result<Option<KbDoc>, sqlx::Error> {
    let prev = get_doc(pg, id).await?;
    let Some(prev) = prev else { return Ok(None) };

    if let Some(title) = &patch.title {
        sqlx::query(
            "update kb_docs set title = $2, updated_by = $3, updated_at = now() where id = $1::uuid",
        )
        .bind(id)
        .bind(title)
        .bind(actor)
        .execute(pg)
        .await?;
    }
    if let Some(body) = &patch.body {
        sqlx::query(
            "update kb_docs set body = $2, updated_by = $3, updated_at = now() where id = $1::uuid",
        )
        .bind(id)
        .bind(body)
        .bind(actor)
        .execute(pg)
        .await?;
    }
    if let Some(icon) = &patch.icon {
        sqlx::query("update kb_docs set icon = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(icon)
            .execute(pg)
            .await?;
    }
    if let Some(perms_inherited) = patch.perms_inherited {
        sqlx::query(
            "update kb_docs set perms_inherited = $2, updated_at = now() where id = $1::uuid",
        )
        .bind(id)
        .bind(perms_inherited)
        .execute(pg)
        .await?;
    }
    if let Some(edit_policy) = &patch.edit_policy {
        sqlx::query("update kb_docs set edit_policy = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(edit_policy)
            .execute(pg)
            .await?;
    }
    if let Some(visibility) = patch.visibility.as_deref() {
        sqlx::query("update kb_docs set visibility = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(visibility)
            .execute(pg)
            .await?;
        // Public docs get a stable slug on first publish.
        if visibility == "public" && prev.public_slug.is_none() {
            sqlx::query(
                "update kb_docs set public_slug = $2 where id = $1::uuid and public_slug is null",
            )
            .bind(id)
            .bind(random_slug())
            .execute(pg)
            .await?;
        }
    }
    if let Some(parent_id) = &patch.parent_id {
        sqlx::query(
            "update kb_docs set parent_id = $2::uuid, updated_at = now() where id = $1::uuid",
        )
        .bind(id)
        .bind(parent_id)
        .execute(pg)
        .await?;
    }

    let next = get_doc(pg, id).await?;
    if let Some(next) = &next {
        // Snapshot on content change; re-route RAG on any content/visibility
        // change — errors swallowed exactly as TS's `.catch(() => {})`.
        if patch.body.is_some() || patch.title.is_some() {
            let _ = snapshot(
                pg,
                "kb-doc",
                id,
                &format!("# {}\n\n{}", next.title, next.body),
                Some(actor),
            )
            .await;
        }
        if patch.body.is_some()
            || patch.title.is_some()
            || patch.visibility.is_some()
            || patch.perms_inherited.is_some()
        {
            let _ = sync_doc_effective(pg, qd, ed, next).await;
        }
    }
    Ok(next)
}

/// Sync a doc into RAG using its EFFECTIVE visibility (inherited from the
/// folder when applicable) — so an inherited-private doc never lands in the
/// org brain.
async fn sync_doc_effective(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    doc: &KbDoc,
) -> Result<(), String> {
    let eff = effective_doc_perms(pg, doc)
        .await
        .map_err(|e| e.to_string())?;
    sync_kb_doc(
        pg,
        qd,
        ed,
        &KbDocSync {
            id: doc.id.clone(),
            space_id: Some(doc.space_id.clone()),
            title: doc.title.clone(),
            body: doc.body.clone(),
            visibility: eff.perms.visibility.clone(),
            official: doc.official,
            owner_user_id: doc.owner_user_id.clone(),
        },
    )
    .await
}

pub async fn delete_doc(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    id: &str,
) -> Result<(), sqlx::Error> {
    let doc = get_doc(pg, id).await?;
    let _ = unindex_kb_doc(
        pg,
        qd,
        ed,
        id,
        doc.as_ref().and_then(|d| d.owner_user_id.as_deref()),
    )
    .await;
    sqlx::query("delete from kb_docs where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
}

/// Set a doc's RAG routing ('auto' | 'none' | a custom brain id) and re-place
/// it immediately. Owner-only at the route layer — routing changes who can
/// retrieve the doc's content. Err = "unknown brain" (the TS throw verbatim).
pub async fn set_doc_routing(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    id: &str,
    routing: &str,
    actor: &str,
) -> Result<Option<KbDoc>, String> {
    if routing != "auto" && routing != "none" {
        let ok: Option<(i32,)> =
            sqlx::query_as("select 1 from rag_collections where id = $1::uuid and kind = 'custom'")
                .bind(routing)
                .fetch_optional(pg)
                .await
                .map_err(|e| crate::error::pg_message(&e))?;
        if ok.is_none() {
            return Err("unknown brain".into());
        }
    }
    sqlx::query(
        "update kb_docs set rag_routing = $2, updated_by = $3, updated_at = now() where id = $1::uuid",
    )
    .bind(id)
    .bind(routing)
    .bind(actor)
    .execute(pg)
    .await
    .map_err(|e| crate::error::pg_message(&e))?;
    let doc = get_doc(pg, id)
        .await
        .map_err(|e| crate::error::pg_message(&e))?;
    let Some(doc) = doc else { return Ok(None) };
    let _ = sync_doc_effective(pg, qd, ed, &doc).await;
    Ok(Some(doc))
}

/// Mark a doc official (→ org brain) or not. Re-routes its RAG placement.
pub async fn set_official(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    id: &str,
    official: bool,
    actor: &str,
) -> Result<Option<KbDoc>, sqlx::Error> {
    sqlx::query(
        "update kb_docs set official = $2, updated_by = $3, updated_at = now() where id = $1::uuid",
    )
    .bind(id)
    .bind(official)
    .bind(actor)
    .execute(pg)
    .await?;
    let doc = get_doc(pg, id).await?;
    let Some(doc) = doc else { return Ok(None) };
    let _ = sync_doc_effective(pg, qd, ed, &doc).await;
    Ok(Some(doc))
}

/// Index a user's existing private docs into their (freshly created) personal
/// RAG collection — called when they spin up their assistant.
pub async fn sync_user_private_docs(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    // Effective visibility, like every other sync path: a doc inheriting
    // private from its space is private even though its own column still
    // says 'org'.
    // AssertSqlSafe: the interpolated fragment is retrieval::sources'
    // EFFECTIVE_DOC_SELECT.
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{EFFECTIVE_DOC_SELECT} where d.owner_user_id = $1::uuid"
    )))
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    for doc in rows.iter().map(kb_doc_of) {
        if doc.visibility != "private" {
            continue;
        }
        let _ = sync_kb_doc(pg, qd, ed, &doc).await;
    }
    Ok(())
}

/// Reparent + reorder a doc within its space (the sidebar tree drag target).
/// A doc can't be nested under itself or a descendant — None is the reject.
pub async fn move_doc(
    pg: &PgPool,
    id: &str,
    parent_id: Option<&str>,
    sort: i32,
) -> Result<Option<KbDocMeta>, sqlx::Error> {
    if let Some(parent) = parent_id {
        if parent == id {
            return Ok(None);
        }
        // Walk up from the proposed parent; if we reach `id`, this is a cycle.
        let mut cur = Some(parent.to_string());
        for _ in 0..100 {
            let Some(node) = cur else { break };
            if node == id {
                return Ok(None);
            }
            let rows: Option<(Option<String>,)> =
                sqlx::query_as("select parent_id::text from kb_docs where id = $1::uuid")
                    .bind(&node)
                    .fetch_optional(pg)
                    .await?;
            cur = rows.and_then(|(pid,)| pid);
        }
    }
    sqlx::query("update kb_docs set parent_id = $2::uuid, sort = $3, updated_at = now() where id = $1::uuid")
        .bind(id)
        .bind(parent_id)
        .bind(sort)
        .execute(pg)
        .await?;
    // AssertSqlSafe: the interpolation is this crate's DOC_META_COLS list.
    let sql = format!("select {DOC_META_COLS} from kb_docs where id = $1::uuid");
    let row: Option<DocMetaRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(KbDocMeta::from))
}

/// The perms + grants pair an ACL check runs against. What a doc's own row
/// says when it does not inherit; when it does, visibility and edit policy
/// come from its folder (falling back to the doc's own if the space row is
/// gone), while the editor list becomes the SPACE's list — but ownership
/// always stays with the doc's own creator, so the author never loses edit
/// rights by being filed under someone else's folder.
pub struct EffectiveDocPerms {
    pub perms: Guarded,
    pub grants: Vec<EditorGrant>,
}

pub async fn effective_doc_perms(
    pg: &PgPool,
    doc: &KbDoc,
) -> Result<EffectiveDocPerms, sqlx::Error> {
    if !doc.perms_inherited {
        return Ok(EffectiveDocPerms {
            perms: Guarded {
                visibility: doc.visibility.clone(),
                edit_policy: doc.edit_policy.clone(),
                owner_user_id: doc.owner_user_id.clone(),
                created_by: doc.created_by.clone(),
            },
            grants: list_editors(pg, ITEM_DOC, &doc.id).await?,
        });
    }
    let space = get_space(pg, &doc.space_id).await?;
    Ok(EffectiveDocPerms {
        perms: Guarded {
            visibility: space
                .as_ref()
                .map(|s| s.visibility.clone())
                .unwrap_or_else(|| doc.visibility.clone()),
            edit_policy: space
                .as_ref()
                .map(|s| s.edit_policy.clone())
                .unwrap_or_else(|| doc.edit_policy.clone()),
            owner_user_id: doc.owner_user_id.clone(),
            created_by: doc.created_by.clone(),
        },
        grants: match &space {
            Some(_) => list_editors(pg, ITEM_SPACE, &doc.space_id).await?,
            None => Vec::new(),
        },
    })
}

// ── Search ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbSearchHit {
    pub id: String,
    pub space_id: String,
    pub space_name: String,
    pub title: String,
    pub icon: Option<String>,
    pub visibility: String,
    /// Spaces are documents too (their overview) — hits open the space itself.
    pub kind: String,
    pub snippet: String,
    /// Not in the TS interface — but it IS on the wire: searchDocs pushes the
    /// raw SQL row (select-order keys, rank selected last) straight into the
    /// response. Declaration order mirrors the select list.
    pub rank: f64,
}

// ts_headline does NOT escape the document it excerpts — a doc body carrying
// `<img src=x onerror=...>` comes back verbatim, and snippets render as HTML
// in the search panel (KbSearch.svelte renders {@html h.snippet}). So highlight
// with sentinels no real document carries (STX-delimited control chars), escape
// the whole snippet here, and only then turn the sentinels into <b>. Escaping
// lives in this module so every consumer of KbSearchHit.snippet is safe, not
// just the one that renders today.
const HL_START: &str = "\u{2}hl\u{2}";
const HL_STOP: &str = "\u{2}/hl\u{2}";
const HEADLINE_OPTS: &str = "MaxWords=24, MinWords=8, ShortWord=3, MaxFragments=1, \
                             StartSel=\"\u{2}hl\u{2}\", StopSel=\"\u{2}/hl\u{2}\"";

/// The whole XSS boundary for KB search — exported for tests. Escapes first,
/// then swaps the sentinels: the sentinels themselves contain nothing the
/// escaper would touch, so the order only matters in that the excerpt must
/// never carry a raw `<b>` through.
pub fn safe_snippet(raw: Option<&str>) -> String {
    email_escape(raw.unwrap_or(""))
        .replace(HL_START, "<b>")
        .replace(HL_STOP, "</b>")
}

/// Full-text search across everything the caller may read: docs AND the
/// top-level space overviews (a space is itself a document), honoring the
/// EFFECTIVE permission model — a doc inside a private space is private even
/// when its own row says 'org' (inheritance), and explicit kb_editors grants
/// admit their grantees. Over-fetch (60), then filter with the same canRead
/// the read routes use, capping at 20.
pub async fn search_docs(
    pg: &PgPool,
    query: &str,
    viewer: SearchViewer<'_>,
) -> Result<Vec<KbSearchHit>, sqlx::Error> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    // rank crosses as TEXT, parsed back to f64 below: ts_rank returns FLOAT4,
    // and the double it promotes to differs from the double postgres.js parses
    // from PG's shortest float4 text ("0.09910322") — 0.09910322153520584 on
    // the wire would diverge from TS. rank_f stays numeric for the order-by;
    // the outer projection re-aliases so the row's key order matches TS's
    // select-star either side.
    let hits = sqlx::query(
        "select id::text, space_id, space_name, title, icon, visibility, kind, snippet, rank_f::text as rank \
         from ( \
           select d.id::text, d.space_id::text, s.name as space_name, d.title, d.icon, d.visibility, 'doc' as kind, \
                  ts_headline('english', coalesce(d.body,''), plainto_tsquery('english', $1), $2) as snippet, \
                  ts_rank(to_tsvector('english', coalesce(d.title,'') || ' ' || coalesce(d.body,'')), \
                          plainto_tsquery('english', $1)) as rank_f \
           from kb_docs d join kb_spaces s on s.id = d.space_id \
           where to_tsvector('english', coalesce(d.title,'') || ' ' || coalesce(d.body,'')) \
                 @@ plainto_tsquery('english', $1) \
           union all \
           select s.id::text, s.id::text, s.name, s.name as title, s.icon, s.visibility, 'space' as kind, \
                  ts_headline('english', coalesce(s.body,''), plainto_tsquery('english', $1), $2) as snippet, \
                  ts_rank(to_tsvector('english', coalesce(s.name,'') || ' ' || coalesce(s.body,'')), \
                          plainto_tsquery('english', $1)) as rank_f \
           from kb_spaces s \
           where to_tsvector('english', coalesce(s.name,'') || ' ' || coalesce(s.body,'')) \
                 @@ plainto_tsquery('english', $1) \
         ) hits order by rank_f desc limit 60",
    )
    .bind(q)
    .bind(HEADLINE_OPTS)
    .fetch_all(pg)
    .await?;

    use sqlx::Row;
    let mut out: Vec<KbSearchHit> = Vec::new();
    for r in &hits {
        if out.len() >= 20 {
            break;
        }
        let kind: String = r.try_get("kind").unwrap_or_default();
        // Fail closed: an unreadable hit (or a lookup error) is dropped.
        let allowed = match kind.as_str() {
            "space" => match get_space(
                pg,
                r.try_get::<String, &str>("id").unwrap_or_default().as_str(),
            )
            .await
            {
                Ok(Some(sp)) => {
                    let grants = list_editors(pg, ITEM_SPACE, &sp.id)
                        .await
                        .unwrap_or_default();
                    can_read(
                        &guarded_of_space(&sp),
                        Some(viewer.user_id),
                        viewer.who,
                        &grants,
                    )
                }
                _ => false,
            },
            _ => match get_doc(
                pg,
                r.try_get::<String, &str>("id").unwrap_or_default().as_str(),
            )
            .await
            {
                Ok(Some(d)) => match effective_doc_perms(pg, &d).await {
                    Ok(eff) => can_read(&eff.perms, Some(viewer.user_id), viewer.who, &eff.grants),
                    Err(_) => false,
                },
                _ => false,
            },
        };
        if !allowed {
            continue;
        }
        out.push(KbSearchHit {
            id: r.try_get("id").unwrap_or_default(),
            space_id: r.try_get("space_id").unwrap_or_default(),
            space_name: r.try_get("space_name").unwrap_or_default(),
            title: r.try_get("title").unwrap_or_default(),
            icon: r.try_get("icon").unwrap_or_default(),
            visibility: r.try_get("visibility").unwrap_or_default(),
            kind,
            snippet: r.try_get("snippet").unwrap_or_default(),
            // text→f64, the postgres.js path (see the SQL comment above).
            rank: r
                .try_get::<String, &str>("rank")
                .ok()
                .and_then(|t| t.parse().ok())
                .unwrap_or_default(),
        });
    }
    // Escape once, here, after ACL filtering and before the hit leaves this
    // module — so no consumer of KbSearchHit.snippet can render an unescaped one.
    for h in &mut out {
        h.snippet = safe_snippet(Some(&h.snippet));
    }
    Ok(out)
}

/// The viewer searchDocs filters for — id + the author string (email ?? name).
pub struct SearchViewer<'a> {
    pub user_id: &'a str,
    pub who: Option<&'a str>,
}

pub fn guarded_of_space(sp: &KbSpace) -> Guarded {
    Guarded {
        owner_user_id: sp.owner_user_id.clone(),
        created_by: sp.created_by.clone(),
        visibility: sp.visibility.clone(),
        edit_policy: sp.edit_policy.clone(),
    }
}

// ── Backlinks ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbBacklink {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub space_id: String,
}

/// Docs that link to this one (editor links point at /knowledge/<id>).
pub async fn get_backlinks(pg: &PgPool, doc_id: &str) -> Result<Vec<KbBacklink>, sqlx::Error> {
    let pattern = format!("%/knowledge/{doc_id}%");
    let rows: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
        "select id::text, title, icon, space_id::text from kb_docs \
         where id <> $1::uuid and body like $2 \
         order by updated_at desc limit 50",
    )
    .bind(doc_id)
    .bind(&pattern)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, title, icon, space_id)| KbBacklink {
            id,
            title,
            icon,
            space_id,
        })
        .collect())
}

/// randomBytes(8).toString('hex') — the 16-char public slug, minted on first
/// publish. Shared with the artifacts plane (its first-publish slug mint is
/// the same shape).
pub(crate) fn random_slug() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("system rng");
    let mut out = String::with_capacity(16);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_snippet_is_the_xss_boundary() {
        // Raw HTML in the excerpt dies at the escape; sentinels survive it and
        // become the only tags in the output.
        let raw = format!("{HL_START}<img src=x onerror=alert(1)>{HL_STOP} plain & text");
        assert_eq!(
            safe_snippet(Some(&raw)),
            "<b>&lt;img src=x onerror=alert(1)&gt;</b> plain &amp; text"
        );
        // Null headline (no excerpt) — empty string, not "null".
        assert_eq!(safe_snippet(None), "");
        // A document that smuggled a literal sentinel INSIDE its own body
        // would get a bold tag — that is the documented trade: the sentinel is
        // a control-char-delimited marker no real document carries.
        assert_eq!(safe_snippet(Some("\u{2}hl\u{2}x")), "<b>x");
    }

    #[test]
    fn headline_opts_carry_the_sentinels() {
        assert!(HEADLINE_OPTS.contains("MaxWords=24, MinWords=8, ShortWord=3, MaxFragments=1"));
        assert!(HEADLINE_OPTS.contains("StartSel=\"\u{2}hl\u{2}\""));
        assert!(HEADLINE_OPTS.contains("StopSel=\"\u{2}/hl\u{2}\""));
    }

    #[test]
    fn slug_is_sixteen_hex_chars() {
        let s = random_slug();
        assert_eq!(s.len(), 16);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn okf_template_matches_the_ts_starter() {
        assert!(
            OKF_TEMPLATE.starts_with("---\ntitle:\nsummary:\ntags: []\nsource: talaria-kb\n---")
        );
        assert!(OKF_TEMPLATE.ends_with("## Overview\n\n## Facts\n\n## Procedures\n"));
    }
}
