// Comment threads on knowledge docs — Notion-shaped: a root comment may
// anchor to a QUOTE from the doc, replies thread under it, threads resolve.
// Access rides the doc's effective permissions (space-inherited + grants):
// anyone who can READ a doc can discuss it. Participants get notified.
//
// The engine takes the notification deps in (the comment fan-out is
// detached), so the routes decide which realtime plane a write publishes
// through.

use sqlx::PgPool;

use crate::kb::perms::can_read;
use crate::kb::{effective_doc_perms, get_doc};
use crate::notify::{NotificationInput, NotifyDeps, add_notification};

/// One comment — field order is the wire order: the struct follows ROW_COLS,
/// which fixes the JSON key order.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbComment {
    pub id: String,
    pub doc_id: String,
    pub parent_id: Option<String>,
    pub author_user_id: Option<String>,
    pub author: String,
    pub quote: Option<String>,
    pub content: String,
    pub resolved: bool,
    pub created_at: String,
}

const ROW_COLS: &str = "id::text, doc_id::text, parent_id::text, author_user_id::text, \
                        author, quote, content, resolved, \
                        (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms";

#[derive(sqlx::FromRow)]
struct CommentRow {
    id: String,
    doc_id: String,
    parent_id: Option<String>,
    author_user_id: Option<String>,
    author: String,
    quote: Option<String>,
    content: String,
    resolved: bool,
    created_ms: i64,
}

impl From<CommentRow> for KbComment {
    fn from(r: CommentRow) -> Self {
        KbComment {
            id: r.id,
            doc_id: r.doc_id,
            parent_id: r.parent_id,
            author_user_id: r.author_user_id,
            author: r.author,
            quote: r.quote,
            content: r.content,
            resolved: r.resolved,
            created_at: crate::agent_auth::epoch_ms_to_iso(r.created_ms),
        }
    }
}

/// Gate: the viewer can read the doc (comments are part of the doc). Errors
/// (doc gone, perms unreadable) are false — fail closed.
pub async fn can_discuss_doc(pg: &PgPool, doc_id: &str, user_id: &str, who: Option<&str>) -> bool {
    let Ok(Some(doc)) = get_doc(pg, doc_id).await else {
        return false;
    };
    match effective_doc_perms(pg, &doc).await {
        Ok(eff) => can_read(&eff.perms, Some(user_id), who, &eff.grants),
        Err(_) => false,
    }
}

pub async fn list_comments(pg: &PgPool, doc_id: &str) -> Result<Vec<KbComment>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's ROW_COLS column list.
    let sql = format!(
        "select {ROW_COLS} from kb_comments where doc_id = $1::uuid order by created_at asc"
    );
    let rows: Vec<CommentRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(doc_id)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(KbComment::from).collect())
}

pub struct NewComment<'a> {
    pub doc_id: &'a str,
    pub parent_id: Option<&'a str>,
    pub author_user_id: &'a str,
    pub author: &'a str,
    pub quote: Option<&'a str>,
    pub content: &'a str,
}

pub async fn add_comment(
    pg: &PgPool,
    notify: &NotifyDeps,
    input: &NewComment<'_>,
) -> Result<KbComment, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's ROW_COLS column list.
    let sql = format!(
        "insert into kb_comments (doc_id, parent_id, author_user_id, author, quote, content) \
         values ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6) returning {ROW_COLS}"
    );
    let row: CommentRow = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(input.doc_id)
        .bind(input.parent_id)
        .bind(input.author_user_id)
        .bind(input.author)
        .bind(input.quote)
        .bind(input.content)
        .fetch_one(pg)
        .await?;
    let comment = KbComment::from(row);

    // Notify the doc owner + everyone already in the thread (never the
    // author) — detached, whole-fanout best-effort: nothing about the
    // notification may fail the comment the user just typed.
    let fanout = CommentFanout {
        pg: pg.clone(),
        notify: notify.clone(),
        doc_id: input.doc_id.to_string(),
        parent_id: input.parent_id.map(str::to_string),
        author_user_id: input.author_user_id.to_string(),
        author: input.author.to_string(),
        content: input.content.to_string(),
    };
    tokio::spawn(async move {
        let _ = fanout.run().await;
    });

    Ok(comment)
}

