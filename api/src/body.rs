// Body validation — parseBody's contract (ui/src/server/api-guard.ts): the
// first zod issue's message rides a 400 {error}. The message STRINGS are part
// of the wire (the SPA surfaces them), so they are ported verbatim from the
// ui's own zod 4.3 and pinned by tests probed against it. zod 4 spells them:
//   "Invalid input: expected object, received null"
//   "Invalid input: expected string, received undefined"   (missing member)
//   "Invalid input: expected string, received number"      (wrong JSON type)
//   "Too small: expected string to have >=1 characters"
//   "Too big: expected string to have <=200 characters"
//   "Invalid email address"
// Checks run in schema order — first failure wins, like zod's issue list.
//
// This module is pure: it returns the MESSAGE, and the route wraps it in the
// 400 envelope. (TS's parseBody builds the Response itself; here the HTTP
// concern stays with the routes so these functions stay trivially testable.)

use serde_json::Value;

/// The request body as JSON, null when unparsable — exactly
/// `request.json().catch(() => null)`.
pub fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

pub(crate) fn zod_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub fn object_msg(received: &str) -> String {
    format!("Invalid input: expected object, received {received}")
}

pub fn array_msg(received: &str) -> String {
    format!("Invalid input: expected array, received {received}")
}

fn string_msg(received: &str) -> String {
    format!("Invalid input: expected string, received {received}")
}

pub fn too_small_msg(min: usize) -> String {
    format!("Too small: expected string to have >={min} characters")
}

pub fn too_big_msg(max: usize) -> String {
    format!("Too big: expected string to have <={max} characters")
}

/// zod's array-length message — always "items", never "item(s)".
pub fn array_too_big_msg(max: usize) -> String {
    format!("Too big: expected array to have <={max} items")
}

/// zod's record type message — "record", not "object", is the received word
/// (a z.record that isn't given one).
pub fn record_msg(received: &str) -> String {
    format!("Invalid input: expected record, received {received}")
}

/// The root z.object check: a non-object body's first issue.
pub fn as_object(body: &Value) -> Result<&serde_json::Map<String, Value>, String> {
    body.as_object()
        .ok_or_else(|| object_msg(zod_type_name(body)))
}

/// JS string length (UTF-16 code units) — what zod's min/max count. An emoji
/// outside the BMP counts 2, so chars().count() would disagree with the TS
/// route on exactly the strings people put in display names.
pub(crate) fn utf16_len(s: &str) -> usize {
    s.chars()
        .map(|c| usize::from((c as u32) > 0xFFFF))
        .sum::<usize>()
        + s.chars().count()
}

/// JS `s.slice(0, max)` for a UTF-16 budget: the longest `&str` prefix whose
/// UTF-16 length stays within `max`. A cut that would split a surrogate pair
/// yields the prefix one unit short rather than a broken half-pair — the one
/// documented divergence from the TS, which happily produces lone surrogates.
pub(crate) fn truncate_utf16(s: &str, max: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &s[..i];
        }
    }
    s
}

/// A required string member with min/max checks, in zod's order: type, then
/// length. `min`/`max` are the inclusive bounds exactly as zod prints them.
pub fn string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<String, String> {
    let v = obj.get(key).ok_or_else(|| string_msg("undefined"))?;
    let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
    let n = utf16_len(s);
    if n < min {
        return Err(too_small_msg(min));
    }
    if n > max {
        return Err(too_big_msg(max));
    }
    Ok(s.to_string())
}

/// An optional string member: absent passes (undefined is what `.optional()`
/// admits); present must be a string within bounds.
pub fn optional_string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(_) => string_member(obj, key, 1, max).map(Some),
    }
}

/// A max-only optional string (`z.string().max(n).optional()` — no min): the
/// empty string is a legal value here (workflows' description).
pub fn optional_max_string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None); // absent — what `.optional()` admits
    };
    let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
    if utf16_len(s) > max {
        return Err(too_big_msg(max));
    }
    Ok(Some(s.to_string()))
}

/// The trimmed string member (`z.string().trim().min(n).max(m)`): the trim
/// runs BEFORE the length checks, so a 79-char name padded to 81 raw passes
/// and a spaces-only name is the min failure. The returned value is trimmed.
pub fn trimmed_string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<String, String> {
    let v = obj.get(key).ok_or_else(|| string_msg("undefined"))?;
    let s = v
        .as_str()
        .ok_or_else(|| string_msg(zod_type_name(v)))?
        .trim()
        .to_string();
    let n = utf16_len(&s);
    if n < min {
        return Err(too_small_msg(min));
    }
    if n > max {
        return Err(too_big_msg(max));
    }
    Ok(s)
}

/// An optional array-of-strings member (`z.array(z.string().min(a).max(b))
/// .max(n).optional()`). Elements validate BEFORE the array-length check —
/// zod 4's issue order, probed: a bad element outranks an over-long array.
pub fn optional_string_array_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    elem_min: usize,
    elem_max: usize,
    max_items: usize,
) -> Result<Option<Vec<String>>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None); // absent — what `.optional()` admits
    };
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::with_capacity(arr.len());
    for el in arr {
        let s = el.as_str().ok_or_else(|| string_msg(zod_type_name(el)))?;
        let n = utf16_len(s);
        if n < elem_min {
            return Err(too_small_msg(elem_min));
        }
        if n > elem_max {
            return Err(too_big_msg(elem_max));
        }
        out.push(s.to_string());
    }
    if arr.len() > max_items {
        return Err(array_too_big_msg(max_items));
    }
    Ok(Some(out))
}

/// An optional array-of-uuids member (`z.array(Uuid).max(n).optional()`):
/// element type first (the string message), then the format.
pub fn optional_uuid_array_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max_items: usize,
) -> Result<Option<Vec<String>>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::with_capacity(arr.len());
    for el in arr {
        let s = el.as_str().ok_or_else(|| string_msg(zod_type_name(el)))?;
        if !zod_uuid_ok(s) {
            return Err("Invalid UUID".into());
        }
        out.push(s.to_string());
    }
    if arr.len() > max_items {
        return Err(array_too_big_msg(max_items));
    }
    Ok(Some(out))
}

