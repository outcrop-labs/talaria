// YAML 1.1-aware emitter for Hermes/PyYAML re-read equality.
use serde_json::{Map, Value};

fn yaml11_bool_like(s: &str) -> bool {
    matches!(
        s,
        "y" | "Y"
            | "yes"
            | "Yes"
            | "YES"
            | "n"
            | "N"
            | "no"
            | "No"
            | "NO"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    )
}

fn yaml11_null_like(s: &str) -> bool {
    matches!(s, "~" | "null" | "Null" | "NULL")
}

/// 1.1 integers: decimal (with `_` separators — `0400` is octal under 1.1,
/// still an int), plus the 0x/0o/0b radix forms.
fn yaml11_int_like(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    let (radix, digits) = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, h)
    } else if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        (8, o)
    } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        (2, b)
    } else {
        (10, t)
    };
    !digits.is_empty() && digits.chars().all(|c| c == '_' || c.is_digit(radix))
}

/// 1.1 floats: `.inf`/`.nan` specials plus mantissa[.frac][e±exp] with `_`
/// separators allowed, and a bare digit run (already an int — still a number).
fn yaml11_float_like(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    if matches!(t, ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let (mantissa, exp) = match t.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (t, None),
    };
    if let Some(e) = exp {
        let e = e.strip_prefix(['-', '+']).unwrap_or(e);
        if e.is_empty() || !e.chars().all(|c| c.is_ascii_digit() || c == '_') {
            return false;
        }
    }
    let parts: Vec<&str> = mantissa.split('.').collect();
    if parts.len() > 2 {
        return false;
    }
    let digits: String = parts.concat();
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '_')
}

/// 1.1 sexagesimal (`190:20:30`) — later groups are at most two digits, which
/// is also what keeps URLs (`http://h:8642/x` — non-digit groups) plain.
fn yaml11_sexagesimal(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    let parts: Vec<&str> = t.split(':').collect();
    parts.len() > 1
        && parts.iter().enumerate().all(|(i, p)| {
            !p.is_empty()
                && p.chars().all(|c| c.is_ascii_digit() || c == '_')
                && (i == 0 || p.len() <= 2)
        })
}

/// Can this string stay a PLAIN scalar — will every YAML 1.1 reader resolve it
/// back to the same string?
pub fn yaml11_plain_ok(s: &str) -> bool {
    if s.is_empty()
        || yaml11_bool_like(s)
        || yaml11_null_like(s)
        || yaml11_int_like(s)
        || yaml11_float_like(s)
        || yaml11_sexagesimal(s)
        || s == "="
        || s == "---"
        || s == "..."
    {
        return false;
    }
    if s.starts_with(' ') || s.ends_with(' ') {
        return false;
    }
    for (i, c) in s.char_indices() {
        if i == 0
            && matches!(
                c,
                ',' | '['
                    | ']'
                    | '{'
                    | '}'
                    | '#'
                    | '&'
                    | '*'
                    | '!'
                    | '|'
                    | '>'
                    | '\''
                    | '"'
                    | '%'
                    | '@'
                    | '`'
            )
        {
            return false;
        }
        match c {
            // ": " mid-string, or a trailing ':' — both end a plain scalar
            ':' if s[i + 1..].starts_with(' ') || i == s.len() - 1 => return false,
            // " #" starts a comment
            '#' if s[..i].ends_with(' ') => return false,
            // a leading "- " / "? " reads as a block indicator
            '-' | '?' if i == 0 && (s.len() == 1 || s[1..].starts_with(' ')) => return false,
            c if c.is_control() => return false,
            _ => {}
        }
    }
    true
}

