// CREDENTIALS DO NOT GO INTO A MODEL'S CONTEXT. A credential in outbound
// context is replaced by an opaque handle («secret:N») before the request
// leaves the process; the model reasons about the handle, and only the
// boundary that USES the value ever sees it. This is an input-side complement
// to the guard's output check: redaction cleans what we keep, sealing
// prevents what we send.
//
// One request's vault lives and dies with the call — no store behind it.

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

// JS \b is an ASCII word boundary; the regex crate's is Unicode-aware. A
// credential jammed against a non-ASCII word char (é) must still seal, so
// the boundary stays ASCII via (?-u:).
const B: &str = r"(?-u:\b)";

struct Pattern {
    label: &'static str,
    re: &'static str,
    /// Some patterns redact a WIDER span than they seal (the PEM block). The
    /// sealer uses this in place of `re` when present.
    redact: Option<&'static str>,
}

/// THE CREDENTIAL SHAPES — one list (order included: `sk-ant-…` before the
/// looser `sk-…`, so the label a human sees is decided by whichever rule
/// fires first).
const PATTERNS: &[Pattern] = &[
    Pattern {
        label: "Anthropic key",
        re: r"\bsk-ant-[A-Za-z0-9_-]{20,}\b",
        redact: None,
    },
    Pattern {
        label: "OpenAI key",
        re: r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}\b",
        redact: None,
    },
    // Stripe live keys only — test keys appear in every tutorial and are not
    // a live credential.
    Pattern {
        label: "Stripe secret key",
        re: r"\b[sr]k_live_[A-Za-z0-9]{16,}\b",
        redact: None,
    },
    Pattern {
        label: "AWS access key",
        re: r"\bAKIA[0-9A-Z]{16}\b",
        redact: None,
    },
    // Google keys are a fixed 39 characters; the exact count is the precision.
    Pattern {
        label: "Google API key",
        re: r"\bAIza[0-9A-Za-z_-]{35}\b",
        redact: None,
    },
    Pattern {
        label: "Slack token",
        re: r"\bxox[baprs]-[0-9A-Za-z-]{10,}\b",
        redact: None,
    },
    Pattern {
        label: "Slack app token",
        re: r"\bxapp-[0-9A-Za-z-]{10,}\b",
        redact: None,
    },
    Pattern {
        label: "GitHub token",
        re: r"\bgh[pousr]_[A-Za-z0-9]{36,}\b",
        redact: None,
    },
    // Fine-grained PAT — the exact shape the workbench hands a dev agent in
    // PAT mode, i.e. the credential an agent is most likely to echo back.
    Pattern {
        label: "GitHub fine-grained token",
        re: r"\bgithub_pat_[A-Za-z0-9_]{30,}\b",
        redact: None,
    },
    Pattern {
        label: "Talaria gateway key",
        re: r"\btlk_[a-f0-9]{40,}\b",
        redact: None,
    },
    Pattern {
        label: "Talaria agent credential",
        re: r"\btak_[a-f0-9]{40,}\b",
        redact: None,
    },
    // user:password@host in any URI. Requires BOTH a userinfo colon and an
    // '@', so ordinary links (https://host:8443/path, ssh://user@host) don't
    // match. Case-insensitive via an inline (?i).
    Pattern {
        label: "Credentials in URL",
        re: r"(?i)\b[a-z][a-z0-9+.-]*://[^\s:/?#@]*:[^\s/?#@]+@[^\s/?#]+",
        redact: None,
    },
    Pattern {
        label: "Private key block",
        // THE LINE BREAK AFTER THE HEADER IS LOAD-BEARING — PEM puts the body
        // on its own line; prose never does ("look for the -----BEGIN PRIVATE
        // KEY----- line in the bundle"). Without it one such SENTENCE would
        // swallow everything after it.
        re: r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----[ \t]*\r?\n",
        // …but redaction swallows the whole block, or to end-of-text when the
        // key arrives truncated mid-stream.
        redact: Some(
            r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----[ \t]*\r?\n[\s\S]*?(?:-----END (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----|$)",
        ),
    },
];

