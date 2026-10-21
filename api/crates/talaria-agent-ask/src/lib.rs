// ASKING THE PERSON — a question with real options, in the stream, when the
// agent genuinely cannot tell what was meant.
//
// WHY THIS EXISTS. An agent that did not know which of two things you wanted
// had exactly two moves: guess, or write a paragraph back and hope you noticed.
// Guessing is how an agent confidently works the wrong ticket; a paragraph is
// how a person ends up re-reading their own request to work out what the agent
// is unsure about. Neither is asking. This is asking: one question, a short
// list of answers, one click.
//
// THERE IS NO TABLE, AND THAT IS THE DESIGN. The chip's option buttons send
// the chosen answer through the composer's own send path, so the answer lands
// as an ORDINARY MESSAGE in the transcript — which is the one place the agent
// actually reads, and the only place a reader would look to find out what was
// decided. A row recording the answer separately would be a second source of
// truth about what was said, and the two would eventually disagree. "Still
// open" is likewise derived rather than stored: a question with a user message
// after it has been answered, whatever words the person used to do it.
//
// WHEN AN AUDITABLE ANSWER IS ACTUALLY NEEDED — one that steers durable work
// and must refuse a stale or unauthorised reply — the precedent is research's
// parked run (`runs::decide`), which files the question with the approvals
// machinery and 409s a late answer. That is a heavier contract for a heavier
// job; a clarifying question in a chat is not that job.
//
// TWO OPTIONS MINIMUM, deliberately. One option is not a question — it is a
// statement with a button under it, and the person would click the only thing
// available. An OPEN question needs no tool at all: the agent can simply ask
// it in its own words, and the composer is always right there. So this tool
// exists for exactly the case where there are competing concrete answers.

use serde_json::{Map, Value, json};
use talaria_body::{array_msg, object_msg, zod_type_name};

/// Fewer than two and it is not a choice; more than six and the buttons stop
/// being scannable — an agent listing seven answers is enumerating rather
/// than deciding what it needs to know.
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 6;
/// A question, not an essay. The agent's own message is where context belongs.
pub const MAX_QUESTION: usize = 400;
/// Button text.
pub const MAX_LABEL: usize = 80;
pub const MAX_DETAIL: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub question: String,
    pub detail: Option<String>,
    pub options: Vec<Choice>,
}

fn text(
    obj: &Map<String, Value>,
    key: &str,
    max: usize,
    required: bool,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => {
            if required {
                Err(format!("{key} is required"))
            } else {
                Ok(None)
            }
        }
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                return if required {
                    Err(format!("{key} cannot be blank"))
                } else {
                    Ok(None)
                };
            }
            if t.chars().count() > max {
                return Err(format!("{key} is longer than {max} characters"));
            }
            Ok(Some(t.to_string()))
        }
        Some(other) => Err(format!(
            "{key} must be a string, got {}",
            zod_type_name(other)
        )),
    }
}

