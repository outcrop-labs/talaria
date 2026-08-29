// Templates — the port of ui/src/server/templates.ts, grown slice by slice.
// This file starts with the BOARD-BINDINGS half the boards family serves
// (which ticket templates a board uses, and its default); the org-wide
// library CRUD (/api/templates, batch 5) and the resolution chain the engine
// reads (resolveTemplate/templatePrompt — the tasks slice) land with their
// own routes.

use sqlx::PgPool;

/// A board's template binding (templates.ts BoardTemplateBinding): which
/// ticket template the board uses, and whether it is the default.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardTemplateBinding {
    pub template_id: String,
    pub is_default: bool,
}

/// The board's bindings — TS's select carries no ORDER BY, and the client
/// treats the set as a set, so neither does this.
pub async fn board_templates(
    pg: &PgPool,
    board_id: &str,
) -> Result<Vec<BoardTemplateBinding>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, bool)>(
        "select template_id::text, is_default from board_templates where board_id = $1::uuid",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(template_id, is_default)| BoardTemplateBinding {
            template_id,
            is_default,
        })
        .collect())
}

/// Replace a board's template set (templates.ts setBoardTemplates). One
/// transaction: delete, then re-insert each id with is_default set by
/// identity with `default_id`. Duplicates in the list are swallowed by the
/// conflict clause, exactly as TS's `on conflict do nothing` does.
pub async fn set_board_templates(
    pg: &PgPool,
    board_id: &str,
    template_ids: &[String],
    default_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("delete from board_templates where board_id = $1::uuid")
        .bind(board_id)
        .execute(&mut *tx)
        .await?;
    for id in template_ids {
        sqlx::query(
            "insert into board_templates (board_id, template_id, is_default) \
             values ($1::uuid, $2::uuid, $3) on conflict do nothing",
        )
        .bind(board_id)
        .bind(id)
        .bind(default_id == Some(id.as_str()))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}
