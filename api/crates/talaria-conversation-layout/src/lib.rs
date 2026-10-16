// HOW ONE PERSON KEEPS THEIR OWN LIST OF CONVERSATIONS — pinned to the top,
// filed under a group, held at a position they chose.
//
// PER VIEWER, WHICH IS THE WHOLE DESIGN. A conversation has an owner and may
// have collaborators, and what each of them wants at the top of their list is
// nobody else's business: a shared plan one person lives in all week is
// another's background noise. So the layout is not a column on
// `conversations` — it is a row per (viewer, conversation), and a write only
// ever touches the writer's own.
//
// MERGE, NOT REPLACE. Every field is optional and absent means "leave it
// alone", because the callers are a PATCH body and an agent tool that each
// carry only what changed. `Option<Option<T>>` says that twice over: the
// outer None is absent, `Some(None)` is the explicit clear a person asked for
// (take it out of its group), and only `Some(Some(v))` sets a value.
//
// A ROW THAT SAYS NOTHING IS DELETED. Unpinning, ungrouping and clearing the
// position leaves a row carrying the defaults every conversation already has,
// and a table of those is a table that grows for ever and answers nothing.

use serde_json::{Map, Value};
use sqlx::PgPool;
use talaria_body::{optional_boolean_member, trimmed_string_member};

/// A group name is a shelf label, not a description: it has to fit a list
/// row beside the conversation's own title.
pub const MAX_GROUP: usize = 40;
/// The far end of a hand-ordered list. Large enough that a caller can leave
/// gaps to insert into, small enough that it cannot be used as a counter.
pub const MAX_POSITION: i32 = 100_000;

/// What one write means to change. See the header for why each field is an
/// `Option` and the two nullable ones are an `Option<Option<_>>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutPatch {
    pub pinned: Option<bool>,
    pub group: Option<Option<String>>,
    pub position: Option<Option<i32>>,
}

impl LayoutPatch {
    /// Nothing to do. The callers answer 400 on this rather than writing a
    /// row that changes nothing.
    pub fn is_empty(&self) -> bool {
        self.pinned.is_none() && self.group.is_none() && self.position.is_none()
    }
}

/// Read the three layout fields out of a request body, field by field, in the
/// order a reader of the body would meet them. The messages are the MODEL's
/// to recover from as much as the client's, so each one says what the field
/// takes rather than naming a schema.
pub fn patch_from(obj: &Map<String, Value>) -> Result<LayoutPatch, String> {
    let pinned = optional_boolean_member(obj, "pinned")?;
    let group = match obj.get("group") {
        None => None,
        // null is the explicit clear: take it out of whatever group it is in.
        Some(Value::Null) => Some(None),
        Some(_) => Some(Some(trimmed_string_member(obj, "group", 1, MAX_GROUP)?)),
    };
    let position = match obj.get("position") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => {
            let n = v.as_i64().ok_or_else(|| {
                "Invalid input: expected position to be a whole number".to_string()
            })?;
            if !(0..=i64::from(MAX_POSITION)).contains(&n) {
                return Err(format!(
                    "Invalid input: expected position between 0 and {MAX_POSITION}"
                ));
            }
            Some(Some(n as i32))
        }
    };
    let patch = LayoutPatch {
        pinned,
        group,
        position,
    };
    if patch.is_empty() {
        return Err("nothing to change — give pinned, group or position".into());
    }
    Ok(patch)
}