/// A REQUIRED array-of-uuids member (`z.array(Uuid).max(n)`): absent answers
/// the array type message on undefined — the length bound is a check, not an
/// option; the member itself is not optional.
pub fn uuid_array_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max_items: usize,
) -> Result<Vec<String>, String> {
    optional_uuid_array_member(obj, key, max_items)?.ok_or_else(|| array_msg("undefined"))
}

/// A required uuid-or-null member (`Uuid.nullable()`): null is a VALUE
/// (None — "no default"), absent is zod's undefined message on the string
/// type. For bodies where clearing the setting and omitting the key are
/// different requests.
pub fn nullable_uuid_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Err(string_msg("undefined")),
        Some(Value::Null) => Ok(None),
        Some(_) => uuid_member(obj, key).map(Some),
    }
}

/// A plain `z.string().email()` member (api-schema's Email): no preprocess,
/// no length bound — the value crosses exactly as sent. (The claim route's
/// trim-and-lower variant is preprocessed_email_member below.)
pub fn email_member(obj: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    let v = obj.get(key).ok_or_else(|| string_msg("undefined"))?;
    let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
    if !zod_email_ok(s) {
        return Err("Invalid email address".into());
    }
    Ok(s.to_string())
}

/// The claim route's preprocessed email: a STRING is trimmed + lowercased
/// before any check (the credential's email is its login key, so it is stored
/// in the form login will look up), then zod's order — email validity, then
/// max 200. Non-strings fail the type check on the raw value, exactly the
/// preprocess pass-through.
pub fn preprocessed_email_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<String, String> {
    let v = obj.get(key).ok_or_else(|| string_msg("undefined"))?;
    let s = v
        .as_str()
        .ok_or_else(|| string_msg(zod_type_name(v)))?
        .trim()
        .to_lowercase();
    if !zod_email_ok(&s) {
        return Err("Invalid email address".into());
    }
    if utf16_len(&s) > max {
        return Err(too_big_msg(max));
    }
    Ok(s)
}

/// zod v4's "practical email" pattern (zod/v4/core/regexes.js), hand-rolled
/// because one pattern is not a regex dependency:
/// /^(?!\.)(?!.*\.\.)([A-Za-z0-9_'+\-.]*)[A-Za-z0-9_+-]@([A-Za-z0-9][A-Za-z0-9\-]*\.)+[A-Za-z]{2,}$/
pub fn zod_email_ok(s: &str) -> bool {
    let Some(at) = s.find('@') else { return false };
    let (local, rest) = s.split_at(at);
    let domain = &rest[1..];

    // (?!\.) and (?!.*\.\.) — no leading dot in the local part, no two
    // consecutive dots anywhere.
    if local.starts_with('.') || s.contains("..") {
        return false;
    }
    // ([A-Za-z0-9_'+\-.]*)[A-Za-z0-9_+-] — at least one char, every char from
    // the permissive set, and the LAST from the tail set (no trailing
    // dot/quote/apostrophe).
    let in_set = |c: char| c.is_ascii_alphanumeric() || matches!(c, '\'' | '_' | '+' | '-' | '.');
    let in_tail = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-');
    if local.is_empty() || !local.chars().all(in_set) || !local.chars().last().is_some_and(in_tail)
    {
        return false;
    }
    // ([A-Za-z0-9][A-Za-z0-9\-]*\.)+ … — everything before the final dot is a
    // dot-separated run of labels, each starting alnum and running alnum/dash.
    // … [A-Za-z]{2,}$ — the final label is 2+ letters, letters only.
    // rsplit_once hands back (before, after): the TLD is the after part.
    let Some((labels, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    // Empty (a leading dot: "a@.co") or any empty label ("a@b..c.de") fails.
    if labels.is_empty() {
        return false;
    }
    labels.split('.').all(|label| {
        matches!(label.chars().next(), Some(c) if c.is_ascii_alphanumeric())
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// A required boolean member — zod's type message only (no bounds to check).
pub fn boolean_member(obj: &serde_json::Map<String, Value>, key: &str) -> Result<bool, String> {
    let v = obj.get(key).ok_or_else(|| boolean_msg("undefined"))?;
    v.as_bool().ok_or_else(|| boolean_msg(zod_type_name(v)))
}

fn boolean_msg(received: &str) -> String {
    format!("Invalid input: expected boolean, received {received}")
}

/// A boolean-or-null member (`z.boolean().nullable()`): absent, like the
/// optional string, is the caller's business (`.optional()` wraps it).
pub fn nullable_boolean_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| boolean_msg(zod_type_name(v))),
    }
}

/// A string member that may be null or absent (`z.string().max(n)
/// .nullable().optional()`): absent and null both pass as None; a present
/// string must fit max. Length runs from 1 — the shape never declares a min
/// beside a nullable, and '' is a legal "clear" value on every route using it.
pub fn nullable_optional_string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => string_member(obj, key, 1, max).map(Some),
    }
}

/// A string member whose NULL IS A VALUE (`z.string().min(1).max(n)
/// .nullable().optional()`): absent is None, present-null is Some(None), a
/// present string is Some(s) within bounds. For routes where "clear the
/// setting" and "don't touch it" are different requests (me's preferences) —
/// the folded helper above cannot tell them apart.
pub fn present_nullable_string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<Option<String>>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => string_member(obj, key, 1, max).map(|s| Some(Some(s))),
    }
}

/// A no-min present-nullable string (`z.string().max(n).nullable().optional()`)
/// — the empty string is a legal VALUE here, distinct from absent and from
/// present-null. admin.model-roles' `model` uses this shape: zod accepts `""`
/// and the HANDLER's truthiness decides it means "clear the assignment".
pub fn present_nullable_max_string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<Option<String>>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => string_member(obj, key, 0, max).map(|s| Some(Some(s))),
    }
}

