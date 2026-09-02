// Gmail service: read the connected user's recent mail (metadata + snippets),
// read one full message, organize by label, and send mail, acting strictly as
// the connected identity (per-user OAuth, or the org account through an
// already-resolved token).
//
// The SEND half lives here — not in pending_actions — so the executor and the
// direct route run the same MIME-building code: two copies would drift on the
// one wire a sent email actually is.

use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;

use crate::gateway::provider::http;
use crate::google::connections::require_token;
use crate::google::errors::GoogleError;
use crate::secretbox::SecretBox;
use sqlx::PgPool;

const GMAIL_BASE: &str = "https://www.googleapis.com/gmail/v1/users/me";
const LABELS_ENDPOINT: &str = "https://www.googleapis.com/gmail/v1/users/me/labels";

// ── Labels (Gmail's folders) ─────────────────────────────────────────────────

/// One Gmail label — wire order pinned (id, name, type).
#[derive(Debug, Clone, Serialize)]
pub struct GmailLabel {
    pub id: String,
    pub name: String,
    /// 'system' | 'user'.
    #[serde(rename = "type")]
    pub kind: String,
}

/// Every label on the account. INBOX and UNREAD are system labels — a message
/// "is in a folder" by carrying its label, and organizing mail means applying
/// and removing them.
pub async fn list_labels_with_token(token: &str) -> Result<Vec<GmailLabel>, GoogleError> {
    let res = http()
        .get(format!("{LABELS_ENDPOINT}?fields=labels(id,name,type)"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail labels request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "gmail labels failed: {status} {text}"
        )));
    }
    let data: Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail labels body: {e}")))?;
    Ok(data
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|l| {
                    Some(GmailLabel {
                        id: l.get("id")?.as_str()?.to_string(),
                        name: l.get("name")?.as_str()?.to_string(),
                        kind: l.get("type")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Create a label, FIND-OR-CREATE: an existing label of the same name comes
/// back as-is, so a retry after a timeout is safe and the caller never ends
/// up with "Vendor" and "vendor " both on the account.
pub async fn create_label_with_token(token: &str, name: &str) -> Result<GmailLabel, GoogleError> {
    let existing = list_labels_with_token(token)
        .await?
        .into_iter()
        .find(|l| l.name == name);
    if let Some(existing) = existing {
        return Ok(existing);
    }
    let res = http()
        .post(LABELS_ENDPOINT)
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "name": name,
                "labelListVisibility": "labelShow",
                "labelVisibility": "labelShow",
            })
            .to_string(),
        )
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail label create request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "gmail label create failed: {status} {text}"
        )));
    }
    let label: Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail label create body: {e}")))?;
    Ok(GmailLabel {
        id: label
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        name: label
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        kind: label
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Labels an organize call may never touch, on either side. Deleting mail (or
/// hiding it as spam) is not "organizing" — the toolkit's whole safety story
/// is that everything it does is reversible, and nothing from TRASH comes back
/// on its own.
const FORBIDDEN_LABELS: [&str; 3] = ["TRASH", "SPAM", "BIN"];

/// OrganizeInput — names, not ids; the engine resolves them so an unknown
/// name is an error naming the fix (create_label), never a silent skip.
pub struct OrganizeInput<'a> {
    /// Message ids (from the listing tool), up to 100 per call.
    pub ids: &'a [String],
    /// Label NAMES to apply (from list_labels / create_label, or INBOX/UNREAD).
    pub add_labels: &'a [String],
    /// Label NAMES to remove — INBOX archives, UNREAD marks read.
    pub remove_labels: &'a [String],
}

/// The refusal a destructive label name draws — its own helper so the exact
/// sentence (which a route's error-classifier reads by prefix) is pinned by
/// a test next to the words it must start with.
fn forbidden_label_error(name: &str) -> Option<GoogleError> {
    FORBIDDEN_LABELS
        .contains(&name.to_uppercase().as_str())
        .then(|| {
            GoogleError::Failed(format!(
                "gmail organize: \"{name}\" would delete or hide mail — organizing never removes anything from All Mail"
            ))
        })
}

/// File/archive/read messages by applying and removing labels. Nothing here
/// can delete. Returns how many messages were touched.
///
/// The caller-facing errors that are NOT Google failures carry the
/// `gmail organize: ` prefix — the organize route answers those as a 400 the
/// agent can act on (it classifies by that prefix).
pub async fn organize_emails_with_token(
    token: &str,
    input: &OrganizeInput<'_>,
) -> Result<usize, GoogleError> {
    // deduped in first-occurrence order, capped at 100
    let mut ids: Vec<&str> = Vec::new();
    for id in input.ids {
        if !ids.contains(&id.as_str()) {
            ids.push(id);
        }
    }
    ids.truncate(100);
    if ids.is_empty() {
        return Err(GoogleError::Failed("gmail organize: no message ids".into()));
    }
    let dedup_cap = |names: &[String], cap: usize| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for n in names {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
        out.truncate(cap);
        out
    };
    let add = dedup_cap(input.add_labels, 10);
    let remove = dedup_cap(input.remove_labels, 10);
    for name in add.iter().chain(remove.iter()) {
        if let Some(err) = forbidden_label_error(name) {
            return Err(err);
        }
    }
    let labels = list_labels_with_token(token).await?;
    let by_name: std::collections::HashMap<&str, &str> = labels
        .iter()
        .map(|l| (l.name.as_str(), l.id.as_str()))
        .collect();
    let missing = |name: &str| -> GoogleError {
        GoogleError::Failed(format!(
            "gmail organize: no label named \"{name}\" — create it first (create_label), or spell it as list_labels shows"
        ))
    };
    // An unknown name is an error naming the fix, never a silent skip.
    let mut add_ids = Vec::with_capacity(add.len());
    for n in &add {
        match by_name.get(n.as_str()) {
            Some(id) => add_ids.push(*id),
            None => return Err(missing(n)),
        }
    }
    let mut remove_ids = Vec::with_capacity(remove.len());
    for n in &remove {
        match by_name.get(n.as_str()) {
            Some(id) => remove_ids.push(*id),
            None => return Err(missing(n)),
        }
    }
    if add_ids.is_empty() && remove_ids.is_empty() {
        return Err(GoogleError::Failed(
            "gmail organize: nothing to add or remove".into(),
        ));
    }

    // batchModify takes up to 1000 ids; cap the call at 100 (the tool's own
    // cap) so one agent turn cannot reorganize a mailbox wholesale by
    // accident.
    let res = http()
        .post(format!("{GMAIL_BASE}/messages/batchModify"))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "ids": ids,
                "addLabelIds": add_ids,
                "removeLabelIds": remove_ids,
            })
            .to_string(),
        )
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail organize request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "gmail organize failed: {status} {text}"
        )));
    }
    Ok(ids.len())
}

