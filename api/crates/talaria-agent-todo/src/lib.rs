// THE AGENT'S OWN PLAN for a conversation — what it decided to do, in order,
// and which step it is on.
//
// WHY THIS EXISTS. Talaria's agents carry 86 tools and had no way to say what
// they intended to do with them. So a multi-step request — "read the spec,
// draft the tickets, then update the doc" — looked from the outside like
// silence followed by an answer, and the one question a person actually has
// while waiting ("is it doing the thing I asked, and how far through is it?")
// had no answer anywhere in the product. The agent was deciding; nothing could
// see the decision. This is the decision, written down where the surface can
// render it.
//
// WHOSE PLAN IT IS. The AGENT's, not the person's, and not the board's. It is
// not a ticket list and it must never grow into one: tickets are durable work
// a human assigns and signs off, and `plan_drafts` already turns a
// conversation into them. This is scratch reasoning with a lifetime of one
// request, which is exactly why it is replaced wholesale rather than
// accumulated.
//
// WHY NOT HERMES' OWN `todo_list`, YET. Hermes ships one, and it is closed
// for every agent (talaria-agent-defs' `MANDATORY_DISABLED`). Its list lives in
// an in-memory store on the agent, and v0.22's API server builds a fresh agent
// per request, so the list survives a turn only through the transcript: the
// newest todo tool result in the history is replayed into the store
// (`_hydrate_todo_store`). On a Hermes SESSION that works, because the history
// is the session's own and holds the tool rows. On the COMPLETIONS arm, which is
// every chat of an agent without pinned sessions and the default, the history
// is the request body: Talaria sends content only, and Hermes' parser keeps
// only system/user/assistant content anyway. The list would reset every turn.
// Enabling it for pinned agents alone is no answer either: the same agent's
// work sessions and its chats would then carry two todo tools.
//
// UNBLOCKED WHEN the session arm is the chat default (`pinned_sessions`
// defaulting on). Then: drop `todo` from `MANDATORY_DISABLED` (Rust and the
// UI mirror); have the talaria-events plugin PUT each `todo_list` result to
// `/api/agent/todo` for its conversation, unclamped up to `MAX_ITEMS`
// (`in_progress` → active, `completed` → done, `cancelled` dropped, `parent`
// flattened); keep `conversation_todos`, the read route and the plan strip as
// the projection; and retire the MCP `todo` tool a release later.
//
// REPLACE, NOT PATCH. Every call carries the whole list. A patch protocol
// (complete item 3, add an item after 2) is more state for a model to track
// across turns and one more thing for it to get wrong — and it buys nothing,
// because a plan is a handful of short lines, not a document. This is the same
// call every coding agent's todo tool makes, so models are practised at it.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgPool;
use talaria_body::{array_msg, array_too_big_msg, object_msg, zod_type_name};

/// A plan of more than this many steps is not a plan — it is a backlog, and a
/// backlog belongs on a board where a person can own and sign off each line.
pub const MAX_ITEMS: usize = 30;
/// One step, not one paragraph. The strip that renders these has one line per
/// item; past this it stops being scannable, which is the only thing it is for.
pub const MAX_TEXT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Active,
    Done,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Active => "active",
            Status::Done => "done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub text: String,
    pub status: Status,
}

/// Read the wire shape. Shared by the route and the fitness sandbox for the
/// same reason the document editor's reader is: a sandbox that accepts what
/// production refuses grades a model on a tool Talaria does not have.
pub fn items_from(obj: &Map<String, Value>) -> Result<Vec<Item>, String> {
    let v = obj.get("items").ok_or_else(|| array_msg("undefined"))?;
    items_from_value(v)
}