/// zod's uuid check (`z.string().uuid()` → "Invalid UUID"): the canonical
/// 8-4-4-4-12 hex layout, either case. Not WHICH version — v4 and the
/// name-derived v8s both pass here.
pub fn zod_uuid_ok(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    let dash = |i: usize| bytes[i] == b'-';
    let hexrun = |range: std::ops::Range<usize>| bytes[range].iter().all(|b| b.is_ascii_hexdigit());
    dash(8)
        && dash(13)
        && dash(18)
        && dash(23)
        && hexrun(0..8)
        && hexrun(9..13)
        && hexrun(14..18)
        && hexrun(19..23)
        && hexrun(24..36)
}

/// A required uuid member: type, then format, in zod's order.
pub fn uuid_member(obj: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    let v = obj.get(key).ok_or_else(|| string_msg("undefined"))?;
    let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
    if !zod_uuid_ok(s) {
        return Err("Invalid UUID".into());
    }
    Ok(s.to_string())
}

/// zod's enum message — the exact quoted-pipe list, in catalog order:
///   Invalid option: expected one of "a"|"b"|"c"
pub fn enum_msg(options: &[&str]) -> String {
    let quoted: Vec<String> = options.iter().map(|o| format!("\"{o}\"")).collect();
    format!("Invalid option: expected one of {}", quoted.join("|"))
}

/// An enum member: present and one of `options`, else zod's enum message.
pub fn enum_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    options: &[&str],
) -> Result<String, String> {
    let v = obj.get(key).ok_or_else(|| enum_msg(options))?;
    let s = v.as_str().ok_or_else(|| enum_msg(options))?;
    if options.contains(&s) {
        Ok(s.to_string())
    } else {
        Err(enum_msg(options))
    }
}

/// An OPTIONAL enum (`z.enum([...]).optional()`): absent passes as None, a
/// present value answers the enum's message whatever its JSON type. The
/// boards/tasks patch bodies lean on this shape heavily.
pub fn optional_enum_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    options: &[&str],
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(_) => enum_member(obj, key, options).map(Some),
    }
}

/// A NULLISH enum (`z.enum([...]).nullish()`): absent AND null both pass as
/// None — the create-side conflation, where a null effort and a missing one
/// land the same. (A PATCH tells them apart; feed this to `nullish_member`
/// as the inner reader there.)
pub fn nullish_enum_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    options: &[&str],
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => enum_member(obj, key, options).map(Some),
    }
}

/// An optional boolean (`z.boolean().optional()`): absent is None; a present
/// value — null included — must be a boolean. (The nullable sibling folds
/// null in; this one is the stricter optional-only shape.)
pub fn optional_boolean_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| {
            format!(
                "Invalid input: expected boolean, received {}",
                zod_type_name(v)
            )
        }),
    }
}

/// A nullish uuid (`Uuid.nullish()`): absent and null both pass as None; a
/// present value must be a uuid, type message then format, in zod's order.
pub fn optional_uuid_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => uuid_member(obj, key).map(Some),
    }
}

/// zod 4's `z.string().datetime()` under default params, transcribed from
/// zod/src/v4/core/regexes.ts: a real proleptic date (per-month day bounds
/// and the leap-year alternatives in the source), then `T`, an `hh:mm` that
/// may carry seconds and any-length fraction, then a bare `Z` — no offsets,
/// no local times, unless the schema asked for them and these routes never
/// do. The failure message is zod 4's `Invalid ISO datetime` (the
/// invalid_format case through the "datetime" → "ISO datetime" dictionary).
static ISO_DATETIME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"^(?:(?:\d\d[2468][048]|\d\d[13579][26]|\d\d0[48]|[02468][048]00|[13579][26]00)-02-29|\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\d|30)|02-(?:0[1-9]|1\d|2[0-8])))T(?:[01]\d|2[0-3]):[0-5]\d(?::[0-5]\d(?:\.\d+)?)?Z$",
    )
    .unwrap()
});

/// A nullish ISO datetime (`z.string().datetime().nullish()`): absent and
/// null both pass as None; a present value must be a string the datetime
/// regex admits.
pub fn nullish_datetime_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let s = v.as_str().ok_or_else(|| {
        format!(
            "Invalid input: expected string, received {}",
            zod_type_name(v)
        )
    })?;
    if !ISO_DATETIME.is_match(s) {
        return Err("Invalid ISO datetime".into());
    }
    Ok(Some(s.to_string()))
}

/// A uuid whose NULL IS A VALUE (`Uuid.nullable().optional()`): absent is
/// None, present-null is Some(None) — "clear it" — and a present string is
/// Some(id). The three-state shape routes like the boards patch body use to
/// tell "move to personal" from "don't touch the team".
pub fn present_nullable_uuid_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<Option<String>>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => uuid_member(obj, key).map(|s| Some(Some(s))),
    }
}

/// The lowercase-kebab pattern (`/^[a-z0-9]+(-[a-z0-9]+)*$/`) — the SLUG/DEPT
/// shape the role-template dialog enforces. Runs AFTER zod's own length
/// checks, exactly where `.regex()` sits in the chain.
pub fn kebab_ok(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// The role-template SLUG/DEPT member: `z.string().min(2).max(60)
/// .regex(KEBAB, 'lowercase-kebab')` — the custom message is the literal
/// string "lowercase-kebab" (zod 4 prints the message verbatim, no received
/// clause), and it only fires after the length checks pass.
pub fn kebab_member(obj: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    let s = string_member(obj, key, 2, 60)?;
    if !kebab_ok(&s) {
        return Err("lowercase-kebab".into());
    }
    Ok(s)
}

/// A required string-or-null member (`z.string().min(n).max(m).nullable()`):
/// absent is the type error on undefined (the key is required — only the
/// VALUE may be null), null is Ok(None), a present string runs the bounds.
pub fn nullable_string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        Some(Value::Null) => Ok(None),
        Some(_) => string_member(obj, key, min, max).map(Some),
        None => Err(string_msg("undefined")),
    }
}

