// THE DEF-SIDE SCHEMA — the small declarative shape a Rust harness
// definition validates a model's reply against. It is the zod half of a TS
// def's `output.schema`, ported as data: zod's ONE declaration serves both
// validation and the wire schema, and this type keeps that dual role (the
// wire renderer reads the same value — json-schema's port composes with it).
//
// WHAT IT REPRODUCES, AND FROM WHERE. zod's safeParse reports a list of
// issues, and harness/json.ts turns those issues into the sentences a model
// reads back on its repair turn: "missing required field 'verdict'",
// "field 'issues[1]' should be string, got number", "field 'verdict' must
// be one of \"pass\" | \"revise\"". Those sentences are behavioral — a model
// acts on them — so the grammar here is not inspired by zod, it is PROBED
// from it: every code, `expected` word, path shape, and message below was
// printed by zod 4.3.6 itself (bun probe, this repo's ui dependency) and is
// pinned in json.rs's tests. Notably, from the probes:
//
//   · a MISSING key reports invalid_type with `received undefined` — json.ts
//     converts it to "missing required field" by asking the INPUT whether the
//     key was absent (absent and wrong-typed are different instructions to a
//     model);
//   · `.optional()`/`.nullable()` are WRAPPERS, not unions — a wrong-typed
//     value inside `z.string().nullable().optional()` reports
//     `expected string`, not a union failure;
//   · an ENUM reports invalid_value for every non-member, INCLUDING wrong
//     types (5, null, true all answer `must be one of ...`);
//   · a RECORD's root mismatch says `record`, not `object`;
//   · a z.union that fails every branch reports one issue whose message is
//     exactly "Invalid input" — the one place zod's own prose reaches the
//     model, through json.ts's default branch;
//   · issues arrive in DECLARATION order (probe: {a,b,c} all missing →
//     paths [a],[b],[c]), and every issue is collected — zod does not abort
//     on the first.
//
// THE VALIDATOR RETURNS THE ZOD-SHAPED OUTPUT, not the input echoed: unknown
// keys strip, `.trim()` runs before the bounds are judged and lands in the
// output, `.default()` fills an absent key. zod's safeParse does all three,
// and the defs read the result (a default `issues: []` is a real empty list
// to the judge, not a missing key).
//
// LENGTH IS UTF-16 CODE UNITS, because zod's is: `z.string().max(40)` counts
// JS `.length`, an emoji is 2, and `<=40 characters` in the message means
// those units. Rust `chars().count()` would call it 1 and pass a string zod
// fails — a fidelity bug neither side could see without an astral-plane tag.
//
// NOT HERE (deliberately): `.transform()` — it runs AFTER validation, so in
// the port it is the def's own code mapping over the validated Value, and
// the wire schema's `io: 'input'` rule (render the side the MODEL emits) is
// exactly the statement that transforms do not belong in the contract. Also
// not here: `.strict()` (no def uses it — unrecognized_keys never fires),
// `.preprocess()` (one site, research's envelope unwrap, which ports with
// research's def as a pre-step over the candidate).

use serde_json::Value;

/// One segment of an issue's path: an object key or an array index.
#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Key(String),
    Idx(usize),
}

/// One validation issue, carrying exactly what harness/json.ts's
/// `describeIssue` needs to sentence it. `expected` matches zod's own word.
#[derive(Debug, Clone, PartialEq)]
pub enum Issue {
    /// The key was absent where the schema demanded a value.
    Missing,
    /// `invalid_type` — wrong JSON type, or a null where the inner schema
    /// (nullable not declared) wanted a value.
    InvalidType { expected: &'static str },
    /// `invalid_value` — an enum non-member; `values` are the members,
    /// pre-JSON-stringified, exactly as zod prints them.
    InvalidValue { values: Vec<String> },
    /// `too_small` on a string's UTF-16 length.
    TooSmall(u64),
    /// `too_big` on a string's UTF-16 length.
    TooBig(u64),
    /// `too_small` on a number's bound — the sentence says "number", not
    /// "string", so it cannot ride `TooSmall`.
    NumTooSmall(f64),
    /// `too_big` on a number's bound.
    NumTooBig(f64),
    /// `too_big` on an array's length.
    ArrayTooBig(u64),
    /// The default branch — zod's own message, passed through verbatim (the
    /// union failure's "Invalid input" arrives here).
    Message(String),
}

/// A field of an object schema, in declaration order — the order zod
/// reports issues in.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: &'static str,
    pub schema: Schema,
}

