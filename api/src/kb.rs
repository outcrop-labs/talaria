// The knowledgebase — an Outline-style markdown drive. Spaces group docs; docs
// nest. Every save snapshots a version (reusing internal_versions). Marking a
// doc "official" indexes it into the org-kb RAG collection so agents ground on
// it; un-officializing / deleting removes it.
//
// Port of ui/src/server/kb.ts, read half: the row shapes and the two readers
// (getDoc/getSpace) plus the effective-permission resolution the refs cone
// ACL-checks against. The drive's write plane (create/save/delete, the
// official→RAG indexing legs) is batch 5 and extends this file in place.

use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;
use crate::kb_perms::{EditorGrant, Guarded, ITEM_DOC, ITEM_SPACE, list_editors};

/// A KB space (kb.ts KbSpace) — full row shape, so batch 5 only adds
/// functions, never fields.
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
    pub edit_policy: String,
    pub owner_user_id: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
}

/// A KB doc (kb.ts KbDoc) — full row shape, same reasoning as KbSpace.
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
    pub owner_user_id: Option<String>,
    pub sort: i32,
    /// RAG routing: 'auto' (space binding / org rules) | 'none' | a brain id.
    pub rag_routing: String,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub updated_at: String,
}

const SPACE_COLS: &str = "id::text, name, description, icon, body, visibility, public_slug, \
                          edit_policy, owner_user_id::text, created_by::text, \
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
            edit_policy,
            owner_user_id,
            created_by,
            created_at: epoch_ms_to_iso(created_ms),
        }
    }
}

const DOC_COLS: &str = "id::text, space_id::text, parent_id::text, title, icon, body, okf, kind, official, \
                        visibility, public_slug, edit_policy, perms_inherited, owner_user_id::text, sort, \
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
    owner_user_id: Option<String>,
    sort: i32,
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
            owner_user_id,
            sort,
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
            owner_user_id,
            sort,
            rag_routing,
            created_by,
            updated_by,
            updated_at: epoch_ms_to_iso(updated_ms),
        }
    }
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

pub async fn get_doc(pg: &PgPool, id: &str) -> Result<Option<KbDoc>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's DOC_COLS column list.
    let sql = format!("select {DOC_COLS} from kb_docs where id = $1::uuid");
    let row: Option<DocRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(KbDoc::from))
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