/// A `z.literal(true)` member — instance verify's "the action, on purpose".
/// zod 4 prints the same message for every non-true input, received type
/// included: "Invalid input: expected true".
pub fn literal_true_member(obj: &serde_json::Map<String, Value>, key: &str) -> Result<(), String> {
    match obj.get(key) {
        Some(Value::Bool(true)) => Ok(()),
        _ => Err("Invalid input: expected true".into()),
    }
}

/// The `.int()` flavor of z.number(), for members declared with the guard.
#[derive(Clone, Copy, PartialEq)]
pub enum NumKind {
    Int,
    Float,
}

/// A float the way JSON.stringify prints it: an integral value serializes as
/// an integer ("0", "1"), not Rust's "0.0". Any number that rides the wire
/// — parsed request echoes, DB columns read as f64 — goes through this so a
/// 1000-token cap is byte-identical to TS's.
pub fn js_num(v: f64) -> serde_json::Number {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.007_199_254_740_992e15 {
        serde_json::Number::from(v as i64)
    } else {
        serde_json::Number::from_f64(v).expect("finite f64")
    }
}

/// A bound as zod prints it — these schemas use whole-number bounds and zod
/// never shows "1000000000.0".
fn fmt_bound(b: f64) -> String {
    if b.fract() == 0.0 && b.abs() < 1e15 {
        format!("{}", b as i64)
    } else {
        format!("{b}")
    }
}

/// An optional+nullable z.number() member (`.nullish()`): absent and null both
/// pass. Numbers run type → int guard → min → max, zod's order. The message
/// table is zod 4.3.6's, probed — including its odd corners: the int guard
/// reports the SAFE-INTEGER bounds (±9007199254740991) for anything beyond
/// them, each side naming its own, and a plain bound breach on an int field
/// still says "expected number", not "expected int".
pub fn nullable_number_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    kind: NumKind,
    min: f64,
    max: f64,
) -> Result<Option<f64>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None); // absent — what `.optional()` admits
    };
    if v.is_null() {
        return Ok(None); // null — what `.nullable()` admits
    }
    let Some(n) = v.as_f64() else {
        return Err(format!(
            "Invalid input: expected number, received {}",
            zod_type_name(v)
        ));
    };
    if kind == NumKind::Int {
        if n.fract() != 0.0 {
            return Err("Invalid input: expected int, received number".into());
        }
        // The guard is two-sided and each side names ITS OWN safe bound.
        if n > 9_007_199_254_740_991.0 {
            return Err("Too big: expected int to be <=9007199254740991".into());
        }
        if n < -9_007_199_254_740_991.0 {
            return Err("Too small: expected int to be >=-9007199254740991".into());
        }
    }
    if n < min {
        return Err(format!(
            "Too small: expected number to be >={}",
            fmt_bound(min)
        ));
    }
    if n > max {
        return Err(format!(
            "Too big: expected number to be <={}",
            fmt_bound(max)
        ));
    }
    Ok(Some(n))
}

/// An `.optional()`-ONLY number (`z.number().…().optional()`, no
/// `.nullable()`): absent passes, but a present null is a type error — zod
/// refuses it, and so does this. The nullish sibling above folds null in;
/// this one is the stricter shape.
pub fn optional_number_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    kind: NumKind,
    min: f64,
    max: f64,
) -> Result<Option<f64>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Null) => Err("Invalid input: expected number, received null".into()),
        Some(_) => nullable_number_member(obj, key, kind, min, max),
    }
}

/// A REQUIRED number (`z.number().…()` with no optional/nullable): absent is
/// the undefined type error, zod's order.
pub fn number_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    kind: NumKind,
    min: f64,
    max: f64,
) -> Result<f64, String> {
    match obj.get(key) {
        None => Err("Invalid input: expected number, received undefined".into()),
        Some(_) => nullable_number_member(obj, key, kind, min, max)
            .and_then(|v| v.ok_or_else(|| "Invalid input: expected number, received null".into())),
    }
}

/// A `.nullish()` PATCH column — the tri-state an update carries: absent is
/// `None` (leave the column alone), null is `Some(None)` (clear it), and a
/// present value reads through `read` (an optional-shaped member helper,
/// called only on the present branch). The conflated helpers above answer
/// create-side questions where null and absent land the same; a PATCH can
/// tell them apart, and this is the combinator that does.
pub fn nullish_member<T>(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    read: impl Fn(&serde_json::Map<String, Value>, &str) -> Result<Option<T>, String>,
) -> Result<Option<Option<T>>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        // The member is present and non-null, so the optional-shaped reader
        // can only answer Some(value) or an error — its absent/null arms are
        // unreachable here.
        Some(_) => read(obj, key).map(Some),
    }
}

#[cfg(test)]
mod datetime_tests {
    use super::*;
    use serde_json::json;

    fn dt(v: Value) -> Result<Option<String>, String> {
        nullish_datetime_member(v.as_object().unwrap(), "dueDate")
    }

