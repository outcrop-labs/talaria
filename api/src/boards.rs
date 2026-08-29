// Boards — the port of ui/src/server/boards.ts, grown slice by slice. This
// file starts with what earlier slices needed (the visibility fragment for the
// read families, role resolution for the runs watch gate); CRUD and the
// agent-policy fragment land with the boards route family's own slice.

/// SQL fragment: the board `b` is one this USER can see — a direct member, or
/// a member of the team that owns it — and not archived. `includeArchived`
/// drops the archival arm for `listBoards`, the one caller with a deliberate
/// view of retired boards (it states its own). The fragment is quoted SQL text
/// here (postgres.js composes it as a tagged fragment); the two placeholder
/// SPELLINGS are passed in because the fragment can sit at any position in a
/// larger query's bind order — TS binds the same userId value at each site.
pub fn board_visibility_sql(user_1: &str, user_2: &str, include_archived: bool) -> String {
    let arms = format!(
        "(exists (select 1 from board_members bvm where bvm.board_id = b.id and bvm.user_id = {user_1}::uuid) \
         or exists (select 1 from team_members tvm where tvm.team_id = b.team_id and tvm.user_id = {user_2}::uuid))"
    );
    if include_archived {
        arms
    } else {
        format!("(b.archived_at is null and {arms})")
    }
}

/// boards.ts RANK: which of two roles wins when both queries answer. The sort
/// is TS's `roles.sort((a, b) => RANK[b] - RANK[a])[0]` — descending by rank,
/// first wins.
fn role_rank(role: &str) -> i32 {
    match role {
        "owner" => 3,
        "editor" => 2,
        "viewer" => 1,
        // TS would compute RANK[unknown] - … as NaN and sort arbitrarily; the
        // map above is the whole declared set, and anything else ranks below
        // all of it rather than jittering the order.
        _ => 0,
    }
}

/// boards.ts boardRole: the caller's strongest role on a board. Two arms,
/// UNION ALL — a direct `board_members` row, and membership of the TEAM that
/// owns it (a team OWNER acts as board owner, any other team member as
/// editor) — then the RANK pick when both answer. Null = no relationship at
/// all. This is the one predicate every "may this person see this board"
/// question routes through; the runs watch gate (realtime.rs) is the caller
/// that crossed first.
pub async fn board_role(
    pg: &sqlx::PgPool,
    user_id: &str,
    board_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select role from board_members where board_id = $1::uuid and user_id = $2::uuid \
         union all \
         select case when tm.role = 'owner' then 'owner' else 'editor' end as role \
         from boards b join team_members tm on tm.team_id = b.team_id and tm.user_id = $2::uuid \
         where b.id = $1::uuid",
    )
    .bind(board_id)
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(role,)| role)
        .max_by_key(|role| role_rank(role)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_fragment_matches_the_ts_composition() {
        assert_eq!(
            board_visibility_sql("$1", "$2", false),
            "(b.archived_at is null and (exists (select 1 from board_members bvm where bvm.board_id = b.id and bvm.user_id = $1::uuid) or exists (select 1 from team_members tvm where tvm.team_id = b.team_id and tvm.user_id = $2::uuid)))"
        );
        assert_eq!(
            board_visibility_sql("$1", "$2", true),
            "(exists (select 1 from board_members bvm where bvm.board_id = b.id and bvm.user_id = $1::uuid) or exists (select 1 from team_members tvm where tvm.team_id = b.team_id and tvm.user_id = $2::uuid))"
        );
    }

    #[test]
    fn rank_orders_owner_over_editor_over_viewer() {
        // The RANK map TS sorts by (descending, stable — first wins): a direct
        // viewer plus team-editor answers editor, a direct viewer plus
        // team-owner answers owner.
        let mut roles = ["viewer", "editor"];
        roles.sort_by_key(|r| std::cmp::Reverse(role_rank(r)));
        assert_eq!(roles[0], "editor");
        let mut roles = ["viewer", "owner"];
        roles.sort_by_key(|r| std::cmp::Reverse(role_rank(r)));
        assert_eq!(roles[0], "owner");
    }
}