pub struct Compiled {
    pub label: &'static str,
    pub re: Regex,
    pub redact: Option<Regex>,
}

pub fn compiled() -> &'static Vec<Compiled> {
    static C: OnceLock<Vec<Compiled>> = OnceLock::new();
    C.get_or_init(|| {
        PATTERNS
            .iter()
            .map(|p| Compiled {
                label: p.label,
                re: Regex::new(&p.re.replace("\\b", B)).unwrap(),
                redact: p.redact.map(|r| Regex::new(&r.replace("\\b", B)).unwrap()),
            })
            .collect()
    })
}

fn handle_re() -> &'static Regex {
    static H: OnceLock<Regex> = OnceLock::new();
    H.get_or_init(|| Regex::new("«secret:(\\d+)»").unwrap())
}

#[derive(Debug, Clone)]
pub struct SealedSecret {
    pub handle: String,
    /// What KIND it is — the pattern's own label. Safe to log and to show a
    /// human; it names the shape, never the value.
    pub label: &'static str,
}

/// One request's substitutions.
#[derive(Debug, Default)]
pub struct SecretVault {
    /// handle -> the real value. The only place it exists outside the caller.
    values: HashMap<String, String>,
    /// What was sealed, for the audit line. Labels only.
    pub sealed: Vec<SealedSecret>,
}

impl SecretVault {
    /// Put the real values back. Called ONLY at the boundary where the value
    /// is used — never on anything that goes back to a model or into a
    /// record. A handle this vault does not know is left alone: a model
    /// inventing `«secret:9»` must not resolve to anything, and a handle from
    /// another request must not resolve here.
    pub fn unseal_text(&self, text: &str) -> String {
        handle_re()
            .replace_all(text, |c: &regex::Captures| {
                let m = c.get(0).map(|g| g.as_str()).unwrap_or_default();
                self.values
                    .get(m)
                    .map(String::as_str)
                    .unwrap_or(m)
                    .to_string()
            })
            .into_owned()
    }

    /// Handles the model produced but were never handed — guessing at a
    /// credential it cannot see.
    pub fn invented_handles(&self, text: &str) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for c in handle_re().captures_iter(text) {
            let h = c.get(0).unwrap().as_str().to_string();
            if !self.values.contains_key(&h) && !seen.contains(&h) {
                seen.push(h);
            }
        }
        seen
    }
}

/// Replace every credential in `text` with a handle, recording the
/// substitution. IDEMPOTENT AND STABLE WITHIN A VAULT: the same value seen
/// twice — in the system prompt and again in a tool result — gets the SAME
/// handle, so a model reading both sees one consistent thing.
pub fn seal_text(text: &str, vault: &mut SecretVault) -> String {
    let mut out = text.to_string();
    for p in compiled() {
        let re = p.redact.as_ref().unwrap_or(&p.re);
        out = re
            .replace_all(&out, |c: &regex::Captures| {
                let m = c.get(0).map(|g| g.as_str()).unwrap_or_default();
                if let Some(handle) = vault
                    .values
                    .iter()
                    .find(|(_, v)| v.as_str() == m)
                    .map(|(h, _)| h.clone())
                {
                    return handle;
                }
                let handle = format!("«secret:{}»", vault.values.len() + 1);
                vault.values.insert(handle.clone(), m.to_string());
                vault.sealed.push(SealedSecret {
                    handle: handle.clone(),
                    label: p.label,
                });
                handle
            })
            .into_owned();
    }
    out
}