    #[test]
    fn nullish_datetime_admits_zod_shapes_and_refuses_the_rest() {
        // The shapes zod 4's default datetime admits.
        assert_eq!(
            dt(json!({ "dueDate": "2026-08-29T12:00:00Z" }))
                .unwrap()
                .as_deref(),
            Some("2026-08-29T12:00:00Z")
        );
        assert_eq!(
            dt(json!({ "dueDate": "2026-08-29T12:00Z" }))
                .unwrap()
                .as_deref(),
            Some("2026-08-29T12:00Z")
        );
        assert_eq!(
            dt(json!({ "dueDate": "2026-08-29T12:00:00.123456Z" }))
                .unwrap()
                .as_deref(),
            Some("2026-08-29T12:00:00.123456Z")
        );
        // Leap day on a leap year.
        assert!(dt(json!({ "dueDate": "2024-02-29T00:00:00Z" })).is_ok());
        // nullish.
        assert_eq!(dt(json!({})).unwrap(), None);
        assert_eq!(dt(json!({ "dueDate": null })).unwrap(), None);
        // Offsets and local times are outside default params.
        assert_eq!(
            dt(json!({ "dueDate": "2026-08-29T12:00:00+02:00" })).unwrap_err(),
            "Invalid ISO datetime"
        );
        assert_eq!(
            dt(json!({ "dueDate": "2026-08-29T12:00:00" })).unwrap_err(),
            "Invalid ISO datetime"
        );
        // Date-only, impossible months/days, impossible times.
        assert_eq!(
            dt(json!({ "dueDate": "2026-08-29Z" })).unwrap_err(),
            "Invalid ISO datetime"
        );
        assert_eq!(
            dt(json!({ "dueDate": "2026-13-01T00:00:00Z" })).unwrap_err(),
            "Invalid ISO datetime"
        );
        assert_eq!(
            dt(json!({ "dueDate": "2026-04-31T00:00:00Z" })).unwrap_err(),
            "Invalid ISO datetime"
        );
        assert_eq!(
            dt(json!({ "dueDate": "2023-02-29T00:00:00Z" })).unwrap_err(),
            "Invalid ISO datetime"
        );
        assert_eq!(
            dt(json!({ "dueDate": "2026-08-29T24:00:00Z" })).unwrap_err(),
            "Invalid ISO datetime"
        );
        assert_eq!(
            dt(json!({ "dueDate": "2026-08-29T12:60:00Z" })).unwrap_err(),
            "Invalid ISO datetime"
        );
        // Non-strings are a type error, not a format one.
        assert_eq!(
            dt(json!({ "dueDate": 17 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zod_messages_are_verbatim() {
        // Each string printed by the ui's own zod 4.3.6 for that case.
        assert_eq!(
            object_msg("null"),
            "Invalid input: expected object, received null"
        );
        assert_eq!(
            object_msg("array"),
            "Invalid input: expected object, received array"
        );
        assert_eq!(
            string_msg("undefined"),
            "Invalid input: expected string, received undefined"
        );
        assert_eq!(
            string_msg("number"),
            "Invalid input: expected string, received number"
        );
        assert_eq!(
            too_small_msg(1),
            "Too small: expected string to have >=1 characters"
        );
        assert_eq!(
            too_big_msg(200),
            "Too big: expected string to have <=200 characters"
        );
    }

    #[test]
    fn string_members_check_type_then_length_in_zod_order() {
        let obj = json!({ "username": 5, "password": "x", "empty": "", "name": null });
        let o = obj.as_object().unwrap();
        assert_eq!(
            string_member(o, "username", 1, 200).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert_eq!(string_member(o, "password", 1, 1000).unwrap(), "x");
        assert_eq!(
            string_member(o, "missing", 1, 1000).unwrap_err(),
            "Invalid input: expected string, received undefined"
        );
        assert_eq!(
            string_member(o, "empty", 1, 1000).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
        assert!(optional_string_member(o, "missing", 200).unwrap().is_none());
        assert!(
            optional_string_member(o, "password", 200)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn email_truth_table_matches_zod_probes() {
        // Every row probed against z.string().email() in the ui's zod 4.3.6.
        let ok = [
            "a@b.co",
            "A@B.CO",
            "a.b@x.io",
            "a+b@c.de",
            "a_b@c.de",
            "a@b-.co",
            "ab@c.de",
            "a@b.coke",
            "a@b--c.de",
            "a@b.c-.de",
            "aa@bb.cc.dd.ee",
            "'a@b.co",
            "'+._a@b.co",
        ];
        let bad = [
            "a@localhost",
            "a@10.0.0.1",
            "a@b.co.",
            "a@b_c.io",
            "a..b@c.de",
            ".a@b.co",
            "a@-b.co",
            "a@b.c",
            "ü@b.co",
            "a@b.co_",
            "a@b.c_n",
            "a@b+c.de",
            "a!b@c.de",
            "a@b..c.de",
            "a@b.co..",
            "",
            "plainstring",
            "a@",
            "@b.co",
            "a@.b.co",
            "a.@b.co",
        ];
        for s in ok {
            assert!(zod_email_ok(s), "should accept {s:?}");
        }
        for s in bad {
            assert!(!zod_email_ok(s), "should reject {s:?}");
        }
    }

    #[test]
    fn length_counts_utf16_units_like_js() {
        // "😀" is one char, two UTF-16 units — a 100-emoji name is 200 to zod.
        let emoji: String = "😀".repeat(100);
        assert_eq!(utf16_len(&emoji), 200);
        let obj = json!({ "name": emoji });
        assert!(string_member(obj.as_object().unwrap(), "name", 1, 200).is_ok());
        let over = json!({ "name": "😀".repeat(101) });
        assert_eq!(
            string_member(over.as_object().unwrap(), "name", 1, 200).unwrap_err(),
            "Too big: expected string to have <=200 characters"
        );
    }

    #[test]
    fn boolean_and_nullable_shapes_match_zod_probes() {
        let obj = json!({ "on": true, "off": "x", "nul": null, "blank": null });
        let o = obj.as_object().unwrap();
        assert!(boolean_member(o, "on").is_ok());
        assert_eq!(
            boolean_member(o, "off").unwrap_err(),
            "Invalid input: expected boolean, received string"
        );
        assert_eq!(
            boolean_member(o, "missing").unwrap_err(),
            "Invalid input: expected boolean, received undefined"
        );
        assert!(nullable_boolean_member(o, "nul").unwrap().is_none());
        assert!(nullable_boolean_member(o, "missing").unwrap().is_none());
        assert_eq!(
            nullable_boolean_member(o, "off").unwrap_err(),
            "Invalid input: expected boolean, received string"
        );
        let hd = json!({ "hd": 5, "clear": null, "set": "x" });
        let h = hd.as_object().unwrap();
        assert!(
            nullable_optional_string_member(h, "absent", 200)
                .unwrap()
                .is_none()
        );
        assert!(
            nullable_optional_string_member(h, "clear", 200)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            nullable_optional_string_member(h, "hd", 200).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        // The present-vs-absent split: absent is None, null is Some(None),
        // a value is Some(Some) — the explicit clear stays distinguishable.
        assert_eq!(
            present_nullable_string_member(h, "absent", 200).unwrap(),
            None
        );
        assert_eq!(
            present_nullable_string_member(h, "clear", 200).unwrap(),
            Some(None)
        );
        assert_eq!(
            present_nullable_string_member(h, "set", 200).unwrap(),
            Some(Some("x".to_string()))
        );
        assert_eq!(
            present_nullable_string_member(h, "hd", 200).unwrap_err(),
            "Invalid input: expected string, received number"
        );
    }

    #[test]
    fn uuid_and_enum_messages_match_zod_probes() {
        let obj = json!({ "userId": "xyz", "good": "67b06c14-7c2a-4fe5-91a4-1d0d2b8b2d81" });
        let o = obj.as_object().unwrap();
        assert!(uuid_member(o, "good").is_ok());
        // Uppercase hex passes — zod's uuid is case-insensitive.
        assert!(
            uuid_member(
                json!({ "u": "67B06C14-7C2A-4FE5-91A4-1D0D2B8B2D81" })
                    .as_object()
                    .unwrap(),
                "u"
            )
            .is_ok()
        );
        assert_eq!(uuid_member(o, "userId").unwrap_err(), "Invalid UUID");
        assert_eq!(
            uuid_member(o, "missing").unwrap_err(),
            "Invalid input: expected string, received undefined"
        );
        let catalog = ["agents.manage", "research.run", "plans.create"];
        let listed = json!({ "perm": "nope", "ok": "research.run" });
        let l = listed.as_object().unwrap();
        assert_eq!(
            enum_member(l, "perm", &catalog).unwrap_err(),
            "Invalid option: expected one of \"agents.manage\"|\"research.run\"|\"plans.create\""
        );
        // Missing and wrong-type both surface the same enum message — zod 4
        // folds them into the option list.
        assert_eq!(
            enum_member(json!({}).as_object().unwrap(), "perm", &catalog).unwrap_err(),
            "Invalid option: expected one of \"agents.manage\"|\"research.run\"|\"plans.create\""
        );
        assert_eq!(
            enum_member(o, "userId", &catalog).unwrap_err(),
            "Invalid option: expected one of \"agents.manage\"|\"research.run\"|\"plans.create\""
        );
        assert_eq!(enum_member(l, "ok", &catalog).unwrap(), "research.run");
    }

    #[test]
    fn kebab_and_literal_true_match_zod_probes() {
        assert!(kebab_ok("software-engineer"));
        assert!(kebab_ok("swe2"));
        assert!(!kebab_ok("BAD"));
        assert!(!kebab_ok("-lead"));
        assert!(!kebab_ok("lead-"));
        assert!(!kebab_ok("a--b"));
        assert!(!kebab_ok(""));
        // The route's SLUG/DEPT member: length messages first (min 2, max 60),
        // then the literal custom message the route passes .regex().
        let v = json!({ "slug": "software-engineer", "one": "a", "big": "x".repeat(61), "bad": "Bad_Slug" });
        let o = v.as_object().unwrap();
        assert_eq!(kebab_member(o, "slug").unwrap(), "software-engineer");
        assert_eq!(
            kebab_member(o, "one").unwrap_err(),
            "Too small: expected string to have >=2 characters"
        );
        assert_eq!(
            kebab_member(o, "big").unwrap_err(),
            "Too big: expected string to have <=60 characters"
        );
        assert_eq!(kebab_member(o, "bad").unwrap_err(), "lowercase-kebab");
        assert_eq!(
            kebab_member(o, "missing").unwrap_err(),
            "Invalid input: expected string, received undefined"
        );
        // z.literal(true): one message for every non-true input, type included.
        let t = json!({ "verify": true, "no": false, "str": "yes", "nul": null });
        let o = t.as_object().unwrap();
        assert!(literal_true_member(o, "verify").is_ok());
        for k in ["no", "str", "nul", "missing"] {
            assert_eq!(
                literal_true_member(o, k).unwrap_err(),
                "Invalid input: expected true",
                "case {k}"
            );
        }
        // z.string().min(3).max(253).nullable() — the instance domain member.
        let d = json!({ "clear": null, "set": "a.bb", "short": "ab", "str": 5 });
        let o = d.as_object().unwrap();
        assert!(
            nullable_string_member(o, "clear", 3, 253)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            nullable_string_member(o, "set", 3, 253).unwrap().as_deref(),
            Some("a.bb")
        );
        assert_eq!(
            nullable_string_member(o, "short", 3, 253).unwrap_err(),
            "Too small: expected string to have >=3 characters"
        );
        assert_eq!(
            nullable_string_member(o, "str", 3, 253).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert_eq!(
            nullable_string_member(json!({}).as_object().unwrap(), "domain", 3, 253).unwrap_err(),
            "Invalid input: expected string, received undefined"
        );
    }

    #[test]
    fn nullable_number_members_match_zod_probes() {
        use super::NumKind;
        // The key-policy schema (keys.$id): int fields max 1e15/10_000, a
        // float field max 1e9 — every row probed against zod 4.3.6.
        let num =
            |v: Value| nullable_number_member(v.as_object().unwrap(), "v", NumKind::Int, 0.0, 1e15);
        assert_eq!(num(json!({ "v": 250 })).unwrap(), Some(250.0));
        assert_eq!(num(json!({ "v": 0 })).unwrap(), Some(0.0));
        assert_eq!(num(json!({ "v": 1e15 })).unwrap(), Some(1e15));
        assert!(num(json!({})).unwrap().is_none()); // nullish: absent…
        assert!(num(json!({ "v": null })).unwrap().is_none()); // …and null
        assert_eq!(
            num(json!({ "v": "x" })).unwrap_err(),
            "Invalid input: expected number, received string"
        );
        assert_eq!(
            num(json!({ "v": true })).unwrap_err(),
            "Invalid input: expected number, received boolean"
        );
        // The int guard runs before the bounds — a negative FRACTION reports
        // the guard, not the min.
        assert_eq!(
            num(json!({ "v": -0.5 })).unwrap_err(),
            "Invalid input: expected int, received number"
        );
        assert_eq!(
            num(json!({ "v": 2.5 })).unwrap_err(),
            "Invalid input: expected int, received number"
        );
        // Beyond safe-integer, the guard reports THE CEILING — not the max —
        // and each side names its own bound.
        assert_eq!(
            num(json!({ "v": 1e16 })).unwrap_err(),
            "Too big: expected int to be <=9007199254740991"
        );
        assert_eq!(
            num(json!({ "v": -1e16 })).unwrap_err(),
            "Too small: expected int to be >=-9007199254740991"
        );
        // An integral breach of the field's own max still says "number".
        assert_eq!(
            num(json!({ "v": 2e15 })).unwrap_err(),
            "Too big: expected number to be <=1000000000000000"
        );
        // Min first, then max, with zod's bound spellings.
        assert_eq!(
            num(json!({ "v": -1 })).unwrap_err(),
            "Too small: expected number to be >=0"
        );
        let float = |v: Value| {
            nullable_number_member(v.as_object().unwrap(), "v", NumKind::Float, 0.0, 1e9)
        };
        assert_eq!(float(json!({ "v": 0.5 })).unwrap(), Some(0.5)); // no int guard
        assert_eq!(
            float(json!({ "v": 2e9 })).unwrap_err(),
            "Too big: expected number to be <=1000000000"
        );
        let rpm = |v: Value| {
            nullable_number_member(v.as_object().unwrap(), "v", NumKind::Int, 0.0, 10_000.0)
        };
        assert_eq!(
            rpm(json!({ "v": 20_000 })).unwrap_err(),
            "Too big: expected number to be <=10000"
        );
    }

    #[test]
    fn optional_and_required_numbers_tell_null_from_absent() {
        use super::NumKind;
        // addTimeSpentSeconds's shape: `.optional()` only — null is a TYPE
        // error, unlike the nullish sibling above.
        let opt = |v: Value| {
            optional_number_member(v.as_object().unwrap(), "v", NumKind::Float, 0.0, 30.0)
        };
        assert_eq!(opt(json!({})).unwrap(), None);
        assert_eq!(opt(json!({ "v": 12.5 })).unwrap(), Some(12.5));
        assert_eq!(
            opt(json!({ "v": null })).unwrap_err(),
            "Invalid input: expected number, received null"
        );
        // promptTokens's shape: required, int-bounded.
        let req = |v: Value| number_member(v.as_object().unwrap(), "v", NumKind::Int, 0.0, 10.0);
        assert_eq!(req(json!({ "v": 3 })).unwrap(), 3.0);
        assert_eq!(
            req(json!({})).unwrap_err(),
            "Invalid input: expected number, received undefined"
        );
        assert_eq!(
            req(json!({ "v": null })).unwrap_err(),
            "Invalid input: expected number, received null"
        );
        assert_eq!(
            req(json!({ "v": 11 })).unwrap_err(),
            "Too big: expected number to be <=10"
        );
    }

    #[test]
    fn nullish_member_carries_the_patch_tri_state() {
        // The PATCH columns: absent = leave alone, null = clear, value = set.
        // Inner reader is the max-only string (description's shape).
        let tri = |v: Value| {
            nullish_member(v.as_object().unwrap(), "d", |o, k| {
                optional_max_string_member(o, k, 5)
            })
        };
        assert_eq!(tri(json!({})).unwrap(), None); // untouched
        assert_eq!(tri(json!({ "d": null })).unwrap(), Some(None)); // cleared
        assert_eq!(
            tri(json!({ "d": "abc" })).unwrap(),
            Some(Some("abc".into()))
        );
        assert_eq!(
            tri(json!({ "d": "toolong" })).unwrap_err(),
            "Too big: expected string to have <=5 characters"
        );
        assert_eq!(
            tri(json!({ "d": 5 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        // The nullish enum inner reader (effort's shape) under the combinator.
        let effort = |v: Value| {
            nullish_member(v.as_object().unwrap(), "e", |o, k| {
                nullish_enum_member(o, k, &["xs", "s"])
            })
        };
        assert_eq!(effort(json!({})).unwrap(), None);
        assert_eq!(effort(json!({ "e": null })).unwrap(), Some(None));
        assert_eq!(
            effort(json!({ "e": "xs" })).unwrap(),
            Some(Some("xs".into()))
        );
        assert_eq!(
            effort(json!({ "e": "xl" })).unwrap_err(),
            "Invalid option: expected one of \"xs\"|\"s\""
        );
    }

    #[test]
    fn trimmed_max_only_and_array_members_match_zod_probes() {
        // Every row probed against the workflows schema in the ui's zod
        // 4.3.6: z.string().trim().min(1).max(80), z.string().max(500)
        // .optional(), and the arrays behind match/skills/toolkits.
        let trimmed = |v: Value| trimmed_string_member(v.as_object().unwrap(), "name", 1, 80);
        assert_eq!(trimmed(json!({ "name": "  x  " })).unwrap(), "x");
        assert_eq!(
            trimmed(json!({ "name": "   " })).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
        // 81 raw chars, 79 after the trim — the bounds see the trimmed value.
        assert_eq!(
            trimmed(json!({ "name": format!("  {}  ", "x".repeat(79)) })).unwrap(),
            "x".repeat(79)
        );
        assert_eq!(
            trimmed(json!({ "name": "x".repeat(81) })).unwrap_err(),
            "Too big: expected string to have <=80 characters"
        );
        assert_eq!(
            trimmed(json!({ "name": 5 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert_eq!(
            trimmed(json!({})).unwrap_err(),
            "Invalid input: expected string, received undefined"
        );

        let desc =
            |v: Value| optional_max_string_member(v.as_object().unwrap(), "description", 500);
        assert_eq!(
            desc(json!({ "name": "x", "description": "" })).unwrap(),
            Some("".into())
        );
        assert_eq!(desc(json!({})).unwrap(), None);
        assert_eq!(
            desc(json!({ "description": "x".repeat(501) })).unwrap_err(),
            "Too big: expected string to have <=500 characters"
        );
        assert_eq!(
            desc(json!({ "description": null })).unwrap_err(),
            "Invalid input: expected string, received null"
        );

        let arr =
            |v: Value| optional_string_array_member(v.as_object().unwrap(), "labels", 1, 60, 30);
        assert_eq!(
            arr(json!({ "labels": "nope" })).unwrap_err(),
            "Invalid input: expected array, received string"
        );
        assert_eq!(
            arr(json!({ "labels": null })).unwrap_err(),
            "Invalid input: expected array, received null"
        );
        assert_eq!(
            arr(json!({ "labels": [5] })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert_eq!(
            arr(json!({ "labels": [""] })).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
        assert_eq!(
            arr(json!({ "labels": ["x".repeat(61)] })).unwrap_err(),
            "Too big: expected string to have <=60 characters"
        );
        // Elements before length: a bad last element outranks the 31st item.
        let mut long = vec!["a"; 30];
        long.push("");
        assert_eq!(
            arr(json!({ "labels": long })).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
        assert_eq!(
            arr(json!({ "labels": vec!["a"; 31] })).unwrap_err(),
            "Too big: expected array to have <=30 items"
        );
        assert_eq!(
            arr(json!({ "labels": ["a", "b"] })).unwrap(),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(arr(json!({ "labels": [] })).unwrap(), Some(vec![]));
        assert_eq!(arr(json!({})).unwrap(), None);

        let uuids = |v: Value| optional_uuid_array_member(v.as_object().unwrap(), "boards", 30);
        assert_eq!(
            uuids(json!({ "boards": ["nope"] })).unwrap_err(),
            "Invalid UUID"
        );
        assert_eq!(
            uuids(json!({ "boards": [5] })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert_eq!(
            uuids(json!({ "boards": vec!["67b06c14-7c2a-4fe5-91a4-1d0d2b8b2d81"; 31] }))
                .unwrap_err(),
            "Too big: expected array to have <=30 items"
        );
        assert!(
            uuids(json!({ "boards": ["67b06c14-7c2a-4fe5-91a4-1d0d2b8b2d81"] }))
                .unwrap()
                .is_some()
        );

        // The toolkit element is an object check — object_msg, not the array
        // spelling: "Invalid input: expected object, received string".
        assert_eq!(
            object_msg("string"),
            "Invalid input: expected object, received string"
        );
        // The prefs record's type message — "record" is the received word a
        // z.record prints, probed for array/null/string/number alike.
        assert_eq!(
            record_msg("array"),
            "Invalid input: expected record, received array"
        );
        assert_eq!(
            record_msg("null"),
            "Invalid input: expected record, received null"
        );
        assert_eq!(
            record_msg("string"),
            "Invalid input: expected record, received string"
        );
        assert_eq!(
            record_msg("number"),
            "Invalid input: expected record, received number"
        );
        assert_eq!(
            object_msg("number"),
            "Invalid input: expected object, received number"
        );
        assert_eq!(
            array_too_big_msg(20),
            "Too big: expected array to have <=20 items"
        );
    }

    #[test]
    fn preprocessed_email_trims_lowers_then_validates() {
        let email = |v: Value| preprocessed_email_member(v.as_object().unwrap(), "email", 200);
        // Preprocess happens first: whitespace + case fall away.
        assert_eq!(
            email(json!({ "email": "  Jon@GetBoxie.COM " })).unwrap(),
            "jon@getboxie.com"
        );
        // Then zod's order: validity before length, on the PREPROCESSED value.
        assert_eq!(
            email(json!({ "email": 5 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert_eq!(
            email(json!({})).unwrap_err(),
            "Invalid input: expected string, received undefined"
        );
        assert_eq!(
            email(json!({ "email": "nope" })).unwrap_err(),
            "Invalid email address"
        );
        // A local part padded to the 200 boundary (195 + '@' + "b.co").
        assert!(email(json!({ "email": format!("{}@b.co", "a".repeat(195)) })).is_ok());
        assert_eq!(
            email(json!({ "email": format!("{}@b.co", "a".repeat(196)) })).unwrap_err(),
            "Too big: expected string to have <=200 characters"
        );
    }

    #[test]
    fn required_uuid_array_and_nullable_uuid_match_zod_probes() {
        // z.array(Uuid).max(50) — the template bindings' templateIds.
        let arr = |v: Value| uuid_array_member(v.as_object().unwrap(), "templateIds", 50);
        assert_eq!(
            arr(json!({})).unwrap_err(),
            "Invalid input: expected array, received undefined"
        );
        assert_eq!(
            arr(json!({ "templateIds": null })).unwrap_err(),
            "Invalid input: expected array, received null"
        );
        assert_eq!(
            arr(json!({ "templateIds": [] })).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            arr(json!({ "templateIds": vec!["67b06c14-7c2a-4fe5-91a4-1d0d2b8b2d81"; 51] }))
                .unwrap_err(),
            "Too big: expected array to have <=50 items"
        );
        assert_eq!(
            arr(json!({ "templateIds": ["not-a-uuid"] })).unwrap_err(),
            "Invalid UUID"
        );
        // Uuid.nullable() — defaultId. Null is a value; absent is the error;
        // a present string runs the uuid format check.
        let def = |v: Value| nullable_uuid_member(v.as_object().unwrap(), "defaultId");
        assert_eq!(def(json!({ "defaultId": null })).unwrap(), None);
        assert_eq!(
            def(json!({ "defaultId": "67b06c14-7c2a-4fe5-91a4-1d0d2b8b2d81" })).unwrap(),
            Some("67b06c14-7c2a-4fe5-91a4-1d0d2b8b2d81".into())
        );
        assert_eq!(
            def(json!({})).unwrap_err(),
            "Invalid input: expected string, received undefined"
        );
        assert_eq!(
            def(json!({ "defaultId": 5 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert_eq!(
            def(json!({ "defaultId": "nope" })).unwrap_err(),
            "Invalid UUID"
        );
    }
}
