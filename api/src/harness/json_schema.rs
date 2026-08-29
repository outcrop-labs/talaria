// THE HARNESS'S OWN SCHEMA, PUT ON THE WIRE — the port of
// harness/json-schema.ts.
//
// WHAT WAS WRONG. Every JSON harness declares a schema, and Talaria used it
// in exactly one place: validation, AFTER the reply came back. What went OUT
// was `response_format: { type: 'json_object' }` — "some JSON, shape
// unspecified" — plus a sentence of prose describing the shape. So the
// contract was enforced by rejecting bad answers rather than by preventing
// them, and two separate failures followed from it:
//
//   ANTHROPIC 400s ON IT OUTRIGHT. Its OpenAI-compatible layer accepts only
//   `response_format.type: 'json_schema'`, so every structured call to a
//   Claude model failed at the protocol. Nine of twenty-six harnesses read
//   0% on claude-haiku while every text harness read 100%.
//
//   EVERYWHERE ELSE IT UNDER-CONSTRAINS. `json_object` guarantees syntax and
//   says nothing about keys or types — the blurb-writer bug this repo found
//   the hard way: a reply keyed by tidied-up display names parsed fine,
//   wrote zero blurbs, and recorded a perfect contract. A schema on the wire
//   makes that reply unrepresentable rather than detectable after the fact.
//
// STRICT IS PER-SCHEMA, NOT A GLOBAL SWITCH: `strict: true` is the mode that
// guarantees conformance, providers only accept it for schemas whose objects
// have fixed keys and forbid extras, and some harnesses cannot satisfy that
// and are not wrong to exist — the blurb writer is an open-keyed map by
// design, because the keys are the caller's vendor ids. So this module
// decides per schema, and says which it got. A non-strict `json_schema` is
// still enormously better than `json_object`: the provider sees the types,
// and the validator remains the backstop for the relational half a schema
// cannot state.
//
// THE RENDERER (`render`) IS PROBED, not imagined: zod's `z.toJSONSchema(
// { io: 'input' })` was run over every shape the defs declare and its exact
// documents — key order included — are reproduced and pinned in the tests.
// (zod prepends a `$schema` document annotation at the root; the renderer
// omits it because the ONLY consumers here are `for_wire`, which drops it,
// and `prompt_shape`, which ignores it — the omission is invisible by
// construction.) `io: 'input'` is itself load-bearing: a `.transform()` runs
// AFTER parsing, so the shape the MODEL emits is the input side, and the
// first TS draft that rendered the OUTPUT side rendered five of nine
// harnesses as an empty schema that Anthropic rejects.

use super::schema::Schema;
use serde_json::{Map, Value};

/// A schema ready for `response_format.json_schema`.
#[derive(Clone)]
pub struct WireSchema {
    /// Required by the wire format. Derived from the harness id, which is
    /// stable and unique, with the characters providers reject removed.
    pub name: String,
    pub schema: Value,
    /// Whether the provider may be asked to GUARANTEE conformance. False
    /// means the schema is legal JSON Schema but not strict-eligible.
    pub strict: bool,
}

/// Providers accept `^[a-zA-Z0-9_-]+$` for the schema name; harness ids
/// carry `:` (`muse:ticket`) and `/` never appears but is cheap to cover.
fn wire_name(harness_id: &str) -> String {
    let cleaned: String = harness_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed: String = cleaned.chars().take(64).collect();
    if trimmed.is_empty() {
        "response".into()
    } else {
        trimmed
    }
}

/// THE KEYWORDS THAT GO ON THE WIRE — structure only, never validation.
///
/// Providers implement different subsets of JSON Schema and reject what they
/// do not know, so a schema rendered faithfully from the declaration is a
/// 400 waiting to happen. Anthropic found the first one live: "For 'array'
/// type, 'minItems' values other than 0 or 1 are not supported (got:
/// [2, 5])". Every provider has a list like that and none of them agree.
///
/// THE DIVISION OF LABOUR SETTLES IT. The wire schema exists to SHAPE
/// DECODING: which keys, of what types, nested how. The validator on the
/// way back enforces every constraint dropped here. So sending `minItems: 2`
/// buys nothing (a reply with one item is rejected on parse either way) and
/// risks losing the entire call to a 400.
///
/// An allowlist rather than a blocklist, because the failure directions are
/// not symmetric: an unknown keyword we forgot to drop is a 400 on every
/// call a harness makes, and an unknown keyword we drop by accident is a
/// slightly looser hint to the decoder.
const WIRE_KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "prefixItems",
    "enum",
    "const",
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    "$defs",
    "$ref",
    // Kept because it is INSTRUCTION, not validation: a field description is
    // one of the more effective ways to get the right value into the right
    // key.
    "description",
    "title",
];

