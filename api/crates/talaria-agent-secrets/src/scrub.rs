//! Taking secrets out of text Talaria did not write — a container's log line,
//! an exception, a fault — before it is stored, shown, mailed or logged.

use crate::config_seal::credential_value;

/// What a redacted secret reads as.
pub const REDACTED: &str = "‹redacted›";

/// `text` with every secret Talaria knows (`secrets`: each value the agents
/// were given — any length from 3 — and its URL-encoded and base64 forms)
/// and every credential-shaped token taken out: a credential-named key's
/// value (`token=…`, `Authorization: …`), what the shared detector calls a
/// credential (URL userinfo and query tokens, provider prefixes, JWTs), a
/// token in a URL path (`/bot<token>/`, `/webhooks/<id>/<token>`, a long
/// mixed segment), and a long base64 blob.
pub fn scrub_text(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    let mut known: Vec<String> = Vec::new();
    for s in secrets
        .iter()
        .map(|s| s.trim())
        .filter(|s| s.chars().count() >= 3)
    {
        known.push(s.to_string());
        known.push(url_encode(s, false));
        known.push(url_encode(s, true));
        for b in [base64(s.as_bytes(), false), base64(s.as_bytes(), true)] {
            known.push(b.trim_end_matches('=').to_string());
        }
    }
    known.sort_by_key(|k| std::cmp::Reverse(k.len()));
    known.dedup();
    for k in known.iter().filter(|k| k.chars().count() >= 3) {
        if out.contains(k.as_str()) {
            out = out.replace(k.as_str(), REDACTED);
        }
    }
    let rules = rules();
    for re in rules {
        out = re
            .replace_all(&out, |c: &regex::Captures| format!("{}{REDACTED}", &c[1]))
            .into_owned();
    }
    // A long base64 blob mixing upper, lower and digits (never a hex digest,
    // which has no upper case).
    static BLOB: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let blob =
        BLOB.get_or_init(|| regex::Regex::new(r"[A-Za-z0-9+/_-]{32,}={0,2}").expect("compiles"));
    out = blob
        .replace_all(&out, |c: &regex::Captures| {
            let t = &c[0];
            let mixed = t.chars().any(|c| c.is_ascii_uppercase())
                && t.chars().any(|c| c.is_ascii_lowercase())
                && t.chars().any(|c| c.is_ascii_digit());
            if mixed {
                REDACTED.to_string()
            } else {
                t.to_string()
            }
        })
        .into_owned();
    out.split_inclusive(char::is_whitespace)
        .map(|word| {
            let bare = word
                .trim_end()
                .trim_matches(|c: char| "\"'(),;[]{}<>".contains(c));
            if bare.len() >= 8 && !bare.contains(REDACTED) && credential_value(bare) {
                word.replacen(bare, REDACTED, 1)
            } else {
                word.to_string()
            }
        })
        .collect()
}

/// Each rule keeps its first group (a key, a path prefix) and redacts the rest.
fn rules() -> &'static [regex::Regex] {
    static RULES: std::sync::OnceLock<Vec<regex::Regex>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        let r = |p: &str| regex::Regex::new(p).expect("a scrub rule compiles");
        vec![
            // key=value, key: value, "key": "value" for a credential-named key.
            r(r#"(?i)((?:[a-z0-9_-]*(?:api[_-]?key|apikey|token|secret|password|passwd|credential|cookie|auth[a-z_-]*|bearer|private[_-]?key|access[_-]?key)[a-z0-9_-]*)"?\s*[:=]\s*"?(?:bearer\s+|basic\s+)?)[^\s",;}]+"#),
            // Tokens in URL paths: Telegram, Discord and Slack webhooks.
            r(r"(?i)(/bot)\d+:[A-Za-z0-9_-]{20,}"),
            r(r"(?i)(/webhooks/\d+/)[A-Za-z0-9_-]{20,}"),
            r(r"(?i)(hooks\.slack\.com/services/)[A-Za-z0-9/]+"),
            // A long path segment mixing upper, lower and digits.
            r(r"(/)[A-Za-z0-9_-]*(?:[A-Z][A-Za-z0-9_-]*[0-9]|[0-9][A-Za-z0-9_-]*[A-Z])[A-Za-z0-9_-]{20,}"),
        ]
    })
}

/// Percent-encoded as a URL query would carry it (`all`: every byte but
/// letters and digits escaped, as some encoders do).
fn url_encode(s: &str, all: bool) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => (b as char).to_string(),
            b'-' | b'_' | b'.' | b'~' if !all => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn base64(bytes: &[u8], url: bool) -> String {
    let table: &[u8] = if url {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
    } else {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    };
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(table[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_planted_secret_never_survives_the_scrub_in_any_form() {
        let secrets = vec!["s3cr".to_string(), "tlk_9f8e7d6c5b4a3f2e1d0c".to_string()];
        for raw in [
            "boot: key=s3cr loaded",
            "calling with tlk_9f8e7d6c5b4a3f2e1d0c",
            "url https://x.example/cb?state=1&x=tlk%5F9f8e7d6c5b4a3f2e1d0c",
            "echo dGxrXzlmOGU3ZDZjNWI0YTNmMmUxZDBj seen",
            "Authorization: Bearer abcdefghij0123456789",
            "POST https://api.telegram.org/bot123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw/sendMessage",
            "https://discord.com/api/webhooks/1234567890/Zk3aXbD9qWerTy7uIoPlKjHgFdSa1234zxcv",
            "token: ghp_abcdefghijklmnopqrstuvwxyz12",
            "\"api_key\": \"sk-proj-abcdefghij1234\"",
        ] {
            let clean = scrub_text(raw, &secrets);
            for leak in [
                "s3cr",
                "tlk_9f8e7d6c5b4a3f2e1d0c",
                "tlk%5F9f8e7d6c5b4a3f2e1d0c",
                "dGxrXzlmOGU3ZDZjNWI0YTNmMmUxZDBj",
                "abcdefghij0123456789",
                "AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw",
                "Zk3aXbD9qWerTy7uIoPlKjHgFdSa1234zxcv",
                "ghp_abcdefghijklmnopqrstuvwxyz12",
                "sk-proj-abcdefghij1234",
            ] {
                assert!(!clean.contains(leak), "{raw} → {clean}");
            }
            assert!(clean.contains(REDACTED), "{raw} → {clean}");
        }
    }

    #[test]
    fn ordinary_text_is_left_as_it_is() {
        for keep in [
            "the recreated container stopped running before it reported healthy",
            "image sha256:7a69201726bc64db2072a1371557137c23312d29705cda2b8ea2a0214a50a4cf not found",
            "config migration to v52 failed and was skipped: KeyError: 'model'",
            "listening on http://0.0.0.0:8642/v1/models (agent 3f2b1c9e-4d5a-4b6c-8e7f-0a1b2c3d4e5f)",
        ] {
            assert_eq!(scrub_text(keep, &[]), keep);
        }
    }
}