/// labelId → name, one labels read for however many ids are asked about.
async fn label_name_map(
    token: &str,
    label_ids: &[String],
) -> Result<std::collections::HashMap<String, String>, GoogleError> {
    let unique: std::collections::HashSet<&str> = label_ids.iter().map(String::as_str).collect();
    if unique.is_empty() {
        return Ok(Default::default());
    }
    let labels = list_labels_with_token(token).await?;
    Ok(labels.into_iter().map(|l| (l.id, l.name)).collect())
}

// ── The listing ──────────────────────────────────────────────────────────────

/// One summary row — wire order pinned.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailSummary {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub subject: String,
    pub snippet: String,
    pub date: Option<String>,
    pub unread: bool,
    /// Label names the message carries (INBOX, UNREAD, and user labels) —
    /// what an organizing agent needs to see before it files anything.
    pub labels: Vec<String>,
}

fn header_of<'a>(m: &'a Value, name: &str) -> &'a str {
    m.get("payload")
        .and_then(|p| p.get("headers"))
        .and_then(Value::as_array)
        .and_then(|hs| {
            hs.iter()
                .find(|h| {
                    h.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                })
                .and_then(|h| h.get("value"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
}

/// The date: internalDate (epoch-ms string) → ISO, else the Date header
/// verbatim, else null.
fn message_date(m: &Value) -> Option<String> {
    if let Some(internal) = m.get("internalDate").and_then(Value::as_str)
        && let Ok(ms) = internal.parse::<i64>()
    {
        return Some(crate::agent_auth::epoch_ms_to_iso(ms));
    }
    let h = header_of(m, "Date");
    (!h.is_empty()).then(|| h.to_string())
}

fn label_ids_of(m: &Value) -> Vec<String> {
    m.get("labelIds")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Recent messages (metadata only), newest first, using an already-resolved
/// token (per-user or org). `q` is Gmail search syntax.
pub async fn list_recent_messages_with_token(
    token: &str,
    max_results: usize,
    q: &str,
) -> Result<Vec<MailSummary>, GoogleError> {
    let auth_header = format!("Bearer {token}");

    // maxResults clamped to [1, 25], parameter order pinned (maxResults, q).
    let list_params = {
        let mut p = url::form_urlencoded::Serializer::new(String::new());
        p.append_pair("maxResults", &max_results.clamp(1, 25).to_string());
        p.append_pair("q", q);
        p.finish()
    };
    let list_res = http()
        .get(format!("{GMAIL_BASE}/messages?{list_params}"))
        .header("authorization", &auth_header)
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail list request: {e}")))?;
    if !list_res.status().is_success() {
        let status = list_res.status();
        let text = list_res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "gmail list failed: {status} {text}"
        )));
    }
    let list: Value = list_res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail list body: {e}")))?;
    let ids: Vec<String> = list
        .get("messages")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    // Fetch each message's metadata (headers + snippet) in parallel:
    // sequential round-trips would cost the listing N×RTT to Google (+700ms
    // at 25 messages) while preserving nothing — results arrive in the ids'
    // order either way.
    let msgs: Vec<Value> = {
        let futs = ids.iter().map(|id| {
            let auth_header = &auth_header;
            // metadataHeaders appended in From, Subject, Date order.
            // The serializer is finished INSIDE this scope — `Serializer` is not
            // Send, and a bare one held across the await below would make every
            // route awaiting this listing non-Send (Handler requires Send).
            let meta_params = {
                let mut p = url::form_urlencoded::Serializer::new(String::new());
                p.append_pair("format", "metadata");
                for h in ["From", "Subject", "Date"] {
                    p.append_pair("metadataHeaders", h);
                }
                p.finish()
            };
            async move {
                let r = http()
                    .get(format!("{GMAIL_BASE}/messages/{id}?{meta_params}"))
                    .header("authorization", auth_header)
                    .send()
                    .await;
                let Ok(r) = r else { return None };
                if !r.status().is_success() {
                    return None; // a failed per-message fetch is dropped
                }
                r.json::<Value>().await.ok()
            }
        });
        futures_util::future::join_all(futs)
            .await
            .into_iter()
            .flatten()
            .collect()
    };

    // One labels read for the whole listing, shared across messages — the
    // label map cannot change between two messages of the same fetch, so
    // asking for it once per message would be N identical calls.
    let all_label_ids: Vec<String> = msgs.iter().flat_map(label_ids_of).collect();
    let label_names = label_name_map(token, &all_label_ids).await?;

    Ok(msgs
        .iter()
        .map(|m| {
            let labels = label_ids_of(m);
            MailSummary {
                id: m
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                thread_id: m
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                from: header_of(m, "From").to_string(),
                subject: {
                    let s = header_of(m, "Subject");
                    if s.is_empty() {
                        "(no subject)".to_string()
                    } else {
                        s.to_string()
                    }
                },
                snippet: m
                    .get("snippet")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                date: message_date(m),
                unread: labels.iter().any(|l| l == "UNREAD"),
                labels: labels
                    .iter()
                    .map(|id| label_names.get(id).cloned().unwrap_or_else(|| id.clone()))
                    .collect(),
            }
        })
        .collect())
}

/// Recent messages as the connected user.
pub async fn list_recent_messages(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
    max_results: usize,
    q: &str,
) -> Result<Vec<MailSummary>, GoogleError> {
    let token = require_token(pg, sb, user_id, now_ms).await?;
    list_recent_messages_with_token(&token, max_results, q).await
}

// ── One full message ─────────────────────────────────────────────────────────

/// Find a part of a given text mime type: the part ITSELF first, then its
/// children — self-first, not deepest-first (an ancestor match wins over a
/// deeper one).
fn text_part_of(part: &Value, mime: &str) -> Option<String> {
    if part.get("mimeType").and_then(Value::as_str) == Some(mime)
        && part
            .get("body")
            .and_then(|b| b.get("data"))
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    {
        return part
            .get("body")
            .and_then(|b| b.get("data"))
            .and_then(Value::as_str)
            .map(decode_base64url);
    }
    for child in part
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(found) = text_part_of(child, mime) {
            return Some(found);
        }
    }
    None
}