fn is_object(v: &Value) -> bool {
    v.as_object().is_some()
}

/// WHAT MAKES A SCHEMA STRICT-ELIGIBLE, walked rather than guessed.
///
/// Every object node must forbid extra properties and require every property
/// it declares. A node that allows open keys (`additionalProperties` absent,
/// true, or a schema) is the map case and cannot be strict. Composition
/// keywords are walked into, because an eligible union of ineligible members
/// is not a thing.
///
/// Returns false rather than throwing on anything it does not recognize: the
/// cost of a false negative is one non-strict request, and the cost of a
/// false positive is a 400 on every call the harness makes.
pub fn strict_eligible(node: &Value, depth: usize) -> bool {
    if depth > 12 || !is_object(node) {
        return false;
    }
    let obj = node.as_object().unwrap();
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(branch)) = obj.get(key) {
            return branch.iter().all(|b| strict_eligible(b, depth + 1));
        }
    }
    if let Some(defs) = obj.get("$defs").filter(|d| is_object(d))
        && !defs
            .as_object()
            .unwrap()
            .values()
            .all(|d| strict_eligible(d, depth + 1))
    {
        return false;
    }
    match obj.get("type") {
        Some(Value::String(t)) if t == "array" => match obj.get("items") {
            None => true,
            Some(items) => strict_eligible(items, depth + 1),
        },
        Some(Value::String(t)) if t == "object" => {
            if obj.get("additionalProperties") != Some(&Value::Bool(false)) {
                return false;
            }
            let Some(props) = obj.get("properties").filter(|p| is_object(p)) else {
                return false;
            };
            let required: Vec<&str> = match obj.get("required") {
                Some(Value::Array(list)) => list.iter().filter_map(|v| v.as_str()).collect(),
                _ => Vec::new(),
            };
            let props = props.as_object().unwrap();
            if props.keys().any(|p| !required.contains(&p.as_str())) {
                return false;
            }
            props.values().all(|p| strict_eligible(p, depth + 1))
        }
        // A non-object, non-array node constrains its own value and nothing
        // below it.
        Some(_) => true,
        None => false,
    }
}

/// Tidy the emitted schema for the wire. It does exactly two things, and the
/// list of things it REFUSES to do is the important half.
///
/// It keeps only the structural keywords (`WIRE_KEYWORDS`) and closes a
/// fixed-key object that came out open.
///
/// IT NEVER TOUCHES `required`, and an earlier draft of the TS file did.
/// Strict mode wants every property listed there, so it is tempting to add
/// the missing ones — but a property is missing from `required` because the
/// harness declared it OPTIONAL, and forcing it would demand a value the
/// model may not have. A schema with optional fields is simply not
/// strict-eligible, and sending it non-strict is the correct, lossless
/// answer.
///
/// It likewise never closes an open MAP (`z.record` emits
/// `additionalProperties: { type: 'string' }` and MEANS it) — same rule: the
/// wire mode adapts to the declared contract, never the other way round.
fn for_wire(node: &Value, depth: usize) -> Value {
    if depth > 12 || !is_object(node) {
        return node.clone();
    }
    let obj = node.as_object().unwrap();
    let mut out: Map<String, Value> = obj
        .iter()
        .filter(|(k, _)| WIRE_KEYWORDS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(branch)) = out.get(key) {
            let branch = branch.clone();
            out.insert(
                key.into(),
                Value::Array(branch.iter().map(|b| for_wire(b, depth + 1)).collect()),
            );
        }
    }
    if let Some(defs) = out.get("$defs").filter(|d| is_object(d)).cloned() {
        let defs = defs.as_object().unwrap().clone();
        let tidied: Map<String, Value> = defs
            .iter()
            .map(|(k, v)| (k.clone(), for_wire(v, depth + 1)))
            .collect();
        out.insert("$defs".into(), Value::Object(tidied));
    }
    if out.get("type") == Some(&Value::String("array".into())) && out.contains_key("items") {
        let item = out.get("items").unwrap().clone();
        out.insert("items".into(), for_wire(&item, depth + 1));
    }
    if out.get("type") == Some(&Value::String("object".into()))
        && out.get("properties").is_some_and(is_object)
    {
        let props = out.get("properties").unwrap().as_object().unwrap().clone();
        let tidied: Map<String, Value> = props
            .iter()
            .map(|(k, v)| (k.clone(), for_wire(v, depth + 1)))
            .collect();
        out.insert("properties".into(), Value::Object(tidied));
        if !out.contains_key("additionalProperties") {
            out.insert("additionalProperties".into(), Value::Bool(false));
        }
    }
    Value::Object(out)
}

