// Comment threads on a document — Notion-shaped: a root comment may anchor to
// a QUOTE from it, replies thread under it, threads resolve.
//
// TWO KINDS OF DOCUMENT, ONE ENGINE. A knowledge-base doc and a Talaria
// artifact both get commented on, and a comment is a comment: the thread
// shape, the resolve rule, the quote anchor and the notification fan-out do
// not care which they hang off. So `CommentTarget` names the two and every
// function takes it, rather than a second table and a second set of routes
// that would drift from these within a release.
//
// Access rides the TARGET's own read permission — the doc's effective
// permissions (space-inherited + grants), or the artifact's. Anyone who can
// READ a thing can discuss it. Participants get notified.
//
// The engine takes the notification deps in (the comment fan-out is
// detached), so the routes decide which realtime plane a write publishes
// through.

use sqlx::PgPool;

use crate::{effective_doc_perms, get_doc};
use talaria_kb_perms::can_read;
use talaria_notify::{NotificationInput, NotifyDeps, add_notification};

/// One comment — field order is the wire order: the struct follows ROW_COLS,
/// which fixes the JSON key order.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbComment {
    pub id: String,
    /// Set on a knowledge-doc comment, null on an artifact's — the wire keeps
    /// both keys so a client can tell what it is looking at without a second
    /// read.
    pub doc_id: Option<String>,
    pub artifact_id: Option<String>,
    pub parent_id: Option<String>,
    pub author_user_id: Option<String>,
    pub author: String,
    pub quote: Option<String>,
    pub content: String,
    pub resolved: bool,
    pub created_at: String,
}

const ROW_COLS: &str = "id::text, doc_id::text, artifact_id::text, parent_id::text, author_user_id::text, \
                        author, quote, content, resolved, \
                        (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms";

#[derive(sqlx::FromRow)]
struct CommentRow {
    id: String,
    doc_id: Option<String>,
    artifact_id: Option<String>,
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
            artifact_id: r.artifact_id,
            parent_id: r.parent_id,
            author_user_id: r.author_user_id,
            author: r.author,
            quote: r.quote,
            content: r.content,
            resolved: r.resolved,
            created_at: talaria_agent_auth::epoch_ms_to_iso(r.created_ms),
        }
    }
}

/// What a comment hangs off. Copy, because it is two borrowed ids and every
/// function here takes one by value.
#[derive(Debug, Clone, Copy)]
pub enum CommentTarget<'a> {
    Doc(&'a str),
    Artifact(&'a str),
}

impl<'a> CommentTarget<'a> {
    /// The column this target filters and inserts on. Returned as a literal
    /// rather than interpolated from an id, so the SQL below stays a constant
    /// this crate wrote.
    fn column(self) -> &'static str {
        match self {
            CommentTarget::Doc(_) => "doc_id",
            CommentTarget::Artifact(_) => "artifact_id",
        }
    }

    fn id(self) -> &'a str {
        match self {
            CommentTarget::Doc(id) | CommentTarget::Artifact(id) => id,
        }
    }
}

// THE READ GATE LIVES WITH THE CALLER, not here. An artifact's permissions
// are `talaria-artifacts`' to answer, and that crate already depends on this
// one — asking it from here would be a dependency cycle. So each route gates
// its own target before calling in: the kb route with `can_discuss_doc`
// below, the artifact route with the `guarded(&artifact)` + `can_read` pair
// every other artifact route uses. The engine below takes an
// already-authorised target.

/// Gate: the viewer can read the doc (comments are part of the doc). Errors
/// (doc gone, perms unreadable) are false — fail closed.
pub async fn can_discuss_doc(pg: &PgPool, doc_id: &str, user_id: &str, who: Option<&str>) -> bool {
    let Ok(Some(doc)) = get_doc(pg, doc_id).await else {
        return false;
    };
    match effective_doc_perms(pg, &doc).await {
        Ok(eff) => {
            let team_ids = talaria_teams::team_ids_for_user(pg, user_id)
                .await
                .unwrap_or_default();
            can_read(&eff.perms, Some(user_id), who, &eff.grants, &team_ids)
        }
        Err(_) => false,
    }
}

pub async fn list_comments(
    pg: &PgPool,
    target: CommentTarget<'_>,
) -> Result<Vec<KbComment>, sqlx::Error> {
    // AssertSqlSafe: both interpolations are this crate's own literals —
    // ROW_COLS and the target's column name. The id is bound.
    let col = target.column();
    let sql = format!(
        "select {ROW_COLS} from kb_comments where {col} = $1::uuid order by created_at asc"
    );
    let rows: Vec<CommentRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(target.id())
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(KbComment::from).collect())
}

pub struct NewComment<'a> {
    pub target: CommentTarget<'a>,
    /// The target's title and owner, resolved by the CALLER. The engine cannot
    /// read an artifact (that crate depends on this one), and asking it to
    /// would be the cycle described at the top of this file. The caller has
    /// already loaded the row to gate the write, so it has both in hand.
    pub target_title: &'a str,
    pub target_owner_user_id: Option<&'a str>,
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
    // AssertSqlSafe: both interpolations are this crate's own literals —
    // ROW_COLS and the target's column name. Every value is bound.
    let col = input.target.column();
    let sql = format!(
        "insert into kb_comments ({col}, parent_id, author_user_id, author, quote, content) \
         values ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6) returning {ROW_COLS}"
    );
    let row: CommentRow = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(input.target.id())
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
        href: match input.target {
            CommentTarget::Doc(id) => format!("/knowledge?d={id}"),
            CommentTarget::Artifact(id) => format!("/artifacts?a={id}"),
        },
        title: input.target_title.to_string(),
        owner_user_id: input.target_owner_user_id.map(str::to_string),
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
    /// Where the notification points — the target's own place in the app.
    href: String,
    title: String,
    owner_user_id: Option<String>,
    parent_id: Option<String>,
    author_user_id: String,
    author: String,
    content: String,
}

impl CommentFanout {
    async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Deduped below: the owner may also be in the thread.
        let mut targets: Vec<String> = Vec::new();
        if let Some(owner) = &self.owner_user_id {
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
                    title: &format!(
                        "{} commented on \u{201c}{}\u{201d}",
                        self.author, self.title
                    ),
                    body: Some(&body),
                    href: Some(&self.href),
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
    let c: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "select doc_id::text, author_user_id::text, parent_id::text from kb_comments where id = $1::uuid",
    )
    .bind(comment_id)
    .fetch_optional(pg)
    .await?;
    let Some((doc_id, author_user_id, parent_id)) = c else {
        return Ok(false);
    };
    let root_id = parent_id.as_deref().unwrap_or(comment_id);
    // ON A KB DOC, its owner may resolve a thread they did not start. On an
    // ARTIFACT there is no owner check here, because reading the artifact is
    // this crate's cycle again — the author and the thread starter still can,
    // which is the rule that matters, and an artifact owner who wants a thread
    // gone can delete the comment through the route that already gates on the
    // artifact. Narrower than the doc case on purpose rather than by accident.
    let doc = match doc_id.as_deref() {
        Some(id) => get_doc(pg, id).await?,
        None => None,
    };
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