pub fn items_from_value(v: &Value) -> Result<Vec<Item>, String> {
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    if arr.len() > MAX_ITEMS {
        return Err(array_too_big_msg(MAX_ITEMS));
    }
    let mut out = Vec::with_capacity(arr.len());
    for (i, el) in arr.iter().enumerate() {
        let o = el
            .as_object()
            .ok_or_else(|| object_msg(zod_type_name(el)))?;
        let text = match o.get("text") {
            Some(Value::String(s)) if !s.trim().is_empty() => {
                if s.chars().count() > MAX_TEXT {
                    return Err(format!(
                        "item {}: text is longer than {MAX_TEXT} characters — a step is one line",
                        i + 1
                    ));
                }
                s.trim().to_string()
            }
            // A step with no words is the one item nobody can act on and
            // nobody can read, so it is refused rather than stored blank.
            _ => {
                return Err(format!(
                    "item {}: text is required and cannot be blank",
                    i + 1
                ));
            }
        };
        let status = match o.get("status") {
            // Absent means pending: a model listing out a fresh plan should
            // not have to stamp every line, and "pending" is the only thing an
            // unstamped step can mean.
            None | Some(Value::Null) => Status::Pending,
            Some(Value::String(s)) => match s.as_str() {
                "pending" => Status::Pending,
                "active" => Status::Active,
                "done" => Status::Done,
                other => {
                    return Err(format!(
                        "item {}: status \"{other}\" is not one of pending, active, done",
                        i + 1
                    ));
                }
            },
            Some(other) => {
                return Err(format!(
                    "item {}: status must be a string, got {}",
                    i + 1,
                    zod_type_name(other)
                ));
            }
        };
        out.push(Item { text, status });
    }
    // ONE THING AT A TIME, and the refusal is cheap to recover from. The strip
    // this renders answers "what is it doing right now" — a question with one
    // answer or none. Two active steps do not make the strip twice as
    // informative, they make it unable to say anything, so the model is told
    // the rule once and fixes it on the next call. Zero active is fine: a plan
    // can be freshly written, or finished.
    let active = out.iter().filter(|i| i.status == Status::Active).count();
    if active > 1 {
        return Err(format!(
            "{active} items are active — at most one step may be active at a time. \
             Mark the one you are on as active and leave the rest pending or done"
        ));
    }
    Ok(out)
}

/// Replace this conversation's plan. Upsert, because the whole contract is
/// "the latest call is the plan".
pub async fn save(
    pg: &PgPool,
    conversation_id: &str,
    items: &[Item],
    actor: &str,
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_value(items).unwrap_or_else(|_| Value::Array(vec![]));
    sqlx::query(
        "insert into conversation_todos (conversation_id, items, updated_by) \
         values ($1::uuid, $2, $3) \
         on conflict (conversation_id) do update set \
           items = excluded.items, \
           updated_by = excluded.updated_by, \
           updated_at = now()",
    )
    .bind(conversation_id)
    .bind(json)
    .bind(actor)
    .execute(pg)
    .await?;
    Ok(())
}

/// This conversation's plan, or an empty list when the agent never wrote one.
/// An absent row and an empty plan read the same on purpose — the surface
/// renders nothing in both cases, and a caller that had to tell them apart
/// would be asking a question with no consequence.
pub async fn load(pg: &PgPool, conversation_id: &str) -> Result<Vec<Item>, sqlx::Error> {
    let row: Option<(Value,)> =
        sqlx::query_as("select items from conversation_todos where conversation_id = $1::uuid")
            .bind(conversation_id)
            .fetch_optional(pg)
            .await?;
    Ok(row
        .and_then(|(v,)| serde_json::from_value::<Vec<Item>>(v).ok())
        .unwrap_or_default())
}

/// The plan as the wire carries it, for a chip and for the surface.
pub fn items_json(items: &[Item]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|i| serde_json::json!({ "text": i.text, "status": i.status.as_str() }))
            .collect(),
    )
}