/// THE SHAPE, WRITTEN OUT FOR THE MODEL TO READ.
///
/// WHY THIS EXISTS. Every structured call carried one sentence — "Reply with
/// exactly one JSON value and nothing else" — and never said WHAT SHAPE. A
/// frontier model infers it from the surrounding prose and looks fine, which
/// is exactly why nobody noticed: the harness was leaning on the model to do
/// work the harness already had the answer to. A 7-14B model does not infer
/// it, and the whole premise of this layer is that it should not have to.
///
/// Not raw JSON Schema — that is verbose, and a `$ref`-heavy document is a
/// worse prompt than no document. This is the shape a person would sketch.
///
/// IT IS BELT AND BRACES, NOT A FALLBACK. It goes out even when
/// `response_format` constrains decoding: a provider can drop the parameter,
/// and the prompt survives it. Bounded, because a prompt is not a schema
/// document — a shape that will not fit is omitted rather than truncated
/// into something misleading.
///
/// Returns None when there is nothing useful to say.
pub fn prompt_shape(schema: &Value, budget: usize) -> Option<String> {
    fn render(node: &Value, depth: usize) -> String {
        if depth > 4 || !is_object(node) {
            return "value".into();
        }
        let obj = node.as_object().unwrap();
        if let Some(Value::Array(items)) = obj.get("enum") {
            return items
                .iter()
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" | ");
        }
        if let Some(c) = obj.get("const") {
            return serde_json::to_string(c).unwrap_or_else(|_| "value".into());
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(Value::Array(branch)) = obj.get(key) {
                let mut seen: Vec<String> = Vec::new();
                for b in branch {
                    let r = render(b, depth + 1);
                    if !seen.contains(&r) {
                        seen.push(r);
                    }
                }
                return seen.join(" | ");
            }
        }
        match obj.get("type") {
            Some(Value::String(t)) if t == "array" => {
                let item = obj.get("items").cloned().unwrap_or(Value::Null);
                format!("[{}, …]", render(&item, depth + 1))
            }
            Some(Value::String(t)) if t == "object" => {
                let props = obj.get("properties").and_then(|p| p.as_object());
                let Some(props) = props.filter(|p| !p.is_empty()) else {
                    // An open map — `z.record`. Say so rather than printing
                    // `{}`, which reads as "an empty object" and is the
                    // opposite of what it means.
                    return match obj.get("additionalProperties").filter(|v| is_object(v)) {
                        Some(value) => format!("{{\"<key>\": {}, …}}", render(value, depth + 1)),
                        None => "{…}".into(),
                    };
                };
                let required: Vec<&str> = match obj.get("required") {
                    Some(Value::Array(list)) => list.iter().filter_map(|v| v.as_str()).collect(),
                    _ => Vec::new(),
                };
                let body = props
                    .iter()
                    .map(|(k, v)| {
                        let key = serde_json::to_string(k).unwrap_or_default();
                        let opt = if required.contains(&k.as_str()) {
                            ""
                        } else {
                            "?"
                        };
                        format!("{key}{opt}: {}", render(v, depth + 1))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{body}}}")
            }
            Some(Value::String(t)) => t.clone(),
            _ => "value".into(),
        }
    }

    let shape = render(schema, 0);
    if shape == "value" || shape == "{…}" || shape.chars().count() > budget {
        return None;
    }
    Some(shape)
}

