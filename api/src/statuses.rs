// Statuses — the port of ui/src/server/statuses.ts, grown slice by slice. This
// file starts with the one fragment the approvals census needs (a ticket is in
// a review-category column); the defaults, materialization, and validation
// planes land with the boards family's own batch.

/// SQL fragment: `t.status` is in the given CATEGORY on the ticket's own board,
/// with the legacy fallback for never-customized boards (statuses.ts
/// statusCategorySql).
///
/// Interpolate only with a LITERAL category + fallback list — no user input —
/// which is the same contract the TS template carries; the callers here are
/// compile-time strings.
pub fn status_category_sql(category: &str, legacy_keys: &[&str]) -> String {
    let keys = legacy_keys
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "( t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and \
         bs.category = '{category}') or ( not exists (select 1 from board_statuses bs where \
         bs.board_id = t.board_id) and t.status in ({keys}) ) )"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_matches_the_ts_composition() {
        assert_eq!(
            status_category_sql("review", &["quality_review"]),
            "( t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id \
             and bs.category = 'review') or ( not exists (select 1 from board_statuses bs where \
             bs.board_id = t.board_id) and t.status in ('quality_review') ) )"
        );
        // A second legacy key joins with a comma, as the TS `.map().join()` does.
        assert!(status_category_sql("done", &["done", "closed"]).contains("'done', 'closed'"));
    }
}