impl Field {
    pub fn required(name: &'static str, schema: Schema) -> Field {
        Field { name, schema }
    }
}

/// The schema algebra, closed over what the ten JSON harness defs declare.
#[derive(Debug, Clone)]
pub enum Schema {
    /// `z.unknown()` — anything, passed through.
    Unknown,
    /// `z.string()`, optionally trimmed and bounded by UTF-16 length.
    Str {
        trim: bool,
        min: Option<u64>,
        max: Option<u64>,
    },
    /// `z.number()`.
    Num,
    /// `z.number().min(min).max(max)` — the bounded number, parallel to
    /// `Num` (a separate variant rather than fields on it: every existing
    /// `Schema::Num` site is an unbounded number and stays untouched).
    BoundedNum { min: f64, max: f64 },
    /// `z.boolean()`.
    Bool,
    /// `z.enum([...])`.
    Enum(Vec<String>),
    /// `z.array(...)`.
    Array(Box<Schema>),
    /// `z.array(...).max(max)` — the length-capped array, parallel to
    /// `Array` for the same reason `BoundedNum` is parallel to `Num`.
    ArrayMax(Box<Schema>, u64),
    /// `z.string().datetime()` — ISO 8601 in zod's exact dialect: `T`
    /// separator, UTC `Z` suffix only (no offsets), calendar-valid. Every
    /// other spelling — date-only, a space for the `T`, a `+01:00` offset,
    /// a missing `Z` — is the same one issue.
    DateTime,
    /// `z.object({...})` — closed on read (unknown keys strip), like zod's
    /// default object.
    Object(Vec<Field>),
    /// `z.record(z.string(), ...)` — an open string-keyed map. Note zod's
    /// root-mismatch word for this shape is `record`, not `object`.
    Record(Box<Schema>),
    /// `z.union([...])` — first branch that validates cleanly wins; every
    /// branch failing is the one "Invalid input" issue.
    Union(Vec<Schema>),
    /// `.nullable()` — null passes; anything else validates against the inner
    /// schema.
    Nullable(Box<Schema>),
    /// `.optional()` — an absent key passes (and stays absent in the
    /// output); a present value validates against the inner schema.
    Optional(Box<Schema>),
    /// `.default(v)` — an absent key passes and the output carries `v`; a
    /// present value validates against the inner schema.
    Defaulted(Box<Schema>, Value),
}

impl Schema {
    pub fn string() -> Schema {
        Schema::Str {
            trim: false,
            min: None,
            max: None,
        }
    }

    /// `z.string().trim().min(min).max(max)` — the bounds judge the TRIMMED
    /// value, as zod's overwrite does.
    pub fn trimmed_string(min: u64, max: u64) -> Schema {
        Schema::Str {
            trim: true,
            min: Some(min),
            max: Some(max),
        }
    }

    pub fn optional(schema: Schema) -> Schema {
        Schema::Optional(Box::new(schema))
    }

    pub fn nullable(schema: Schema) -> Schema {
        Schema::Nullable(Box::new(schema))
    }

    pub fn with_default(schema: Schema, value: Value) -> Schema {
        Schema::Defaulted(Box::new(schema), value)
    }
}

/// Validate `value` against `schema`, collecting EVERY issue in declaration
/// order, and return the zod-shaped output (stripped, trimmed, defaulted).
/// An empty issue list means the value held the contract.
pub fn validate(schema: &Schema, value: &Value) -> (Value, Vec<(Vec<Seg>, Issue)>) {
    let mut issues = Vec::new();
    let out = walk(schema, value, &mut Vec::new(), &mut issues);
    (out, issues)
}

