// THE WIRE SHAPE of an edit batch, read once for everybody.
//
// WHO SHARES THIS, and why it is not left in the route. The route parses what
// an agent POSTs; the fitness sandbox parses what a candidate model *would*
// have POSTed, to grade whether it can use the tool. If those two readings
// differ at all then the benchmark is measuring a tool Talaria does not have —
// a model that passes in the sandbox and 400s in production gets called
// competent, which is the exact failure the toolbox invariants exist to stop.
// So the refusals below are the only refusals, and both callers get them.

use serde_json::{Map, Value};
use talaria_body::{array_msg, array_too_big_msg, object_msg, string_msg, zod_type_name};

use crate::Edit;

/// One call's worth of edits. Generous enough for a real revision pass,
/// bounded so a runaway model cannot hand us a megabyte of patch.
pub const MAX_EDITS: usize = 50;
pub const MAX_TEXT: usize = 200_000;

/// Read one element. An element carrying neither `oldString` nor `section` is
/// refused BY NAME rather than as a generic "invalid element": the single most
/// likely model error on a new tool is reaching for the field the other tool
/// uses (`markdown` alone, as `update_document` takes), and a message that
/// says which two shapes exist is a one-turn fix.
pub fn edit_from(el: &Value) -> Result<Edit, String> {
    let obj = el
        .as_object()
        .ok_or_else(|| object_msg(zod_type_name(el)))?;
    let has_old = obj.contains_key("oldString");
    let has_section = obj.contains_key("section");
    if has_old && has_section {
        return Err("an edit is either oldString/newString or section/markdown, not both".into());
    }
    let text = |key: &str| -> Result<String, String> {
        match obj.get(key) {
            // Absent is the empty string on purpose: omitting `newString` is
            // how a model deletes, and requiring it would only push it to
            // send `""` — the same thing with one more way to get it wrong.
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) if s.len() > MAX_TEXT => Err(format!(
                "{key} is longer than {MAX_TEXT} characters — split the edit, \
                 or use update_document to replace the whole body"
            )),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(other) => Err(format!("{key}: {}", string_msg(zod_type_name(other)))),
        }
    };
    if has_old {
        return Ok(Edit::Replace {
            old: text("oldString")?,
            new: text("newString")?,
        });
    }
    if has_section {
        return Ok(Edit::Section {
            heading: text("section")?,
            markdown: text("markdown")?,
        });
    }
    Err(
        "each edit needs either oldString (with newString) to replace exact text, \
         or section (with markdown) to rewrite the body under a heading"
            .into(),
    )
}

/// Read the `edits` member of a request object.
pub fn edits_from(obj: &Map<String, Value>) -> Result<Vec<Edit>, String> {
    let v = obj.get("edits").ok_or_else(|| array_msg("undefined"))?;
    edits_from_value(v)
}

/// Read an `edits` array that is already in hand — the sandbox has the
/// argument object, not the request body.
pub fn edits_from_value(v: &Value) -> Result<Vec<Edit>, String> {
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    if arr.is_empty() {
        return Err("edits is empty — nothing to do".into());
    }
    if arr.len() > MAX_EDITS {
        return Err(array_too_big_msg(MAX_EDITS));
    }
    arr.iter().map(edit_from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edits_of(v: Value) -> Result<Vec<Edit>, String> {
        edits_from(v.as_object().expect("object"))
    }

    #[test]
    fn reads_both_shapes_in_order() {
        let got = edits_of(json!({
            "edits": [
                { "oldString": "a", "newString": "b" },
                { "section": "## S", "markdown": "x" },
            ]
        }))
        .expect("both shapes");
        assert_eq!(
            got,
            vec![
                Edit::Replace {
                    old: "a".into(),
                    new: "b".into()
                },
                Edit::Section {
                    heading: "## S".into(),
                    markdown: "x".into()
                }
            ]
        );
    }

    #[test]
    fn a_missing_new_string_is_a_deletion_not_an_error() {
        let got = edits_of(json!({ "edits": [{ "oldString": "gone" }] })).expect("deletion");
        assert_eq!(
            got,
            vec![Edit::Replace {
                old: "gone".into(),
                new: String::new()
            }]
        );
    }

    #[test]
    fn markdown_alone_is_refused_by_name() {
        // The likeliest model error: reaching for `update_document`'s field.
        // "invalid element" would leave it guessing at the schema it just got
        // wrong, so the message names both shapes.
        let err =
            edits_of(json!({ "edits": [{ "markdown": "the whole doc" }] })).expect_err("no anchor");
        assert!(err.contains("oldString"), "{err}");
        assert!(err.contains("section"), "{err}");
    }

    #[test]
    fn both_shapes_at_once_is_refused() {
        let err = edits_of(json!({ "edits": [{ "oldString": "a", "section": "## S" }] }))
            .expect_err("ambiguous shape");
        assert!(err.contains("not both"), "{err}");
    }

    #[test]
    fn an_empty_or_missing_or_oversized_list_is_refused() {
        assert!(
            edits_of(json!({ "edits": [] }))
                .expect_err("empty")
                .contains("empty")
        );
        assert!(edits_of(json!({})).is_err());
        assert!(edits_of(json!({ "edits": { "oldString": "a" } })).is_err());
        let many: Vec<Value> = (0..=MAX_EDITS)
            .map(|_| json!({ "oldString": "a" }))
            .collect();
        assert!(edits_of(json!({ "edits": many })).is_err());
    }

    #[test]
    fn a_non_string_field_reports_the_type_it_got() {
        let err = edits_of(json!({ "edits": [{ "oldString": 7 }] })).expect_err("number");
        assert!(err.contains("oldString"), "{err}");
    }

    #[test]
    fn an_oversized_edit_points_at_the_tool_that_can_take_it() {
        let huge = "x".repeat(MAX_TEXT + 1);
        let err = edits_of(json!({ "edits": [{ "oldString": huge, "newString": "y" }] }))
            .expect_err("too big");
        assert!(err.contains("update_document"), "{err}");
    }
}