fn decode_base64url(b64url: &str) -> String {
    // base64url bodies may lack padding; add what's
    // missing rather than fail a body over it.
    let padded = match b64url.len() % 4 {
        2 => format!("{b64url}=="),
        3 => format!("{b64url}="),
        _ => b64url.to_string(),
    };
    base64::engine::general_purpose::URL_SAFE
        .decode(padded.as_bytes())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

/// HTML → readable text, hand-rolled (no dependency): an agent reading a
/// message needs the words, not a browser-grade rendering. Entity order
/// matters — &amp; last, so a literal "&amp;lt;" cannot decay into "<".
fn html_to_text(html: &str) -> String {
    // 1. Whole elements whose content is markup, not words: script, style,
    //    head, title — dropped WITH their content. An opening tag pairs with
    //    the FIRST close of the SAME name (the four names are fixed, so each
    //    gets its own close pattern).
    static OPEN_ELEMENT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)<(script|style|head|title)\b[^>]*>").unwrap()
    });
    static CLOSE_OF: std::sync::LazyLock<std::collections::HashMap<&'static str, regex::Regex>> =
        std::sync::LazyLock::new(|| {
            ["script", "style", "head", "title"]
                .into_iter()
                .map(|name| {
                    (
                        name,
                        regex::Regex::new(&format!(r"(?i)</{name}\s*>")).unwrap(),
                    )
                })
                .collect()
        });

    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        let rest = &html[i..];
        if let Some(open) = OPEN_ELEMENT.captures(rest)
            && open.get(0).unwrap().start() == 0
        {
            let name = open.get(1).unwrap().as_str();
            let close = CLOSE_OF
                .get(name)
                .and_then(|re| re.find(rest))
                .map(|m| m.end())
                // No close at all → the element runs to the end (dropping
                // the tail is the same nothing-left-to-read).
                .unwrap_or(rest.len());
            i += close;
        } else {
            // Advance one char (byte-wise is fine: '<' is ASCII).
            let next = (i + 1..=html.len())
                .find(|j| html.is_char_boundary(*j))
                .unwrap_or(html.len());
            out.push_str(&html[i..next]);
            i = next;
        }
    }

    // 2. Structural tags → newlines, list markers, everything else stripped.
    static BR: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)<br\s*/?>").unwrap());
    static BLOCK_CLOSE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)</(p|div|tr|table|h[1-6]|li|blockquote)\s*>").unwrap()
    });
    static LI_OPEN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)<li\b[^>]*>").unwrap());
    static ANY_TAG: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"<[^>]+>").unwrap());
    static NBSP: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)&nbsp;").unwrap());
    static LT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)&lt;").unwrap());
    static GT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)&gt;").unwrap());
    static QUOT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)&quot;").unwrap());
    static SQUOT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)&#39;").unwrap());
    static AMP: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)&amp;").unwrap());
    static TRAIL_SPACES: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"[ \t]+\n").unwrap());
    static BLANK_RUNS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\n{3,}").unwrap());

    let s = BR.replace_all(&out, "\n");
    let s = BLOCK_CLOSE.replace_all(&s, "\n");
    let s = LI_OPEN.replace_all(&s, "• ");
    let s = ANY_TAG.replace_all(&s, "");
    let s = NBSP.replace_all(&s, " ");
    let s = LT.replace_all(&s, "<");
    let s = GT.replace_all(&s, ">");
    let s = QUOT.replace_all(&s, "\"");
    let s = SQUOT.replace_all(&s, "'");
    let s = AMP.replace_all(&s, "&");
    let s = TRAIL_SPACES.replace_all(&s, "\n");
    let s = BLANK_RUNS.replace_all(&s, "\n\n");
    s.trim().to_string()
}