/// The walk itself. `path` is borrowed and extended/retracted as the
/// recursion descends, so each issue owns a copy of its full path.
fn walk(
    schema: &Schema,
    value: &Value,
    path: &mut Vec<Seg>,
    issues: &mut Vec<(Vec<Seg>, Issue)>,
) -> Value {
    match schema {
        Schema::Unknown => value.clone(),
        Schema::Nullable(inner) => {
            if value.is_null() {
                Value::Null
            } else {
                walk(inner, value, path, issues)
            }
        }
        // At a VALUE position optional/defaulted behave as the inner schema;
        // the absent-key half of their contract is decided by the enclosing
        // object's field loop.
        Schema::Optional(inner) | Schema::Defaulted(inner, _) => walk(inner, value, path, issues),
        Schema::Str { trim, min, max } => match value.as_str() {
            None => {
                issues.push((path.clone(), Issue::InvalidType { expected: "string" }));
                value.clone()
            }
            Some(raw) => {
                let s = if *trim { raw.trim() } else { raw };
                // zod counts JS string length: UTF-16 code units.
                let units = s.encode_utf16().count() as u64;
                if let Some(lo) = min.filter(|lo| units < *lo) {
                    issues.push((path.clone(), Issue::TooSmall(lo)));
                }
                if let Some(hi) = max.filter(|hi| units > *hi) {
                    issues.push((path.clone(), Issue::TooBig(hi)));
                }
                Value::String(s.to_string())
            }
        },
        Schema::DateTime => match value.as_str() {
            None => {
                issues.push((path.clone(), Issue::InvalidType { expected: "string" }));
                value.clone()
            }
            Some(s) => {
                if is_zod_datetime(s) {
                    value.clone()
                } else {
                    // zod's datetime issue is invalid_format carrying its own
                    // fixed message, so it renders through the Message arm
                    // verbatim rather than through a code-specific sentence.
                    issues.push((path.clone(), Issue::Message("Invalid ISO datetime".into())));
                    value.clone()
                }
            }
        },
        Schema::Num => {
            if value.is_number() {
                value.clone()
            } else {
                issues.push((path.clone(), Issue::InvalidType { expected: "number" }));
                value.clone()
            }
        }
        Schema::BoundedNum { min, max } => match value.as_f64() {
            None => {
                issues.push((path.clone(), Issue::InvalidType { expected: "number" }));
                value.clone()
            }
            Some(n) => {
                if n < *min {
                    issues.push((path.clone(), Issue::NumTooSmall(*min)));
                }
                if n > *max {
                    issues.push((path.clone(), Issue::NumTooBig(*max)));
                }
                value.clone()
            }
        },
        Schema::Bool => {
            if value.is_boolean() {
                value.clone()
            } else {
                issues.push((
                    path.clone(),
                    Issue::InvalidType {
                        expected: "boolean",
                    },
                ));
                value.clone()
            }
        }
        Schema::Enum(members) => {
            let ok = value
                .as_str()
                .is_some_and(|s| members.iter().any(|m| m == s));
            if ok {
                value.clone()
            } else {
                issues.push((
                    path.clone(),
                    Issue::InvalidValue {
                        values: members
                            .iter()
                            .map(|m| serde_json::to_string(m).unwrap_or_default())
                            .collect(),
                    },
                ));
                value.clone()
            }
        }
        Schema::Array(item) => match value.as_array() {
            None => {
                issues.push((path.clone(), Issue::InvalidType { expected: "array" }));
                value.clone()
            }
            Some(items) => Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item_value)| {
                        path.push(Seg::Idx(i));
                        let out = walk(item, item_value, path, issues);
                        path.pop();
                        out
                    })
                    .collect(),
            ),
        },
        Schema::ArrayMax(item, max) => match value.as_array() {
            None => {
                issues.push((path.clone(), Issue::InvalidType { expected: "array" }));
                value.clone()
            }
            Some(items) => {
                if items.len() as u64 > *max {
                    issues.push((path.clone(), Issue::ArrayTooBig(*max)));
                }
                Value::Array(
                    items
                        .iter()
                        .enumerate()
                        .map(|(i, item_value)| {
                            path.push(Seg::Idx(i));
                            let out = walk(item, item_value, path, issues);
                            path.pop();
                            out
                        })
                        .collect(),
                )
            }
        },
        Schema::Object(fields) => match value.as_object() {
            None => {
                issues.push((path.clone(), Issue::InvalidType { expected: "object" }));
                value.clone()
            }
            Some(map) => {
                let mut out = serde_json::Map::new();
                for field in fields {
                    match map.get(field.name) {
                        None => match &field.schema {
                            Schema::Optional(_) => {} // absent stays absent
                            Schema::Defaulted(_, d) => {
                                out.insert(field.name.to_string(), d.clone());
                            }
                            _ => {
                                path.push(Seg::Key(field.name.to_string()));
                                issues.push((path.clone(), Issue::Missing));
                                path.pop();
                            }
                        },
                        Some(v) => {
                            path.push(Seg::Key(field.name.to_string()));
                            let cleaned = walk(&field.schema, v, path, issues);
                            path.pop();
                            out.insert(field.name.to_string(), cleaned);
                        }
                    }
                }
                // Unknown keys strip — zod's default object, and the probes
                // confirm nothing reports them.
                Value::Object(out)
            }
        },
        Schema::Record(value_schema) => match value.as_object() {
            None => {
                issues.push((path.clone(), Issue::InvalidType { expected: "record" }));
                value.clone()
            }
            Some(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    path.push(Seg::Key(k.clone()));
                    let cleaned = walk(value_schema, v, path, issues);
                    path.pop();
                    out.insert(k.clone(), cleaned);
                }
                Value::Object(out)
            }
        },
        Schema::Union(branches) => {
            // First clean branch wins; all-fail is the ONE "Invalid input".
            for branch in branches {
                let mut probe_issues = Vec::new();
                let out = walk(branch, value, path, &mut probe_issues);
                if probe_issues.is_empty() {
                    return out;
                }
            }
            issues.push((path.clone(), Issue::Message("Invalid input".into())));
            value.clone()
        }
    }
}

