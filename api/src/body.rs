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

fn string_msg(received: &str) -> String {
    format!("Invalid input: expected string, received {received}")
}

pub fn too_small_msg(min: usize) -> String {
    format!("Too small: expected string to have >={min} characters")
}

pub fn too_big_msg(max: usize) -> String {
    format!("Too big: expected string to have <={max} characters")
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
}
