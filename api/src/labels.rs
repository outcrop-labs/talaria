// Board labels — first-class, colored,
// scoped to a board. Task.tags stays a plain string array of label NAMES
// (agents and old data keep working); ensure_labels auto-registers any name
// that reaches a ticket, so the registry is always complete and always the
// place to manage from. Renames cascade into every ticket's tags; deletes
// strip the label off tickets.

use crate::realtime::{BoardEvent, RealtimeDeps, publish_board};
use sqlx::PgPool;

/// The color palette — the whole declared set a
/// label's color may be; anything else coerces to 'slate' on create and is
/// refused on update.
pub const LABEL_COLOR_KEYS: &[&str] = &[
    "slate", "bronze", "green", "amber", "red", "blue", "purple", "teal", "pink", "orange", "lime",
    "cyan", "indigo", "magenta", "olive", "brown",
];

/// The wire row — the ROW select's key order: id,
/// boardId, name, color, position. No timestamps on this table.
#[derive(serde::Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BoardLabel {
    pub id: String,
    pub board_id: String,
    pub name: String,
    pub color: String,
    pub position: i32,
}

type LabelRow = (String, String, String, String, i32);

pub async fn list_labels(pg: &PgPool, board_id: &str) -> Result<Vec<BoardLabel>, sqlx::Error> {
    let rows: Vec<LabelRow> = sqlx::query_as(
        "select id::text, board_id::text, name, color, position \
         from board_labels where board_id = $1::uuid \
         order by position, lower(name)",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, board_id, name, color, position)| BoardLabel {
            id,
            board_id,
            name,
            color,
            position,
        })
        .collect())
}

/// Create (or recolor). The inner Err is the route's 400 'label name
/// required' — a name that is only whitespace is caught HERE, not by the
/// route's schema. An unknown color COERCES to slate
/// (create is forgiving; update is not). Re-creating a name is a recolor: the
/// upsert updates the color and returns the row.
pub async fn create_label(
    pg: &PgPool,
    board_id: &str,
    name: &str,
    color: Option<&str>,
) -> Result<Result<BoardLabel, String>, sqlx::Error> {
    let n = name.trim();
    if n.is_empty() {
        return Ok(Err("label name required".into()));
    }
    let c = color
        .filter(|c| LABEL_COLOR_KEYS.contains(c))
        .unwrap_or("slate");
    let row: LabelRow = sqlx::query_as(
        "insert into board_labels (board_id, name, color) values ($1::uuid, $2, $3) \
         on conflict (board_id, name) do update set color = excluded.color \
         returning id::text, board_id::text, name, color, position",
    )
    .bind(board_id)
    .bind(n)
    .bind(c)
    .fetch_one(pg)
    .await?;
    let (id, board_id, name, color, position) = row;
    Ok(Ok(BoardLabel {
        id,
        board_id,
        name,
        color,
        position,
    }))
}

/// Register any names that reached a ticket but
/// aren't labels yet (agents, MCP, old callers). Keeps free-string writes
/// working AND manageable. The tasks family's write paths call this.
pub async fn ensure_labels(
    pg: &PgPool,
    board_id: &str,
    names: &[String],
) -> Result<(), sqlx::Error> {
    // trimmed, empties dropped, first occurrence wins.
    let mut clean: Vec<String> = Vec::new();
    for n in names {
        let t = n.trim();
        if !t.is_empty() && !clean.iter().any(|c| c == t) {
            clean.push(t.to_string());
        }
    }
    for n in &clean {
        sqlx::query(
            "insert into board_labels (board_id, name) values ($1::uuid, $2) \
             on conflict do nothing",
        )
        .bind(board_id)
        .bind(n)
        .execute(pg)
        .await?;
    }
    Ok(())
}

/// Update. The inner Errs ('no such label', 'unknown color') are the route's
/// 400s — and they fire BEFORE
/// any write, so a bad color changes nothing. A rename CASCADES into every
/// ticket carrying the old name; a color-only (or empty) patch still
/// publishes, because the function's end is the publish.
pub async fn update_label(
    pg: &PgPool,
    realtime: &RealtimeDeps,
    board_id: &str,
    label_id: &str,
    name: Option<&str>,
    color: Option<&str>,
) -> Result<Result<(), String>, sqlx::Error> {
    let cur: Option<(String,)> =
        sqlx::query_as("select name from board_labels where id = $1::uuid and board_id = $2::uuid")
            .bind(label_id)
            .bind(board_id)
            .fetch_optional(pg)
            .await?;
    let Some((cur_name,)) = cur else {
        return Ok(Err("no such label".into()));
    };
    if let Some(color) = color {
        if !LABEL_COLOR_KEYS.contains(&color) {
            return Ok(Err("unknown color".into()));
        }
        sqlx::query("update board_labels set color = $1 where id = $2::uuid")
            .bind(color)
            .bind(label_id)
            .execute(pg)
            .await?;
    }
    if let Some(next) = name
        .map(str::trim)
        .filter(|n| !n.is_empty() && *n != cur_name)
    {
        sqlx::query("update board_labels set name = $1 where id = $2::uuid")
            .bind(next)
            .bind(label_id)
            .execute(pg)
            .await?;
        // Rename cascades into every ticket carrying the old name.
        sqlx::query(
            "update tasks set tags = to_jsonb(array( \
                select case when e = $1 then $2 else e end \
                from jsonb_array_elements_text(tags) as e \
             )), updated_at = now() \
             where board_id = $3::uuid and tags ? $1",
        )
        .bind(&cur_name)
        .bind(next)
        .bind(board_id)
        .execute(pg)
        .await?;
    }
    publish_board(
        realtime,
        board_id,
        &BoardEvent {
            kind_tag: "board",
            task_id: None,
            deleted: None,
        },
    );
    Ok(Ok(()))
}

/// Delete — a miss is a quiet Ok (already gone; the outcome
/// the caller wanted), a hit strips the label off every ticket. Both write
/// paths publish the board bump.
pub async fn delete_label(
    pg: &PgPool,
    realtime: &RealtimeDeps,
    board_id: &str,
    label_id: &str,
) -> Result<(), sqlx::Error> {
    let cur: Option<(String,)> =
        sqlx::query_as("select name from board_labels where id = $1::uuid and board_id = $2::uuid")
            .bind(label_id)
            .bind(board_id)
            .fetch_optional(pg)
            .await?;
    let Some((cur_name,)) = cur else {
        return Ok(());
    };
    sqlx::query("delete from board_labels where id = $1::uuid")
        .bind(label_id)
        .execute(pg)
        .await?;
    sqlx::query(
        "update tasks set tags = to_jsonb(array( \
            select e from jsonb_array_elements_text(tags) as e where e <> $1 \
         )), updated_at = now() \
         where board_id = $2::uuid and tags ? $1",
    )
    .bind(&cur_name)
    .bind(board_id)
    .execute(pg)
    .await?;
    publish_board(
        realtime,
        board_id,
        &BoardEvent {
            kind_tag: "board",
            task_id: None,
            deleted: None,
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_keys_are_pinned_in_order() {
        assert_eq!(LABEL_COLOR_KEYS.len(), 16);
        assert_eq!(LABEL_COLOR_KEYS[0], "slate");
        assert_eq!(LABEL_COLOR_KEYS[15], "brown");
    }
}