// ── The renderer: Schema → the JSON Schema document zod would emit ──────────

/// Render a `Schema` as the JSON Schema document `z.toJSONSchema(
/// { io: 'input' })` produces for the equivalent zod declaration — key order
/// included, because the wire doc's byte shape follows from it under the
/// preserve-order map. Probed against zod 4.3.6; every branch cites what the
/// probe printed. See the module header for why `$schema` is omitted.
pub fn render(schema: &Schema) -> Value {
    let mut node = Map::new();
    match schema {
        // z.unknown() renders as the empty document — "any value". The wire
        // side's empty-schema check turns that into "send no schema", which
        // is the answer Anthropic requires rather than a bug to soften.
        Schema::Unknown => {}
        Schema::Str { min, max, .. } => {
            node.insert("type".into(), "string".into());
            if let Some(min) = min {
                node.insert("minLength".into(), (*min).into());
            }
            if let Some(max) = max {
                node.insert("maxLength".into(), (*max).into());
            }
        }
        Schema::Num => {
            node.insert("type".into(), "number".into());
        }
        Schema::Bool => {
            node.insert("type".into(), "boolean".into());
        }
        Schema::Enum(members) => {
            node.insert("type".into(), "string".into());
            node.insert(
                "enum".into(),
                Value::Array(members.iter().map(|m| Value::String(m.clone())).collect()),
            );
        }
        Schema::Array(item) => {
            node.insert("type".into(), "array".into());
            node.insert("items".into(), render(item));
        }
        Schema::Object(fields) => {
            node.insert("type".into(), "object".into());
            let mut props = Map::new();
            let mut required = Vec::new();
            for field in fields {
                props.insert(field.name.to_string(), render(&field.schema));
                // zod's required list carries every field except the ones a
                // wrapper made optional — `.nullable()` alone does NOT.
                if !matches!(field.schema, Schema::Optional(_) | Schema::Defaulted(_, _)) {
                    required.push(Value::String(field.name.to_string()));
                }
            }
            node.insert("properties".into(), Value::Object(props));
            if !required.is_empty() {
                node.insert("required".into(), Value::Array(required));
            }
        }
        Schema::Record(value_schema) => {
            node.insert("type".into(), "object".into());
            node.insert(
                "propertyNames".into(),
                serde_json::json!({"type": "string"}),
            );
            node.insert("additionalProperties".into(), render(value_schema));
        }
        Schema::Union(branches) => {
            node.insert(
                "anyOf".into(),
                Value::Array(branches.iter().map(render).collect()),
            );
        }
        // Probe: a bare nullable string collapses to a type array
        // `["string","null"]`; anything with more than a bare type (enum,
        // array, object, bounds) renders as anyOf with a null member.
        Schema::Nullable(inner) => {
            let rendered = render(inner);
            let bare_string = rendered.as_object().is_some_and(|o| o.len() == 1)
                && rendered.get("type") == Some(&Value::String("string".into()));
            if bare_string {
                node.insert("type".into(), serde_json::json!(["string", "null"]));
            } else {
                node.insert(
                    "anyOf".into(),
                    Value::Array(vec![rendered, serde_json::json!({"type": "null"})]),
                );
            }
        }
        // Optionality lives in the parent's required list; the node itself
        // renders as the inner shape.
        Schema::Optional(inner) => return render(inner),
        // Probe: `.default(v)` puts `default` FIRST, before `type`.
        Schema::Defaulted(inner, default) => {
            node.insert("default".into(), default.clone());
            let rendered = render(inner);
            if let Value::Object(inner_obj) = rendered {
                for (k, v) in inner_obj {
                    node.insert(k, v);
                }
            }
        }
    }
    Value::Object(node)
}