/// Seal one message's content WHATEVER shape it is. A `string` is the prose
/// turn; an image-carrying turn is an array of OpenAI parts, and its
/// `{type:'text'}` part is exactly as likely to hold a credential. Image/data
/// parts and anything unrecognized pass through untouched.
pub fn seal_content(content: &Value, vault: &mut SecretVault) -> Value {
    match content {
        Value::String(s) => Value::String(seal_text(s, vault)),
        Value::Array(parts) => Value::Array(
            parts
                .iter()
                .map(|part| {
                    let is_text = part.get("type").and_then(|t| t.as_str()) == Some("text")
                        && part.get("text").map(|t| t.is_string()).unwrap_or(false);
                    if !is_text {
                        return part.clone();
                    }
                    let sealed = seal_text(part["text"].as_str().unwrap_or_default(), vault);
                    let mut out = part.clone();
                    out["text"] = Value::String(sealed);
                    out
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seal(text: &str) -> (String, SecretVault) {
        let mut v = SecretVault::default();
        let out = seal_text(text, &mut v);
        (out, v)
    }

    #[test]
    fn anthropic_label_wins_over_openai() {
        // sk-ant-… satisfies both rules; the first pattern decides the label.
        let (_, v) = seal("here: sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(v.sealed.len(), 1);
        assert_eq!(v.sealed[0].label, "Anthropic key");
        let (_, v) = seal("here: sk-proj-aaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(v.sealed[0].label, "OpenAI key");
    }

    // A shape the GitHub rule actually matches: `ghp_` + ≥36 alphanumerics.
    const GH: &str = "ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn same_value_gets_the_same_handle() {
        let mut v = SecretVault::default();
        let out = seal_text(&format!("a: {GH}; b: {GH}"), &mut v);
        assert_eq!(v.values.len(), 1);
        assert_eq!(out.matches("«secret:1»").count(), 2);
        // And a different value gets the next number.
        let out2 = seal_text(&format!("c: {GH}"), &mut v);
        assert_eq!(out2, "c: «secret:1»");
    }

    #[test]
    fn url_credentials_match_plain_urls_do_not() {
        let (out, v) = seal("db at postgres://admin:hunter2@db.internal:5432/app");
        assert_eq!(v.sealed[0].label, "Credentials in URL");
        assert!(!out.contains("hunter2"));
        let (_, v) = seal("docs at https://docs.example.com:8443/page and ssh://jon@host");
        assert!(v.sealed.is_empty());
    }

    #[test]
    fn pem_sentence_is_prose_not_a_key_block() {
        // THE load-bearing line break: prose about the marker seals nothing.
        let (_, v) = seal("look for the -----BEGIN PRIVATE KEY----- line in the bundle");
        assert!(v.sealed.is_empty());
        // A real block seals as one secret, whole.
        let key = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow...\n-----END RSA PRIVATE KEY-----\n";
        let (out, v) = seal(key);
        assert_eq!(v.sealed.len(), 1);
        assert_eq!(v.sealed[0].label, "Private key block");
        assert!(!out.contains("MIIEow"));
        // Unterminated (truncated mid-stream): swallow to end-of-text.
        let (out, _) = seal("-----BEGIN PRIVATE KEY-----\nMIIEow... truncated");
        assert_eq!(out, "«secret:1»");
    }

    #[test]
    fn content_parts_seal_text_but_not_images() {
        let mut v = SecretVault::default();
        let tak = format!("key: tak_{}", "a".repeat(44));
        let content = json!([
            {"type": "text", "text": tak},
            {"type": "image_url", "image_url": {"url": "https://x/img.png"}},
            {"type": "text", "text": "plain turn"},
        ]);
        let out = seal_content(&content, &mut v);
        assert_eq!(out[0]["text"], json!("key: «secret:1»"));
        assert_eq!(out[1], content[1]); // untouched, byte-for-byte
        assert_eq!(out[2]["text"], json!("plain turn"));
        assert_eq!(v.sealed[0].label, "Talaria agent credential");
        // A plain string content seals too.
        let out = seal_content(&json!("sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaa"), &mut v);
        assert_eq!(out, json!("«secret:2»"));
    }

    #[test]
    fn unseal_round_trips_and_invented_stay_away() {
        let mut v = SecretVault::default();
        let sealed = seal_text(&format!("token {GH} ok"), &mut v);
        assert_eq!(v.unseal_text(&sealed), format!("token {GH} ok"));
        // An invented handle resolves to nothing — left verbatim.
        assert_eq!(v.unseal_text("«secret:9»"), "«secret:9»");
        assert_eq!(v.invented_handles(&sealed), Vec::<String>::new());
        assert_eq!(
            v.invented_handles("«secret:9» and «secret:2»"),
            vec!["«secret:9»".to_string(), "«secret:2»".to_string()]
        );
    }
}