// ── The sentences (json.ts's describeIssue, over this module's issues) ──────

/// zod 4.3.6's datetime grammar, probed rather than read from a spec:
/// `YYYY-MM-DDTHH:MM:SS[.f+]Z`, separators uppercase, UTC only (a `+01:00`
/// offset is invalid), the date CALENDAR-valid (Feb 30 and hour 24 fail),
/// and the fraction, when present, at least one digit with no upper cap.
/// Years 0000 and 9999 pass. Every deviation is the one `invalid_format`
/// issue upstream.
pub fn is_zod_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    let dig = |at: usize| b.get(at).is_some_and(|c| c.is_ascii_digit());
    let run = |from: usize, len: usize| (0..len).all(|k| dig(from + k));
    let num =
        |from: usize, len: usize| (0..len).fold(0i64, |a, k| a * 10 + (b[from + k] - b'0') as i64);
    // Spine: YYYY-MM-DDTHH:MM:SS, then a fraction and/or Z.
    if b.len() < 20
        || !run(0, 4)
        || b.get(4) != Some(&b'-')
        || !run(5, 2)
        || b.get(7) != Some(&b'-')
        || !run(8, 2)
        || b.get(10) != Some(&b'T')
        || !run(11, 2)
        || b.get(13) != Some(&b':')
        || !run(14, 2)
        || b.get(16) != Some(&b':')
        || !run(17, 2)
    {
        return false;
    }
    let (y, mo, d) = (num(0, 4), num(5, 2), num(8, 2));
    let (h, mi, se) = (num(11, 2), num(14, 2), num(17, 2));
    if !(1..=12).contains(&mo) || h > 23 || mi > 59 || se > 59 {
        return false;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let dim = match mo {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if d < 1 || d > dim {
        return false;
    }
    // The tail: 'Z' alone, or '.' plus one-or-more digits then 'Z'. Byte 19
    // sits on a char boundary (17..19 are ASCII digits), so the slice is
    // safe; non-ASCII tails fail the digit/Z tests.
    let tail = &s[19..];
    match tail.strip_prefix('.') {
        Some(frac) => {
            let digits = frac.strip_suffix('Z').unwrap_or("");
            !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit())
        }
        None => tail == "Z",
    }
}