/// Read the wire shape. Shared with the fitness sandbox so a candidate model
/// is graded against the refusals production actually makes.
pub fn question_from(obj: &Map<String, Value>) -> Result<Question, String> {
    let question = text(obj, "question", MAX_QUESTION, true)?.expect("required");
    let detail = text(obj, "detail", MAX_QUESTION, false)?;
    let raw = obj.get("options").ok_or_else(|| array_msg("undefined"))?;
    let arr = raw
        .as_array()
        .ok_or_else(|| array_msg(zod_type_name(raw)))?;
    if arr.len() < MIN_OPTIONS {
        return Err(format!(
            "give at least {MIN_OPTIONS} options — with one answer there is nothing to choose. \
             If the question is open, just ask it in your message instead"
        ));
    }
    if arr.len() > MAX_OPTIONS {
        return Err(format!(
            "{} options is too many (max {MAX_OPTIONS}) — ask the one thing that is actually \
             blocking you",
            arr.len()
        ));
    }
    let mut options = Vec::with_capacity(arr.len());
    for (i, el) in arr.iter().enumerate() {
        // A bare string is the common shape and worth accepting: an option
        // with no detail is just its label, and making a model wrap every one
        // in an object is friction for nothing.
        let choice = match el {
            Value::String(s) => {
                let t = s.trim();
                if t.is_empty() {
                    return Err(format!("option {} cannot be blank", i + 1));
                }
                if t.chars().count() > MAX_LABEL {
                    return Err(format!(
                        "option {}: label is longer than {MAX_LABEL} characters — it is a button",
                        i + 1
                    ));
                }
                Choice {
                    label: t.to_string(),
                    detail: None,
                }
            }
            Value::Object(o) => Choice {
                label: text(o, "label", MAX_LABEL, true)
                    .map_err(|e| format!("option {}: {e}", i + 1))?
                    .expect("required"),
                detail: text(o, "detail", MAX_DETAIL, false)
                    .map_err(|e| format!("option {}: {e}", i + 1))?,
            },
            other => {
                return Err(format!(
                    "option {}: {}",
                    i + 1,
                    object_msg(zod_type_name(other))
                ));
            }
        };
        options.push(choice);
    }
    // TWO BUTTONS WITH THE SAME WORDS ON THEM cannot be told apart by the
    // person clicking or by the agent reading the reply, since the answer
    // travels as the label's text.
    for i in 0..options.len() {
        if let Some(j) = options[..i]
            .iter()
            .position(|o| o.label.eq_ignore_ascii_case(&options[i].label))
        {
            return Err(format!(
                "options {} and {} say the same thing (\"{}\") — the answer is sent as the \
                 option's own words, so two that read alike cannot be told apart",
                j + 1,
                i + 1,
                options[i].label
            ));
        }
    }
    Ok(Question {
        question,
        detail,
        options,
    })
}

/// The chip the stream renders. `id` is stable per question text so a second
/// identical ask inside one turn replaces rather than stacks; a DIFFERENT
/// question is a different chip, because both are still open.
pub fn chip(q: &Question) -> Value {
    json!({
        "id": format!("ask:{}", short_id(&q.question)),
        "kind": "question",
        "label": q.question,
        "description": q.detail,
        "options": q.options.iter().map(|o| json!({
            "label": o.label,
            "detail": o.detail,
        })).collect::<Vec<_>>(),
    })
}

/// A short, stable key from the question's own words — enough to tell two
/// open questions apart in one turn without carrying the whole sentence as an
/// id. Not a hash with collision guarantees; it does not need to be.
fn short_id(question: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in question.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:x}")
}