/// Quote a non-plain string. Single quotes with `''` escaping is the readable
/// form; control characters go through serde_json (whose escape set — `\"`
/// `\\` `\n` `\t` `\u00XX` — is exactly YAML's double-quoted subset).
pub fn yaml11_quote(s: &str) -> String {
    if s.chars().any(|c| c.is_control()) {
        serde_json::to_string(s).unwrap_or_else(|_| format!("'{}'", s.replace('\'', "''")))
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

fn yaml11_key(k: &str) -> String {
    if yaml11_plain_ok(k) {
        k.to_string()
    } else {
        yaml11_quote(k)
    }
}

/// A scalar's rendered text, or None when the value is a collection.
fn yaml11_scalar(v: &Value) -> Option<String> {
    match v {
        Value::Null => Some("null".into()),
        Value::Bool(b) => Some(if *b { "true".into() } else { "false".into() }),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(if yaml11_plain_ok(s) {
            s.clone()
        } else {
            yaml11_quote(s)
        }),
        _ => None,
    }
}

fn yaml11_entry(k: &str, val: &Value, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    let key = yaml11_key(k);
    if let Some(s) = yaml11_scalar(val) {
        out.push_str(&format!("{pad}{key}: {s}\n"));
    } else if val.as_object().is_some_and(Map::is_empty) {
        out.push_str(&format!("{pad}{key}: {{}}\n"));
    } else if val.as_array().is_some_and(Vec::is_empty) {
        out.push_str(&format!("{pad}{key}: []\n"));
    } else if val.as_object().is_some() {
        out.push_str(&format!("{pad}{key}:\n"));
        for (k2, v2) in val.as_object().expect("just checked") {
            yaml11_entry(k2, v2, indent + 1, out);
        }
    } else {
        out.push_str(&format!("{pad}{key}:\n"));
        yaml11_seq(val.as_array().expect("just checked"), indent + 1, out);
    }
}

fn yaml11_seq(items: &[Value], indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    for item in items {
        if let Some(s) = yaml11_scalar(item) {
            out.push_str(&format!("{pad}- {s}\n"));
        } else if item.as_object().is_some_and(Map::is_empty) {
            out.push_str(&format!("{pad}- {{}}\n"));
        } else if item.as_array().is_some_and(Vec::is_empty) {
            out.push_str(&format!("{pad}- []\n"));
        } else if item.as_array().is_some() {
            // seq-in-seq has no block spelling that starts on the dash line
            // without chaining; JSON flow is valid YAML and always correct.
            out.push_str(&format!(
                "{pad}- {}\n",
                serde_json::to_string(item).unwrap_or_default()
            ));
        } else if let Some(m) = item.as_object() {
            // The map's first key rides the `- ` line (the dash eats two
            // columns); its later entries align under it at indent+1. A
            // COLLECTION under that first key continues one level deeper still,
            // so it clears the dash rather than aligning with it.
            let mut entries = m.iter();
            let Some((k1, v1)) = entries.next() else {
                continue;
            };
            let key1 = yaml11_key(k1);
            match yaml11_scalar(v1) {
                Some(s1) => out.push_str(&format!("{pad}- {key1}: {s1}\n")),
                None if v1.as_object().is_some_and(Map::is_empty) => {
                    out.push_str(&format!("{pad}- {key1}: {{}}\n"))
                }
                None if v1.as_array().is_some_and(Vec::is_empty) => {
                    out.push_str(&format!("{pad}- {key1}: []\n"))
                }
                None => {
                    out.push_str(&format!("{pad}- {key1}:\n"));
                    match v1 {
                        Value::Object(m2) => {
                            for (k2, v2) in m2 {
                                yaml11_entry(k2, v2, indent + 2, out);
                            }
                        }
                        Value::Array(a2) => yaml11_seq(a2, indent + 2, out),
                        _ => unreachable!("guarded by the None arms above"),
                    }
                }
            }
            for (k2, v2) in entries {
                yaml11_entry(k2, v2, indent + 1, out);
            }
        }
    }
}

/// Emit a config tree as YAML a 1.1 reader resolves identically. The root is
/// always a mapping (a scalar root renders as one bare line).
pub fn yaml11_emit(v: &Value) -> String {
    let mut out = String::new();
    if let Some(m) = v.as_object().filter(|m| !m.is_empty()) {
        for (k, val) in m {
            yaml11_entry(k, val, 0, &mut out);
        }
    } else if let Some(s) = yaml11_scalar(v) {
        out.push_str(&s);
        out.push('\n');
    }
    out
}