/// The harness's schema, ready for the wire — or None when this build cannot
/// express it, in which case the caller falls back to `json_object` and the
/// prompt anchor rather than sending something a provider will reject.
///
/// NEVER FAILS. It runs on the hot path of every structured call, and a
/// schema that cannot render is a reason to send a weaker request, never a
/// reason to fail a run that would otherwise have worked.
pub fn wire_schema_of(harness_id: &str, schema: &Schema) -> Option<WireSchema> {
    let raw = render(schema);
    let wire = for_wire(&raw, 0);
    // AN EMPTY SCHEMA IS NOT A SCHEMA. It is legal JSON Schema meaning "any
    // value", it constrains nothing, and Anthropic refuses it outright
    // ("Empty schema ({}) that accepts any JSON value is not supported").
    // Returning None sends the request without a `response_format` and lets
    // the prompt anchor carry the ask — weaker, and enormously better than
    // a 400.
    let obj = wire.as_object()?;
    if obj.is_empty() {
        return None;
    }
    let strict = strict_eligible(&wire, 0);
    Some(WireSchema {
        name: wire_name(harness_id),
        schema: wire,
        strict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn judge_schema() -> Schema {
        Schema::Object(vec![
            super::super::schema::Field::required(
                "verdict",
                Schema::Enum(["pass", "fail"].iter().map(|s| s.to_string()).collect()),
            ),
            super::super::schema::Field::required("summary", Schema::string()),
        ])
    }

    fn obj(fields: Vec<super::super::schema::Field>) -> Schema {
        Schema::Object(fields)
    }

    // ── wire_schema_of ─────────────────────────────────────────────────────

    #[test]
    fn renders_the_shape_the_model_must_produce() {
        let wire = wire_schema_of("judge", &judge_schema()).unwrap();
        // Exact document, key order included — zod emits type, properties,
        // required; for_wire appends additionalProperties.
        assert_eq!(
            wire.schema,
            json!({
                "type": "object",
                "properties": {
                    "verdict": {"type": "string", "enum": ["pass", "fail"]},
                    "summary": {"type": "string"}
                },
                "required": ["verdict", "summary"],
                "additionalProperties": false
            })
        );
        assert!(wire.strict);
    }

    #[test]
    fn drops_schema_document_annotations_providers_reject() {
        let wire = wire_schema_of(
            "t",
            &obj(vec![super::super::schema::Field::required(
                "a",
                Schema::string(),
            )]),
        )
        .unwrap();
        assert!(wire.schema.get("$schema").is_none());
    }

    #[test]
    fn makes_a_name_the_wire_format_accepts_out_of_a_harness_id() {
        // Ids carry ':' (`muse:ticket`); the field is `^[a-zA-Z0-9_-]+$`.
        assert_eq!(
            wire_schema_of(
                "muse:ticket",
                &obj(vec![super::super::schema::Field::required(
                    "a",
                    Schema::string()
                )])
            )
            .unwrap()
            .name,
            "muse_ticket"
        );
    }

    #[test]
    fn sends_an_open_keyed_map_non_strict_rather_than_closing_it() {
        // blurb-writer: `z.record(z.string(), z.string())`, open by design
        // because the keys are the caller's vendor ids. Strict would 400;
        // closing it would rewrite the contract the harness declared.
        let wire =
            wire_schema_of("blurb-writer", &Schema::Record(Box::new(Schema::string()))).unwrap();
        assert!(!wire.strict);
        assert_eq!(
            wire.schema.get("additionalProperties"),
            Some(&json!({"type": "string"}))
        );
    }

    #[test]
    fn sends_a_schema_with_optional_fields_non_strict_and_never_forces_them_required() {
        // THE HAZARD AN EARLIER DRAFT WALKED INTO. Strict wants every
        // property in `required`, so adding the missing ones is tempting —
        // but a key is absent from `required` because the harness declared
        // it optional. Forcing it would rewrite the contract into one its
        // own parser fails.
        let wire = wire_schema_of(
            "t",
            &obj(vec![
                super::super::schema::Field::required("a", Schema::string()),
                super::super::schema::Field::required("b", Schema::optional(Schema::Num)),
            ]),
        )
        .unwrap();
        assert_eq!(wire.schema.get("required"), Some(&json!(["a"])));
        assert!(!wire.strict);
    }

    #[test]
    fn a_schema_that_cannot_express_itself_answers_none_not_an_empty_doc() {
        // The TS side asks zod to render and catches a throw (z.custom); the
        // Rust equivalent is the one shape that renders as the empty
        // document — z.unknown(). Both land here: no response_format, prompt
        // anchor only. Failing the run is the one wrong answer.
        assert!(wire_schema_of("t", &Schema::Unknown).is_none());
    }

    // ── strict_eligible ────────────────────────────────────────────────────

    #[test]
    fn refuses_a_nested_object_that_is_left_open() {
        let open = json!({
            "type": "object",
            "properties": {"inner": {"type": "object"}},
            "required": ["inner"],
            "additionalProperties": false
        });
        assert!(!strict_eligible(&open, 0));
    }

    #[test]
    fn walks_arrays_and_unions_rather_than_judging_only_the_root() {
        let wire = wire_schema_of(
            "t",
            &obj(vec![super::super::schema::Field::required(
                "rows",
                Schema::Array(Box::new(obj(vec![super::super::schema::Field::required(
                    "id",
                    Schema::string(),
                )]))),
            )]),
        )
        .unwrap();
        assert!(wire.strict);
        assert!(!strict_eligible(
            &json!({"anyOf": [{"type": "string"}, {"type": "object"}]}),
            0
        ));
    }

    // ── the provider subset ────────────────────────────────────────────────

    #[test]
    fn drops_validation_keywords_keeping_only_structure() {
        // ANTHROPIC FOUND THIS ONE LIVE: "For 'array' type, 'minItems'
        // values other than 0 or 1 are not supported". Providers implement
        // different subsets and reject what they do not know.
        let wire = wire_schema_of(
            "t",
            &obj(vec![super::super::schema::Field::required(
                "queries",
                Schema::Array(Box::new(Schema::Str {
                    trim: false,
                    min: Some(4),
                    max: None,
                })),
            )]),
        )
        .unwrap();
        let queries = wire
            .schema
            .get("properties")
            .unwrap()
            .get("queries")
            .unwrap();
        assert_eq!(queries.get("type"), Some(&json!("array")));
        assert!(queries.get("minItems").is_none());
        assert!(queries.get("maxItems").is_none());
        assert!(queries.get("items").unwrap().get("minLength").is_none());
    }

    #[test]
    fn keeps_the_constraint_enforced_where_it_belongs_on_the_way_back() {
        // Nothing is lost by dropping them: the validator still parses the
        // reply, so a bound is enforced on validate. The wire schema shapes
        // decoding; it was never the validator.
        let schema = obj(vec![super::super::schema::Field::required(
            "q",
            Schema::Str {
                trim: false,
                min: Some(2),
                max: None,
            },
        )]);
        let (_, issues) = super::super::schema::validate(&schema, &json!({"q": "x"}));
        assert!(!issues.is_empty());
        let (_, issues) = super::super::schema::validate(&schema, &json!({"q": "xy"}));
        assert!(issues.is_empty());
    }

    // ── the shape a small model is told to produce ──────────────────────────

    #[test]
    fn writes_the_schema_out_as_something_a_person_would_sketch() {
        // THE LAZINESS THIS ENDS: "reply with exactly one JSON value" with no
        // shape, while the harness one line away held the schema.
        let wire = wire_schema_of(
            "judge",
            &Schema::Object(vec![
                super::super::schema::Field::required(
                    "verdict",
                    Schema::Enum(["pass", "revise"].iter().map(|s| s.to_string()).collect()),
                ),
                super::super::schema::Field::required("summary", Schema::string()),
                super::super::schema::Field::required(
                    "issues",
                    Schema::optional(Schema::Array(Box::new(Schema::string()))),
                ),
            ]),
        )
        .unwrap();
        assert_eq!(
            prompt_shape(&wire.schema, 600),
            Some("{\"verdict\": \"pass\" | \"revise\", \"summary\": string, \"issues\"?: [string, …]}".into())
        );
    }

    #[test]
    fn marks_optional_keys_so_a_small_model_does_not_invent_one() {
        let wire = wire_schema_of(
            "t",
            &Schema::Object(vec![
                super::super::schema::Field::required("a", Schema::string()),
                super::super::schema::Field::required("b", Schema::optional(Schema::Num)),
            ]),
        )
        .unwrap();
        assert_eq!(
            prompt_shape(&wire.schema, 600),
            Some("{\"a\": string, \"b\"?: number}".into())
        );
    }

    #[test]
    fn says_an_open_map_is_a_map_rather_than_printing_an_empty_object() {
        // `{}` reads as "an empty object", which is the opposite of what a
        // record means — and blurb-writer's whole contract is an open map.
        let wire =
            wire_schema_of("blurb-writer", &Schema::Record(Box::new(Schema::string()))).unwrap();
        assert_eq!(
            prompt_shape(&wire.schema, 600),
            Some("{\"<key>\": string, …}".into())
        );
    }

    #[test]
    fn omits_a_shape_too_large_to_be_a_prompt_rather_than_truncating_it() {
        // A truncated shape is worse than none: it reads as the whole
        // contract and is not one.
        let fields: Vec<super::super::schema::Field> = (0..80)
            .map(|i| {
                super::super::schema::Field::required(
                    Box::leak(format!("field_{i}").into_boxed_str()),
                    Schema::string(),
                )
            })
            .collect();
        let wire = wire_schema_of("t", &Schema::Object(fields)).unwrap();
        assert_eq!(prompt_shape(&wire.schema, 600), None);
    }

    // ── the renderer's probed documents ─────────────────────────────────────

    #[test]
    fn the_renderer_matches_zods_documents_key_order_and_all() {
        use super::super::schema::Field;
        // Every assertion is a `z.toJSONSchema({io:'input'})` probe output.
        assert_eq!(render(&Schema::string()), json!({"type": "string"}));
        assert_eq!(
            render(&Schema::Str {
                trim: true,
                min: Some(1),
                max: Some(40)
            }),
            json!({"type": "string", "minLength": 1, "maxLength": 40})
        );
        assert_eq!(render(&Schema::Num), json!({"type": "number"}));
        assert_eq!(render(&Schema::Bool), json!({"type": "boolean"}));
        assert_eq!(
            render(&Schema::Enum(
                ["a", "b"].iter().map(|s| s.to_string()).collect()
            )),
            json!({"type": "string", "enum": ["a", "b"]})
        );
        assert_eq!(
            render(&Schema::Array(Box::new(Schema::string()))),
            json!({"type": "array", "items": {"type": "string"}})
        );
        assert_eq!(
            render(&Schema::Nullable(Box::new(Schema::string()))),
            json!({"type": ["string", "null"]})
        );
        assert_eq!(
            render(&Schema::Nullable(Box::new(Schema::Array(Box::new(
                Schema::string()
            ))))),
            json!({"anyOf": [{"type": "array", "items": {"type": "string"}}, {"type": "null"}]})
        );
        assert_eq!(
            render(&Schema::Nullable(Box::new(Schema::Enum(
                ["a", "b"].iter().map(|s| s.to_string()).collect()
            )))),
            json!({"anyOf": [{"type": "string", "enum": ["a", "b"]}, {"type": "null"}]})
        );
        // nullable alone does NOT make a field optional: it stays in
        // required, and the wire doc says so.
        assert_eq!(
            render(&Schema::Object(vec![Field::required(
                "tags",
                Schema::nullable(Schema::Array(Box::new(Schema::string())))
            )])),
            json!({"type": "object", "properties": {"tags": {"anyOf": [{"type": "array", "items": {"type": "string"}}, {"type": "null"}]}}, "required": ["tags"]})
        );
        // `.default(v)` puts default FIRST — the probe's exact order.
        assert_eq!(
            render(&Schema::Object(vec![Field::required(
                "def",
                Schema::with_default(Schema::Array(Box::new(Schema::string())), json!([])),
            )])),
            json!({"type": "object", "properties": {"def": {"default": [], "type": "array", "items": {"type": "string"}}}})
        );
        // propertyNames marks the record; for_wire drops it (not structural
        // for decoding) but strict_eligible reads additionalProperties and
        // refuses the map.
        assert_eq!(
            render(&Schema::Record(Box::new(Schema::string()))),
            json!({"type": "object", "propertyNames": {"type": "string"}, "additionalProperties": {"type": "string"}})
        );
        assert_eq!(
            render(&Schema::Union(vec![
                Schema::string(),
                Schema::Array(Box::new(Schema::string()))
            ])),
            json!({"anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]})
        );
    }
}
