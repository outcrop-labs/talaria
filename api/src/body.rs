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