/// The message's own words. text/plain when Google serves one (multipart/
/// alternative carries the same body without markup — the version to quote),
/// else the html stripped to text. Html-ONLY mail is common — most
/// transactional and marketing senders ship no plain part — and returning
/// empty there was a real read_email failure in the field (body empty,
/// snippet present).
fn body_text_of(part: &Value) -> String {
    if let Some(plain) = text_part_of(part, "text/plain") {
        return plain;
    }
    text_part_of(part, "text/html")
        .map(|html| html_to_text(&html))
        .unwrap_or_default()
}

/// One full message — wire order pinned.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailMessage {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub snippet: String,
    pub date: Option<String>,
    pub unread: bool,
    pub labels: Vec<String>,
    /// The plain-text body — the html stripped to text when the mail ships no
    /// plain part. Empty only when Google serves neither; snippet then.
    pub body: String,
}

/// One full message (headers + plain-text body) using an already-resolved
/// token. Body is capped — a mailing-list monster must not eat the agent's
/// context window whole.
pub async fn get_message_with_token(token: &str, id: &str) -> Result<MailMessage, GoogleError> {
    let res = http()
        .get(format!(
            "{GMAIL_BASE}/messages/{}?format=full",
            crate::google::oauth::encode_uri_component(id)
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail get request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "gmail get failed: {status} {text}"
        )));
    }
    let m: Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail get body: {e}")))?;
    let payload = m.get("payload").cloned().unwrap_or(Value::Null);
    let header = |name: &str| {
        payload
            .get("headers")
            .and_then(Value::as_array)
            .and_then(|hs| {
                hs.iter().find(|h| {
                    h.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                })
            })
            .and_then(|h| h.get("value"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let labels = label_ids_of(&m);
    let label_names = label_name_map(token, &labels).await?;
    let subject = {
        let s = header("Subject");
        if s.is_empty() {
            "(no subject)".to_string()
        } else {
            s
        }
    };
    Ok(MailMessage {
        id: m
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        thread_id: m
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        from: header("From"),
        to: header("To"),
        subject,
        snippet: m
            .get("snippet")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        date: message_date(&m),
        unread: labels.iter().any(|l| l == "UNREAD"),
        labels: labels
            .iter()
            .map(|id| label_names.get(id).cloned().unwrap_or_else(|| id.clone()))
            .collect(),
        // the 20_000 cut is in UTF-16 code units (JS string length), not
        // bytes or chars.
        body: crate::body::utf16_substr(&body_text_of(&payload), 0, 20_000).to_string(),
    })
}

// ── Send ─────────────────────────────────────────────────────────────────────

/// SendInput — the route body's five fields, defaults already applied.
pub struct SendInput<'a> {
    pub to: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
    pub cc: Option<&'a str>,
    pub bcc: Option<&'a str>,
}

/// RFC 2047 encode a header value if it has non-ASCII characters.
fn encode_header(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(value);
    format!("=?UTF-8?B?{b64}?=")
}

/// Send a plain-text email as the connected user.
/// Returns the sent message id and thread id.
pub async fn send_message(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
    input: &SendInput<'_>,
) -> Result<(String, String), GoogleError> {
    let token = require_token(pg, sb, user_id, now_ms).await?;
    send_message_with_token(&token, input, None).await
}

/// Send using an already-resolved token (per-user or org). `from` sets a
/// verified send-as alias on the account; omit to send from the account's own
/// address. Headers in fixed order (To, From?, Cc?, Bcc?, Subject,
/// MIME-Version, Content-Type, Content-Transfer-Encoding), `\r\n` joined,
/// base64url raw.
pub async fn send_message_with_token(
    token: &str,
    input: &SendInput<'_>,
    from: Option<&str>,
) -> Result<(String, String), GoogleError> {
    let mut headers = vec![format!("To: {}", input.to)];
    if let Some(from) = from {
        headers.push(format!("From: {from}"));
    }
    if let Some(cc) = input.cc.filter(|s| !s.is_empty()) {
        headers.push(format!("Cc: {cc}"));
    }
    if let Some(bcc) = input.bcc.filter(|s| !s.is_empty()) {
        headers.push(format!("Bcc: {bcc}"));
    }
    headers.push(format!("Subject: {}", encode_header(input.subject)));
    headers.push("MIME-Version: 1.0".to_string());
    headers.push("Content-Type: text/plain; charset=\"UTF-8\"".to_string());
    headers.push("Content-Transfer-Encoding: 8bit".to_string());
    // Headers, a blank separator line, then the body.
    let mime = format!("{}\r\n\r\n{}", headers.join("\r\n"), input.body);
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mime.as_bytes());

    let res = http()
        .post(format!("{GMAIL_BASE}/messages/send"))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(serde_json::json!({ "raw": raw }).to_string())
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail send request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "gmail send failed: {status} {text}"
        )));
    }
    let sent: Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("gmail send body: {e}")))?;
    Ok((
        sent.get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        sent.get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_matches_the_ts_ladder() {
        // Structural tags, entities in their order, whitespace squeezed.
        // `</p>` and `<br/>` BOTH become \n, and only runs of 3+ collapse.
        assert_eq!(
            html_to_text("<p>Hello&nbsp;world</p>\n<br/>next line"),
            "Hello world\n\nnext line"
        );
        // `</ul>` is not in the close-tag list, so the bullets run together.
        assert_eq!(html_to_text("<ul><li>one<li>two</ul>"), "• one• two");
        // &amp; LAST: a literal "&amp;lt;" stays "&lt;".
        assert_eq!(html_to_text("&amp;lt;"), "&lt;");
        assert_eq!(html_to_text("&lt;b&gt;"), "<b>");
        assert_eq!(
            html_to_text("&quot;q&quot; and &#39;r&#39;"),
            "\"q\" and 'r'"
        );
        // Markup-only elements drop WITH their content.
        assert_eq!(
            html_to_text("a<style>x{color:red}</style>b<script>bad()</script>c"),
            "abc"
        );
        assert_eq!(html_to_text("<head><title>t</title></head>body"), "body");
        // Case-insensitive tags and entities; blank runs collapse.
        assert_eq!(html_to_text("A<BR>B</P>\n\n\n\nC &NBSP;D"), "A\nB\n\nC  D");
        // Trailing spaces before a newline vanish; ends trim.
        assert_eq!(html_to_text("  x  \n  y  "), "x\n  y");
        assert_eq!(html_to_text("   "), "");
    }

    #[test]
    fn plain_part_wins_over_html() {
        let msg = serde_json::json!({
            "payload": {
                "mimeType": "multipart/alternative",
                "parts": [
                    {"mimeType": "text/plain", "body": {"data": b64("the words")}},
                    {"mimeType": "text/html", "body": {"data": b64("<p>the <b>words</b></p>")}}
                ]
            }
        });
        let p = msg.get("payload").unwrap();
        assert_eq!(body_text_of(p), "the words");
        // Html-only mail (most transactional senders) still yields the words.
        let html_only = serde_json::json!({
            "payload": {"parts": [
                {"mimeType": "text/html", "body": {"data": b64("<html><body><p>Deal&nbsp;inside</p></body></html>")}}
            ]}
        });
        assert_eq!(
            body_text_of(html_only.get("payload").unwrap()),
            "Deal inside"
        );
        // Neither part → empty (snippet is the caller's fallback).
        assert_eq!(body_text_of(&serde_json::json!({})), "");
    }

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
    }

    #[test]
    fn rfc2047_only_when_non_ascii() {
        assert_eq!(encode_header("hello"), "hello");
        assert_eq!(encode_header("Réunion"), "=?UTF-8?B?UsOpdW5pb24=?=");
    }

    #[test]
    fn organize_refuses_destructive_labels_in_ts_words() {
        // Case-insensitive over the three destructive names; the message
        // carries the 'gmail organize: ' prefix the route classifies by.
        for name in ["TRASH", "spam", "Bin"] {
            let err = forbidden_label_error(name).expect("refused");
            let msg = match err {
                GoogleError::Failed(m) => m,
                GoogleError::NotConnected => unreachable!(),
            };
            assert!(msg.starts_with("gmail organize: "), "{msg}");
            assert!(msg.contains("would delete or hide mail"), "{msg}");
        }
        assert!(forbidden_label_error("INBOX").is_none());
        assert!(forbidden_label_error("UNREAD").is_none());
    }
}