/// A numeric bound rendered the way zod prints it and the wire schema wants
/// it: whole numbers without the `.0` (`>=0`, not `>=0.0`).
pub fn num_label(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 9.2e18 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// `issues[0]`, `plan.steps[2].title` — the path a human (or a model) can
/// read back against its own output.
pub fn path_label(path: &[Seg]) -> String {
    let mut acc = String::new();
    for seg in path {
        match seg {
            Seg::Idx(i) => acc.push_str(&format!("[{i}]")),
            Seg::Key(k) => {
                if acc.is_empty() {
                    acc.push_str(k);
                } else {
                    acc.push('.');
                    acc.push_str(k);
                }
            }
        }
    }
    acc
}

/// zod's `typeof` name for a JSON value — the `got` word in a repair
/// sentence. Parsed JSON never yields undefined; 'nothing' is kept for the
/// value-at-a-missing-path case, which TS reaches through `valueAt`.
pub fn type_name(v: Option<&Value>) -> &'static str {
    match v {
        None | Some(Value::Null) => match v {
            None => "nothing",
            Some(_) => "null",
        },
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

/// Walk an issue's path against the parsed root — what the model actually
/// wrote at that spot, for the `got` clause.
pub fn value_at<'a>(root: &'a Value, path: &[Seg]) -> Option<&'a Value> {
    let mut cur = root;
    for seg in path {
        match seg {
            Seg::Key(k) => cur = cur.get(k)?,
            Seg::Idx(i) => cur = cur.get(*i)?,
        }
    }
    Some(cur)
}