/// One line for a model reading its own plan back, and for an audit entry.
/// `2/5 done · now: draft the tickets`
pub fn summary(items: &[Item]) -> String {
    if items.is_empty() {
        return "no steps".into();
    }
    let done = items.iter().filter(|i| i.status == Status::Done).count();
    let now = items.iter().find(|i| i.status == Status::Active);
    let head = format!("{done}/{} done", items.len());
    match now {
        Some(i) => format!("{head} · now: {}", i.text),
        None if done == items.len() => format!("{head} · all steps complete"),
        None => head,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: Value) -> Result<Vec<Item>, String> {
        items_from(v.as_object().expect("object"))
    }

    #[test]
    fn reads_a_plan_with_its_statuses() {
        let got = parse(json!({ "items": [
            { "text": "read the spec", "status": "done" },
            { "text": "draft the tickets", "status": "active" },
            { "text": "update the doc", "status": "pending" },
        ]}))
        .expect("three steps");
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].status, Status::Done);
        assert_eq!(got[1].status, Status::Active);
        assert_eq!(got[2].status, Status::Pending);
        assert_eq!(got[1].text, "draft the tickets");
    }

    #[test]
    fn an_unstamped_step_is_pending() {
        // Writing a fresh plan should not require stamping every line.
        let got = parse(json!({ "items": [{ "text": "first" }, { "text": "second" }] }))
            .expect("defaults");
        assert!(got.iter().all(|i| i.status == Status::Pending));
    }

    #[test]
    fn an_empty_plan_is_legal_and_clears_the_strip() {
        // The way an agent says "I am done planning" — and the only way to
        // clear a plan at all, since every call replaces.
        assert_eq!(parse(json!({ "items": [] })).expect("empty"), vec![]);
    }

    #[test]
    fn two_active_steps_are_refused_with_the_rule() {
        let err = parse(json!({ "items": [
            { "text": "a", "status": "active" },
            { "text": "b", "status": "active" },
        ]}))
        .expect_err("two active");
        assert!(err.contains("2 items are active"), "{err}");
        assert!(err.contains("at most one"), "{err}");
    }

    #[test]
    fn no_active_step_is_fine() {
        // A plan can be freshly written, or finished.
        assert!(parse(json!({ "items": [{ "text": "a" }] })).is_ok());
        assert!(parse(json!({ "items": [{ "text": "a", "status": "done" }] })).is_ok());
    }

    #[test]
    fn a_blank_step_is_refused_by_position() {
        for blank in [json!(""), json!("   "), json!(null), json!(7)] {
            let err = parse(json!({ "items": [{ "text": "ok" }, { "text": blank }] }))
                .expect_err("blank");
            assert!(err.contains("item 2"), "{err} for {blank}");
        }
    }

    #[test]
    fn an_unknown_status_names_the_three_that_exist() {
        let err = parse(json!({ "items": [{ "text": "a", "status": "in_progress" }] }))
            .expect_err("bad status");
        assert!(err.contains("in_progress"), "{err}");
        assert!(err.contains("pending, active, done"), "{err}");
    }

    #[test]
    fn the_caps_hold() {
        let many: Vec<Value> = (0..=MAX_ITEMS)
            .map(|i| json!({ "text": format!("s{i}") }))
            .collect();
        assert!(parse(json!({ "items": many })).is_err());
        let long = "x".repeat(MAX_TEXT + 1);
        let err = parse(json!({ "items": [{ "text": long }] })).expect_err("too long");
        assert!(err.contains("one line"), "{err}");
        // A multi-byte step is measured in CHARACTERS, not bytes: an em-dash
        // plan is not half the length of an ASCII one.
        let wide = "é".repeat(MAX_TEXT);
        assert!(parse(json!({ "items": [{ "text": wide }] })).is_ok());
    }

    #[test]
    fn text_is_trimmed_so_the_strip_lines_up() {
        let got = parse(json!({ "items": [{ "text": "  padded  " }] })).expect("ok");
        assert_eq!(got[0].text, "padded");
    }

    #[test]
    fn a_non_array_and_a_missing_items_both_refuse() {
        assert!(parse(json!({})).is_err());
        assert!(parse(json!({ "items": { "text": "a" } })).is_err());
        assert!(parse(json!({ "items": ["a string"] })).is_err());
    }

    #[test]
    fn the_summary_says_where_the_plan_is() {
        let items = vec![
            Item {
                text: "read".into(),
                status: Status::Done,
            },
            Item {
                text: "draft".into(),
                status: Status::Active,
            },
            Item {
                text: "ship".into(),
                status: Status::Pending,
            },
        ];
        assert_eq!(summary(&items), "1/3 done · now: draft");
        let all_done = vec![Item {
            text: "read".into(),
            status: Status::Done,
        }];
        assert_eq!(summary(&all_done), "1/1 done · all steps complete");
        assert_eq!(summary(&[]), "no steps");
        // Written but not started: the count, and no claim about what is next.
        let fresh = vec![Item {
            text: "read".into(),
            status: Status::Pending,
        }];
        assert_eq!(summary(&fresh), "0/1 done");
    }

    #[test]
    fn items_round_trip_through_the_wire_shape() {
        // The stored column and the chip both carry this, so a status that
        // serialized as "Pending" would render wrong AND fail to load back.
        let items = vec![Item {
            text: "a".into(),
            status: Status::Active,
        }];
        let wire = items_json(&items);
        assert_eq!(wire[0]["status"], "active");
        let back: Vec<Item> =
            serde_json::from_value(serde_json::to_value(&items).unwrap()).expect("round trip");
        assert_eq!(back, items);
    }
}