/// The detached notification fanout for a fresh comment. Owned data — it runs
/// after the response is gone.
struct CommentFanout {
    pg: PgPool,
    notify: NotifyDeps,
    doc_id: String,
    parent_id: Option<String>,
    author_user_id: String,
    author: String,
    content: String,
}

impl CommentFanout {
    async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(doc) = get_doc(&self.pg, &self.doc_id).await? else {
            return Ok(());
        };
        // Deduped below: the owner may also be in the thread.
        let mut targets: Vec<String> = Vec::new();
        if let Some(owner) = &doc.owner_user_id {
            targets.push(owner.clone());
        }
        if let Some(parent) = &self.parent_id {
            let thread: Vec<(String,)> = sqlx::query_as(
                "select distinct author_user_id::text from kb_comments \
                 where (id = $1::uuid or parent_id = $1::uuid) and author_user_id is not null",
            )
            .bind(parent)
            .fetch_all(&self.pg)
            .await?;
            for (id,) in thread {
                targets.push(id);
            }
        }
        targets.retain(|t| t != &self.author_user_id);
        targets.sort();
        targets.dedup();
        // slice(0,140) by chars, not bytes — a UTF-8 boundary must never cut.
        let body: String = self.content.chars().take(140).collect();
        for user_id in &targets {
            let _ = add_notification(
                &self.notify,
                user_id,
                &NotificationInput {
                    kind: "kb-comment",
                    title: &format!("{} commented on \u{201c}{}\u{201d}", self.author, doc.title),
                    body: Some(&body),
                    href: Some(&format!("/knowledge?d={}", self.doc_id)),
                },
            )
            .await;
        }
        Ok(())
    }
}

/// Resolve/unresolve a whole thread (root id). Author, thread starter, or the
/// doc owner may do it. False = not found or not allowed (the route's 403).
pub async fn set_resolved(
    pg: &PgPool,
    comment_id: &str,
    resolved: bool,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let c: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "select doc_id::text, author_user_id::text, parent_id::text from kb_comments where id = $1::uuid",
    )
    .bind(comment_id)
    .fetch_optional(pg)
    .await?;
    let Some((doc_id, author_user_id, parent_id)) = c else {
        return Ok(false);
    };
    let root_id = parent_id.as_deref().unwrap_or(comment_id);
    let doc = get_doc(pg, &doc_id).await?;
    let root: Option<(Option<String>,)> =
        sqlx::query_as("select author_user_id::text from kb_comments where id = $1::uuid")
            .bind(root_id)
            .fetch_optional(pg)
            .await?;
    let may = author_user_id.as_deref() == Some(user_id)
        || root.and_then(|(a,)| a).as_deref() == Some(user_id)
        || doc.as_ref().and_then(|d| d.owner_user_id.as_deref()) == Some(user_id);
    if !may {
        return Ok(false);
    }
    sqlx::query("update kb_comments set resolved = $2 where id = $1::uuid or parent_id = $1::uuid")
        .bind(root_id)
        .bind(resolved)
        .execute(pg)
        .await?;
    Ok(true)
}

/// Delete own comment (its replies cascade). False = not found or not the
/// author's.
pub async fn delete_comment(
    pg: &PgPool,
    comment_id: &str,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let n = sqlx::query(
        "delete from kb_comments where id = $1::uuid and author_user_id = $2::uuid returning 1",
    )
    .bind(comment_id)
    .bind(user_id)
    .execute(pg)
    .await?
    .rows_affected();
    Ok(n > 0)
}