/// What the agent is told when the question has landed. It says the turn is
/// over, because the single thing an agent must not do after asking is carry
/// on and act on the guess it just admitted it could not make.
pub fn asked_sentence(q: &Question) -> String {
    format!(
        "Asked: \"{}\" with {} options. Their answer will arrive as your next message — \
         end your turn now and wait for it rather than acting on a guess.",
        q.question,
        q.options.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: Value) -> Result<Question, String> {
        question_from(v.as_object().expect("object"))
    }

    #[test]
    fn reads_a_question_with_plain_string_options() {
        let q = parse(json!({
            "question": "Which board should these tickets land on?",
            "options": ["Platform", "Growth"],
        }))
        .expect("two options");
        assert_eq!(q.question, "Which board should these tickets land on?");
        assert_eq!(q.options.len(), 2);
        assert_eq!(q.options[0].label, "Platform");
        assert_eq!(q.options[0].detail, None);
        assert_eq!(q.detail, None);
    }

    #[test]
    fn reads_options_with_their_own_detail() {
        let q = parse(json!({
            "question": "How deep should I go?",
            "detail": "The spec is long and some of it looks stale.",
            "options": [
                { "label": "Just the retry section", "detail": "Fastest; skips the rest" },
                { "label": "The whole spec", "detail": "Slower, catches stale parts" },
            ],
        }))
        .expect("objects");
        assert_eq!(
            q.detail.as_deref(),
            Some("The spec is long and some of it looks stale.")
        );
        assert_eq!(
            q.options[1].detail.as_deref(),
            Some("Slower, catches stale parts")
        );
    }

    #[test]
    fn one_option_is_not_a_question() {
        let err = parse(json!({ "question": "ok?", "options": ["Yes"] })).expect_err("one");
        assert!(err.contains("at least 2"), "{err}");
        // And the message points at the move that IS right for an open
        // question, which is to not use the tool.
        assert!(err.contains("just ask it in your message"), "{err}");
        let none = parse(json!({ "question": "ok?", "options": [] })).expect_err("none");
        assert!(none.contains("at least 2"), "{none}");
    }

    #[test]
    fn too_many_options_says_to_ask_the_blocking_thing() {
        let many: Vec<Value> = (0..=MAX_OPTIONS)
            .map(|i| json!(format!("option {i}")))
            .collect();
        let err = parse(json!({ "question": "which?", "options": many })).expect_err("too many");
        assert!(err.contains("too many"), "{err}");
        assert!(err.contains("blocking you"), "{err}");
    }

    #[test]
    fn two_options_that_read_alike_are_refused() {
        // The answer travels as the option's own words, so duplicates are
        // unanswerable — by the person clicking and by the agent reading.
        let err = parse(json!({
            "question": "which?",
            "options": ["Platform", "platform"],
        }))
        .expect_err("dupes");
        assert!(err.contains("options 1 and 2"), "{err}");
        assert!(err.contains("same thing"), "{err}");
    }

    #[test]
    fn a_missing_or_blank_question_is_refused() {
        assert!(parse(json!({ "options": ["a", "b"] })).is_err());
        assert!(parse(json!({ "question": "   ", "options": ["a", "b"] })).is_err());
        assert!(parse(json!({ "question": 7, "options": ["a", "b"] })).is_err());
    }

    #[test]
    fn a_blank_or_oversized_option_is_refused_by_position() {
        let blank = parse(json!({ "question": "q?", "options": ["ok", "  "] })).expect_err("blank");
        assert!(blank.contains("option 2"), "{blank}");
        let long = "x".repeat(MAX_LABEL + 1);
        let big = parse(json!({ "question": "q?", "options": ["ok", long] })).expect_err("long");
        assert!(big.contains("option 2"), "{big}");
        assert!(big.contains("it is a button"), "{big}");
    }

    #[test]
    fn the_options_member_must_be_an_array_of_strings_or_objects() {
        assert!(parse(json!({ "question": "q?", "options": "Platform" })).is_err());
        let err = parse(json!({ "question": "q?", "options": ["ok", 7] })).expect_err("number");
        assert!(err.contains("option 2"), "{err}");
    }

    #[test]
    fn the_chip_carries_the_question_and_its_options() {
        let q = parse(json!({
            "question": "Which board?",
            "options": [{ "label": "Platform", "detail": "the infra board" }, "Growth"],
        }))
        .expect("ok");
        let c = chip(&q);
        assert_eq!(c["kind"], "question");
        assert_eq!(c["label"], "Which board?");
        assert_eq!(c["options"][0]["label"], "Platform");
        assert_eq!(c["options"][0]["detail"], "the infra board");
        assert!(c["options"][1]["detail"].is_null());
        assert!(
            c["id"].as_str().expect("id").starts_with("ask:"),
            "{}",
            c["id"]
        );
    }

    #[test]
    fn the_chip_id_is_stable_per_question_and_differs_across_questions() {
        // Asking the SAME thing twice in one turn should replace, not stack.
        // Two different open questions are two chips, because both are open.
        let a = parse(json!({ "question": "Which board?", "options": ["x", "y"] })).unwrap();
        let b = parse(json!({ "question": "Which board?", "options": ["p", "q"] })).unwrap();
        let c = parse(json!({ "question": "Which agent?", "options": ["x", "y"] })).unwrap();
        assert_eq!(chip(&a)["id"], chip(&b)["id"]);
        assert_ne!(chip(&a)["id"], chip(&c)["id"]);
    }

    #[test]
    fn the_sentence_tells_the_agent_to_stop() {
        // The one thing an agent must not do after asking is carry on acting
        // on the guess it just said it could not make.
        let q = parse(json!({ "question": "Which board?", "options": ["x", "y"] })).unwrap();
        let s = asked_sentence(&q);
        assert!(s.contains("Which board?"), "{s}");
        assert!(s.contains("2 options"), "{s}");
        assert!(s.contains("end your turn"), "{s}");
    }

    #[test]
    fn question_text_is_trimmed_and_capped() {
        let q = parse(json!({ "question": "  Which board?  ", "options": ["a", "b"] })).unwrap();
        assert_eq!(q.question, "Which board?");
        let long = "x".repeat(MAX_QUESTION + 1);
        assert!(parse(json!({ "question": long, "options": ["a", "b"] })).is_err());
    }
}