/// One issue as the sentence a model reads. Every branch mirrors a probed
/// zod issue rendered by harness/json.ts's describeIssue — see the module
/// header for which probe pinned which word.
pub fn describe_issue(issue: &(Vec<Seg>, Issue), root: &Value) -> String {
    let (path, kind) = issue;
    let label = path_label(path);
    match kind {
        Issue::Missing => format!("missing required field '{label}'"),
        Issue::InvalidType { expected } => {
            let got = type_name(value_at(root, path).or(Some(&Value::Null)));
            if label.is_empty() {
                format!("expected {expected}, got {got}")
            } else {
                format!("field '{label}' should be {expected}, got {got}")
            }
        }
        Issue::InvalidValue { values } => {
            let allowed = values.join(" | ");
            let got = value_at(root, path)
                .and_then(|v| serde_json::to_string(v).ok())
                .unwrap_or_else(|| "null".into());
            if label.is_empty() {
                format!("value must be one of {allowed}")
            } else {
                format!("field '{label}' must be one of {allowed} (got {got})")
            }
        }
        Issue::TooSmall(n) => {
            let msg = format!("Too small: expected string to have >={n} characters");
            if label.is_empty() {
                msg
            } else {
                format!("field '{label}': {msg}")
            }
        }
        Issue::TooBig(n) => {
            let msg = format!("Too big: expected string to have <={n} characters");
            if label.is_empty() {
                msg
            } else {
                format!("field '{label}': {msg}")
            }
        }
        Issue::NumTooSmall(n) => {
            let msg = format!("Too small: expected number to be >={}", num_label(*n));
            if label.is_empty() {
                msg
            } else {
                format!("field '{label}': {msg}")
            }
        }
        Issue::NumTooBig(n) => {
            let msg = format!("Too big: expected number to be <={}", num_label(*n));
            if label.is_empty() {
                msg
            } else {
                format!("field '{label}': {msg}")
            }
        }
        Issue::ArrayTooBig(n) => {
            let msg = format!("Too big: expected array to have <={n} items");
            if label.is_empty() {
                msg
            } else {
                format!("field '{label}': {msg}")
            }
        }
        Issue::Message(m) => {
            if label.is_empty() {
                m.clone()
            } else {
                format!("field '{label}': {m}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Every assertion below is a zod 4.3.6 probe output, not an intention.
    // The probes ran against the exact def shapes (judge's verdict object,
    // the briefer's nullable-optional id, research's trimmed query, the
    // blurb writer's record).

    fn verdict_schema() -> Schema {
        Schema::Object(vec![
            Field::required(
                "verdict",
                Schema::Enum(
                    ["pass", "revise", "escalate"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                ),
            ),
            Field::required("summary", Schema::string()),
            Field::required(
                "issues",
                Schema::optional(Schema::Array(Box::new(Schema::Unknown))),
            ),
        ])
    }

    #[test]
    fn missing_key_reports_missing_with_the_field_name() {
        let schema = verdict_schema();
        let (_, issues) = validate(&schema, &json!({"summary": "ok"}));
        assert_eq!(issues.len(), 1);
        assert_eq!(
            describe_issue(&issues[0], &json!({"summary": "ok"})),
            "missing required field 'verdict'"
        );
    }

    #[test]
    fn issues_arrive_in_declaration_order_and_all_of_them() {
        let schema = Schema::Object(vec![
            Field::required("a", Schema::string()),
            Field::required("b", Schema::string()),
            Field::required("c", Schema::string()),
        ]);
        let (_, issues) = validate(&schema, &json!({"c": 1}));
        let labels: Vec<String> = issues.iter().map(|(p, _)| path_label(p)).collect();
        assert_eq!(labels, vec!["a", "b", "c"]);
    }

    #[test]
    fn wrong_type_names_field_and_both_types() {
        let schema = verdict_schema();
        let root = json!({"verdict": "pass", "summary": 42});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'summary' should be string, got number"
        );
    }

    #[test]
    fn enum_non_members_are_invalid_value_even_wrong_types() {
        let schema = verdict_schema();
        for bad in [json!("maybe"), json!(5), json!(null), json!(true)] {
            let root = json!({"verdict": bad, "summary": "s"});
            let (_, issues) = validate(&schema, &root);
            assert_eq!(issues.len(), 1, "{bad}");
            // `got` is the value JSON-stringified — quotes and all — because
            // the probe prints values through JSON.stringify.
            let got = serde_json::to_string(&bad).unwrap();
            assert_eq!(
                describe_issue(&issues[0], &root),
                format!(
                    "field 'verdict' must be one of \"pass\" | \"revise\" | \"escalate\" (got {got})"
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn nullable_optional_collapses_to_the_inner_expected_word() {
        let schema = Schema::Object(vec![
            Field::required("question", Schema::string()),
            Field::required("recommendation", Schema::string()),
            Field::required(
                "recommendedActionId",
                Schema::optional(Schema::nullable(Schema::string())),
            ),
        ]);
        let root = json!({"question": "q", "recommendation": "r", "recommendedActionId": 5});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'recommendedActionId' should be string, got number"
        );
        // And the two legal spellings pass: absent, and null.
        let ok1 = validate(&schema, &json!({"question": "q", "recommendation": "r"}));
        assert!(ok1.1.is_empty());
        let ok2 = validate(
            &schema,
            &json!({"question": "q", "recommendation": "r", "recommendedActionId": null}),
        );
        assert!(ok2.1.is_empty());
    }

    #[test]
    fn trimmed_string_bounds_judge_the_trimmed_value() {
        let schema = Schema::Object(vec![Field::required(
            "query",
            Schema::trimmed_string(1, 40),
        )]);
        let whitespace = json!({"query": "   "});
        let (_, issues) = validate(&schema, &whitespace);
        assert_eq!(
            describe_issue(&issues[0], &whitespace),
            "field 'query': Too small: expected string to have >=1 characters"
        );
        let long = json!({"query": "x".repeat(41)});
        let (_, issues) = validate(&schema, &long);
        assert_eq!(
            describe_issue(&issues[0], &long),
            "field 'query': Too big: expected string to have <=40 characters"
        );
    }

    #[test]
    fn length_counts_utf16_units_because_zods_does() {
        // One emoji = 2 UTF-16 units = JS .length 2. zod's max(3) fails a
        // two-emoji string; chars().count() would call it 2 and pass it.
        let schema = Schema::Object(vec![Field::required(
            "q",
            Schema::Str {
                trim: false,
                min: None,
                max: Some(3),
            },
        )]);
        let root = json!({"q": "🎯🎯"});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn array_elements_carry_their_index() {
        let schema = Schema::Object(vec![Field::required(
            "issues",
            Schema::Array(Box::new(Schema::string())),
        )]);
        let root = json!({"issues": ["a", 7]});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'issues[1]' should be string, got number"
        );
    }

    #[test]
    fn nested_paths_render_dotted_and_indexed() {
        let schema = Schema::Object(vec![Field::required(
            "plan",
            Schema::Object(vec![Field::required("title", Schema::string())]),
        )]);
        let root = json!({"plan": {}});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "missing required field 'plan.title'"
        );
        // A missing INTERMEDIATE object reports as its own missing field —
        // the probe shows invalid_type expected 'object' at ['plan'], which
        // isAbsent converts the same way.
        let (_, issues) = validate(&schema, &json!({}));
        assert_eq!(
            describe_issue(&issues[0], &json!({})),
            "missing required field 'plan'"
        );
    }

    #[test]
    fn record_says_record_and_keys_carry_their_name() {
        let schema = Schema::Record(Box::new(Schema::string()));
        let array_root = json!(["a"]);
        let (_, issues) = validate(&schema, &array_root);
        assert_eq!(
            describe_issue(&issues[0], &array_root),
            "expected record, got array"
        );
        let root = json!({"gpt-4": 5});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'gpt-4' should be string, got number"
        );
        // null root is a type failure too, not a pass.
        let (_, issues) = validate(&schema, &json!(null));
        assert!(matches!(
            issues[0].1,
            Issue::InvalidType { expected: "record" }
        ));
    }

    #[test]
    fn root_shape_mismatch_has_no_label() {
        let schema = verdict_schema();
        let root = json!([{"verdict": "pass", "summary": "ok"}]);
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "expected object, got array"
        );
        let root = json!("hello");
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "expected object, got string"
        );
    }

    #[test]
    fn union_failure_is_the_one_invalid_input() {
        let schema = Schema::Union(vec![
            Schema::string(),
            Schema::Array(Box::new(Schema::string())),
        ]);
        let root = json!(5);
        let (_, issues) = validate(&schema, &root);
        assert_eq!(issues.len(), 1);
        assert_eq!(describe_issue(&issues[0], &root), "Invalid input");
    }

    #[test]
    fn unknown_keys_strip_silently_and_defaults_fill() {
        let schema = Schema::Object(vec![
            Field::required(
                "verdict",
                Schema::Enum(
                    ["pass", "revise", "escalate"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                ),
            ),
            Field::required("summary", Schema::string()),
            Field::required(
                "issues",
                Schema::with_default(Schema::Array(Box::new(Schema::string())), json!([])),
            ),
        ]);
        let (out, issues) = validate(
            &schema,
            &json!({"verdict": "pass", "summary": "s", "surprise": 1}),
        );
        assert!(issues.is_empty());
        assert_eq!(
            out,
            json!({"verdict": "pass", "summary": "s", "issues": []})
        );
        assert!(out.get("surprise").is_none());
        // Absent optional stays absent (not null, not a key with undefined).
        let schema = Schema::Object(vec![
            Field::required("a", Schema::string()),
            Field::required("b", Schema::optional(Schema::string())),
        ]);
        let (out, issues) = validate(&schema, &json!({"a": "x"}));
        assert!(issues.is_empty());
        assert_eq!(out, json!({"a": "x"}));
    }

    #[test]
    fn trim_runs_into_the_output() {
        let schema = Schema::Object(vec![Field::required("q", Schema::trimmed_string(1, 40))]);
        let (out, issues) = validate(&schema, &json!({"q": "  padded  "}));
        assert!(issues.is_empty());
        assert_eq!(out, json!({"q": "padded"}));
    }

    // ── the three variants muse brought, each pinned to a zod 4.3.6 probe ────

    #[test]
    fn bounded_number_sentences_are_the_probed_ones() {
        let schema = Schema::Object(vec![Field::required(
            "estimatedHours",
            Schema::BoundedNum {
                min: 0.0,
                max: 999.0,
            },
        )]);
        let root = json!({"estimatedHours": -1});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'estimatedHours': Too small: expected number to be >=0"
        );
        let root = json!({"estimatedHours": 1000});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'estimatedHours': Too big: expected number to be <=999"
        );
        // The bounds are inclusive, and a non-number is a type failure with
        // zod's `expected number` word.
        for ok in [json!(0), json!(999), json!(8.5)] {
            let (_, issues) = validate(&schema, &json!({"estimatedHours": ok}));
            assert!(issues.is_empty(), "{ok}");
        }
        let root = json!({"estimatedHours": "5"});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'estimatedHours' should be number, got string"
        );
    }

    #[test]
    fn array_max_counts_items_after_the_probed_sentence() {
        let schema = Schema::Object(vec![Field::required(
            "tags",
            Schema::ArrayMax(Box::new(Schema::trimmed_string(1, 40)), 20),
        )]);
        let many: Vec<String> = (0..21).map(|i| format!("t{i}")).collect();
        let root = json!({ "tags": many });
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'tags': Too big: expected array to have <=20 items"
        );
        let few: Vec<String> = (0..20).map(|i| format!("t{i}")).collect();
        let (_, issues) = validate(&schema, &json!({ "tags": few }));
        assert!(issues.is_empty());
        // Element issues still carry their index alongside the cap.
        let root = json!({"tags": ["ok", ""]});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'tags[1]': Too small: expected string to have >=1 characters"
        );
    }

    #[test]
    fn datetime_is_zods_dialect_and_every_miss_is_one_sentence() {
        let schema = Schema::Object(vec![Field::required(
            "dueDate",
            Schema::optional(Schema::nullable(Schema::DateTime)),
        )]);
        for good in [
            "2026-03-06T00:00:00Z",
            "2026-03-06T00:00:00.000Z",
            "2026-03-06T23:59:59.1Z",
        ] {
            let (_, issues) = validate(&schema, &json!({ "dueDate": good }));
            assert!(issues.is_empty(), "{good}");
        }
        // Every one of these answered the SAME probe issue: invalid_format,
        // message "Invalid ISO datetime" — the shapes a model actually
        // writes when it guesses at a date field.
        for bad in [
            "2026-03-06",                // date only
            "Friday",                    // prose
            "next friday",               // prose
            "03/06/2026",                // slash date
            "2026-03-06 17:00",          // a space where the T belongs
            "2026-03-06T09:00",          // no seconds, no Z
            "2026-03-06T09:00:00+01:00", // an offset, which zod refuses
            "2026-03-06t09:00:00z",      // lowercase separators
            "2026-13-06T00:00:00Z",      // month 13
            "2026-02-30T00:00:00Z",      // Feb 30
            "2026-03-06T24:00:00Z",      // hour 24
            "2026-03-06T00:00:60Z",      // second 60
            "2026-03-06T00:00:00.Z",     // a fraction with no digits
        ] {
            let root = json!({ "dueDate": bad });
            let (_, issues) = validate(&schema, &root);
            assert_eq!(issues.len(), 1, "{bad}");
            assert_eq!(
                describe_issue(&issues[0], &root),
                "field 'dueDate': Invalid ISO datetime",
                "{bad}"
            );
        }
        // The wrapper contract: absent passes, null passes (nullable), and a
        // number is an invalid_type with zod's `expected string` word.
        let (_, issues) = validate(&schema, &json!({}));
        assert!(issues.is_empty());
        let (_, issues) = validate(&schema, &json!({"dueDate": null}));
        assert!(issues.is_empty());
        let root = json!({"dueDate": 5});
        let (_, issues) = validate(&schema, &root);
        assert_eq!(
            describe_issue(&issues[0], &root),
            "field 'dueDate' should be string, got number"
        );
    }
}