/// Merge the patch into this viewer's row, and delete the row if what is left
/// is what every conversation already looks like.
///
/// READ, MERGE, WRITE rather than one clever upsert: the merge has to know the
/// values it did NOT receive to decide whether the row still says anything,
/// and the only writer of a row is the one person it belongs to.
pub async fn write_layout(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
    patch: &LayoutPatch,
) -> Result<(), sqlx::Error> {
    let current: Option<(bool, Option<String>, Option<i32>)> = sqlx::query_as(
        "select pinned, group_name, position from conversation_layout \
         where user_id = $1::uuid and conversation_id = $2::uuid",
    )
    .bind(user_id)
    .bind(conversation_id)
    .fetch_optional(pg)
    .await?;
    let (mut pinned, mut group, mut position) = current.unwrap_or((false, None, None));
    if let Some(v) = patch.pinned {
        pinned = v;
    }
    if let Some(v) = &patch.group {
        group = v.clone();
    }
    if let Some(v) = patch.position {
        position = v;
    }
    if !pinned && group.is_none() && position.is_none() {
        sqlx::query(
            "delete from conversation_layout \
             where user_id = $1::uuid and conversation_id = $2::uuid",
        )
        .bind(user_id)
        .bind(conversation_id)
        .execute(pg)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "insert into conversation_layout \
             (user_id, conversation_id, pinned, group_name, position, updated_at) \
         values ($1::uuid, $2::uuid, $3, $4, $5, now()) \
         on conflict (user_id, conversation_id) do update set \
             pinned = excluded.pinned, \
             group_name = excluded.group_name, \
             position = excluded.position, \
             updated_at = now()",
    )
    .bind(user_id)
    .bind(conversation_id)
    .bind(pinned)
    .bind(group)
    .bind(position)
    .execute(pg)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use talaria_body::{too_big_msg, too_small_msg};

    fn read(v: Value) -> Result<LayoutPatch, String> {
        patch_from(v.as_object().expect("object"))
    }

    #[test]
    fn reads_the_three_fields_and_tells_absent_from_cleared() {
        let p = read(json!({ "pinned": true, "group": "Billing", "position": 3 })).expect("read");
        assert_eq!(p.pinned, Some(true));
        assert_eq!(p.group, Some(Some("Billing".into())));
        assert_eq!(p.position, Some(Some(3)));

        // ABSENT IS NOT CLEARED. A body carrying only `pinned` must leave the
        // group where it is, or every pin would unfile the conversation.
        let only = read(json!({ "pinned": false })).expect("read");
        assert_eq!(only.pinned, Some(false));
        assert_eq!(only.group, None);
        assert_eq!(only.position, None);

        // null is the clear, and reads as a present field.
        let cleared = read(json!({ "group": null, "position": null })).expect("read");
        assert_eq!(cleared.group, Some(None));
        assert_eq!(cleared.position, Some(None));
    }

    #[test]
    fn refuses_a_body_that_asks_for_nothing() {
        let err = read(json!({})).expect_err("nothing to change");
        assert!(err.contains("nothing to change"), "{err}");
    }

    #[test]
    fn holds_the_group_bounds_and_trims_before_measuring() {
        let padded = read(json!({ "group": "  Billing  " })).expect("read");
        assert_eq!(padded.group, Some(Some("Billing".into())));

        // Spaces only is the min failure, not an empty group name.
        let blank = read(json!({ "group": "   " })).expect_err("blank");
        assert_eq!(blank, too_small_msg(1));

        let long = read(json!({ "group": "g".repeat(MAX_GROUP + 1) })).expect_err("too long");
        assert_eq!(long, too_big_msg(MAX_GROUP));

        let wrong = read(json!({ "group": 7 })).expect_err("not a string");
        assert!(wrong.contains("expected string"), "{wrong}");
    }

    #[test]
    fn holds_the_position_bounds() {
        assert_eq!(
            read(json!({ "position": 0 })).expect("read").position,
            Some(Some(0))
        );
        assert_eq!(
            read(json!({ "position": MAX_POSITION }))
                .expect("read")
                .position,
            Some(Some(MAX_POSITION))
        );
        for bad in [json!(-1), json!(1.5), json!("3"), json!(MAX_POSITION + 1)] {
            let err = read(json!({ "position": bad })).expect_err("refused");
            assert!(
                err.contains("position"),
                "a position message must name the field: {err}"
            );
        }
    }

    #[test]
    fn refuses_a_pinned_that_is_not_a_boolean() {
        let err = read(json!({ "pinned": "yes" })).expect_err("not a boolean");
        assert!(err.contains("expected boolean"), "{err}");
    }
}
