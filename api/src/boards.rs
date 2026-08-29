// Boards — the port of ui/src/server/boards.ts, grown slice by slice. This
// file starts with the one piece the read families need (the visibility
// fragment); role resolution, CRUD, and the agent-policy fragment land with
// the boards route family's own slice.

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
}
