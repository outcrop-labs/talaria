// Transactional email. Pluggable providers:
//   smtp    any SMTP server (Google Workspace, etc.)
//   resend  Resend's HTTP API
// Config lives in app_settings with secrets SEALED; the admin panel writes it
// and test-sends. send_email is the one entry point every feature uses
// (invites and notification routing today, more transactional mail later). No
// provider configured = sends fail soft with a clear error the caller can
// surface.
//
// send_email NEVER ERRS AND ALWAYS RETURNS. Both halves matter: most sends
// are a side effect of somebody else's request or of a queue drain, so a
// provider that is merely slow must become a reported failure rather than a
// caller that waits. tokio's timeout actually cancels the future, and the
// socket phases underneath (connect, greeting, mid-dialogue) are each
// bounded individually by the transport's stream timeout.

use std::time::Duration;

use lettre::message::header::Header;
use lettre::message::{Mailbox, Message, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use sqlx::PgPool;

use crate::gateway::provider::http;
use crate::gateway::settings::{get_setting, set_setting};
use crate::secretbox::SecretBox;

const KEY: &str = "email_config";

/// The outside bound of one send attempt, whole call, provider included.
pub const EMAIL_SEND_TIMEOUT_MS: u64 = 30_000;
/// The Resend HTTP request's own budget.
const RESEND_TIMEOUT_MS: u64 = 15_000;
/// The SMTP stream timeout — every socket phase (greeting included) is
/// bounded by this; the whole call is bounded again by the 30s above.
const SMTP_STREAM_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Smtp,
    Resend,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmailConfig {
    pub provider: Option<Provider>,
    /// From header, e.g. "Talaria <talaria@yourcompany.com>".
    pub from: String,
    pub smtp: SmtpConfig,
    pub resend: ResendConfig,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub secure: bool,
    pub user: String,
    pub pass_enc: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResendConfig {
    pub api_key_enc: Option<String>,
}

fn provider_of(v: &serde_json::Value) -> Option<Provider> {
    match v.as_str() {
        Some("smtp") => Some(Provider::Smtp),
        Some("resend") => Some(Provider::Resend),
        _ => None,
    }
}

/// The config, DEFAULTS filled out with whatever the row says. The
/// stored pass/apiKey ciphers ride along as-is (sealed strings); only the
/// send path ever opens them.
pub async fn get_email_config(pg: &PgPool) -> EmailConfig {
    config_from_stored(&get_setting(pg, KEY, serde_json::json!({})).await)
}

fn config_from_stored(stored: &serde_json::Value) -> EmailConfig {
    let str_of = |k: &str| {
        stored
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let smtp = stored.get("smtp").cloned().unwrap_or(serde_json::json!({}));
    let resend = stored
        .get("resend")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    EmailConfig {
        provider: stored.get("provider").and_then(provider_of),
        from: str_of("from"),
        smtp: SmtpConfig {
            host: smtp
                .get("host")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            port: smtp.get("port").and_then(|v| v.as_u64()).unwrap_or(587) as u16,
            secure: smtp
                .get("secure")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            user: smtp
                .get("user")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            pass_enc: smtp
                .get("passEnc")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        },
        resend: ResendConfig {
            api_key_enc: resend
                .get("apiKeyEnc")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        },
    }
}

/// The patch setEmailConfig accepts — every field optional, and the two
/// secrets tri-state: `None` = keep what's stored, `Some(None)` = clear,
/// `Some(Some(new))` = replace (sealed on the way in).
#[derive(Default)]
pub struct EmailConfigPatch {
    pub provider: Option<Option<Provider>>,
    pub from: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_secure: Option<bool>,
    pub smtp_user: Option<String>,
    pub smtp_pass: Option<Option<String>>,
    pub resend_api_key: Option<Option<String>>,
}

/// Errors as plain strings: the admin route surfaces them verbatim,
/// and a sealed-secret failure is a sentence for the admin, not a typed enum
/// arm nobody matches.
pub async fn set_email_config(
    pg: &PgPool,
    sb: &SecretBox,
    patch: &EmailConfigPatch,
) -> Result<(), String> {
    let cur = get_email_config(pg).await;
    let provider = match patch.provider {
        Some(Some(p)) => serde_json::json!(match p {
            Provider::Smtp => "smtp",
            Provider::Resend => "resend",
        }),
        Some(None) => serde_json::Value::Null,
        None => cur
            .provider
            .map(|p| match p {
                Provider::Smtp => serde_json::json!("smtp"),
                Provider::Resend => serde_json::json!("resend"),
            })
            .unwrap_or(serde_json::Value::Null),
    };
    // A seal failure stops the write before anything lands — nothing merges
    // half-sealed.
    let seal = |v: &str| sb.seal(v).map_err(|e| e.to_string());
    let pass_enc = match &patch.smtp_pass {
        None => cur.smtp.pass_enc.clone(),
        Some(None) => None,
        Some(Some(pass)) => Some(seal(pass)?),
    };
    let api_key_enc = match &patch.resend_api_key {
        None => cur.resend.api_key_enc.clone(),
        Some(None) => None,
        Some(Some(key)) => Some(seal(key)?),
    };
    let next = serde_json::json!({
        "provider": provider,
        "from": patch.from.clone().unwrap_or(cur.from),
        "smtp": {
            "host": patch.smtp_host.clone().unwrap_or(cur.smtp.host),
            "port": patch.smtp_port.unwrap_or(cur.smtp.port),
            "secure": patch.smtp_secure.unwrap_or(cur.smtp.secure),
            "user": patch.smtp_user.clone().unwrap_or(cur.smtp.user),
            "passEnc": pass_enc,
        },
        "resend": { "apiKeyEnc": api_key_enc },
    });
    set_setting(pg, KEY, &next).await.map_err(|e| e.to_string())
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmailInput {
    pub to: String,
    pub subject: String,
    pub html: String,
    pub text: Option<String>,
    /// Extra RFC 5322 headers. One reason it exists: `List-Unsubscribe`, which
    /// every bulk-ish mail we send needs — a mail client that can offer a
    /// one-tap unsubscribe is a mail client that does not offer "report spam"
    /// instead, and the reputation of the sending domain is the whole
    /// product's ability to reach anybody. Both providers below carry it
    /// verbatim.
    pub headers: Vec<(String, String)>,
}

impl EmailInput {
    pub fn new(to: &str, subject: &str, html: &str, text: Option<&str>) -> EmailInput {
        EmailInput {
            to: to.to_string(),
            subject: subject.to_string(),
            html: html.to_string(),
            text: text.map(str::to_string),
            headers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SendOutcome {
    Sent,
    Failed(String),
}

/// Send one transactional email through the configured provider. Always
/// returns — never errors, never panics up; see EMAIL_SEND_TIMEOUT_MS.
pub async fn send_email(pg: &PgPool, sb: &SecretBox, input: &EmailInput) -> SendOutcome {
    let cfg = get_email_config(pg).await;
    let Some(provider) = cfg.provider else {
        return SendOutcome::Failed("no email provider configured (Admin → Org → Email)".into());
    };
    if cfg.from.trim().is_empty() {
        return SendOutcome::Failed("no From address configured".into());
    }
    let what = match provider {
        Provider::Smtp => "smtp",
        Provider::Resend => "resend",
    };
    let work = send_via(&cfg, sb, provider, input);
    match tokio::time::timeout(Duration::from_millis(EMAIL_SEND_TIMEOUT_MS), work).await {
        Ok(outcome) => outcome,
        Err(_) => SendOutcome::Failed(format!(
            "{what} did not respond within {}s",
            EMAIL_SEND_TIMEOUT_MS / 1000
        )),
    }
}

async fn send_via(
    cfg: &EmailConfig,
    sb: &SecretBox,
    provider: Provider,
    input: &EmailInput,
) -> SendOutcome {
    match provider {
        Provider::Resend => send_via_resend(cfg, sb, input).await,
        Provider::Smtp => send_via_smtp(cfg, sb, input).await,
    }
}

/// `{from, to, subject, html, text, headers?}` — the shape Resend documents;
/// `text`/`headers` are omitted rather than null when absent (undefined keys
/// vanish in JSON — skip_serializing_if, not explicit null).
#[derive(serde::Serialize)]
struct ResendBody<'a> {
    from: &'a str,
    to: Vec<&'a str>,
    subject: &'a str,
    html: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    headers: Option<std::collections::BTreeMap<&'a str, &'a str>>,
}

async fn send_via_resend(cfg: &EmailConfig, sb: &SecretBox, input: &EmailInput) -> SendOutcome {
    let Some(key_enc) = cfg.resend.api_key_enc.as_deref() else {
        return SendOutcome::Failed("Resend API key missing".into());
    };
    // An unopenable key (rotated root, mangled token) is a config failure
    // worded as a send failure — the admin has to hear about it.
    let key = match sb.open(key_enc) {
        Ok(k) => k,
        Err(e) => return SendOutcome::Failed(format!("could not open the Resend API key: {e}")),
    };
    let mut headers = None;
    if !input.headers.is_empty() {
        let mut m = std::collections::BTreeMap::new();
        for (k, v) in &input.headers {
            m.insert(k.as_str(), v.as_str());
        }
        headers = Some(m);
    }
    let body = ResendBody {
        from: &cfg.from,
        to: vec![input.to.as_str()],
        subject: &input.subject,
        html: &input.html,
        text: input.text.as_deref(),
        headers,
    };
    let r = http()
        .post("https://api.resend.com/emails")
        .timeout(Duration::from_millis(RESEND_TIMEOUT_MS))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await;
    let resp = match r {
        Ok(r) => r,
        Err(e) => return SendOutcome::Failed(e.to_string()),
    };
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let message = resp.json::<serde_json::Value>().await.ok().and_then(|j| {
            j.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        });
        return SendOutcome::Failed(match message {
            Some(m) => format!("Resend: {m}"),
            None => format!("Resend: {status}"),
        });
    }
    SendOutcome::Sent
}

/// One transport per send, closed per send (a queue draining hundreds of
/// mails would otherwise hold a socket per message). lettre's builder keeps
/// reuse off and the transport's stream dies with it.
///
/// The `secure` flag: `false` is OPPORTUNISTIC STARTTLS — upgrade when the
/// server offers it, proceed plaintext when it does not — and `true` is TLS
/// from the first byte (implicit TLS, port 465). Note `Tls::Required`
/// (lettre's starttls_relay) would be the WRONG arm for `secure:false`: it
/// refuses to send to a server that never offers STARTTLS, which
/// opportunistic mode deliberately does.
async fn send_via_smtp(cfg: &EmailConfig, sb: &SecretBox, input: &EmailInput) -> SendOutcome {
    if cfg.smtp.host.is_empty() {
        return SendOutcome::Failed("SMTP host missing".into());
    }
    let pass = match cfg.smtp.pass_enc.as_deref() {
        Some(enc) => match sb.open(enc) {
            Ok(p) => p,
            Err(e) => return SendOutcome::Failed(format!("could not open the SMTP password: {e}")),
        },
        None => String::new(),
    };
    let msg = match build_smtp_message(cfg, input) {
        Ok(m) => m,
        Err(e) => return SendOutcome::Failed(e),
    };
    let mut builder =
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(cfg.smtp.host.clone())
            .port(cfg.smtp.port);
    let tls = match TlsParameters::new(cfg.smtp.host.clone()) {
        Ok(t) => t,
        Err(e) => return SendOutcome::Failed(e.to_string()),
    };
    builder = if cfg.smtp.secure {
        builder.tls(Tls::Wrapper(tls))
    } else {
        builder.tls(Tls::Opportunistic(tls))
    };
    if !cfg.smtp.user.is_empty() {
        builder = builder.credentials(Credentials::new(cfg.smtp.user.clone(), pass));
    }
    let transport = builder.timeout(Some(SMTP_STREAM_TIMEOUT)).build();
    // The async send takes the message by value (async_trait moves it into
    // the boxed future); one send per transport, nothing left to reuse.
    match transport.send(msg).await {
        Ok(_) => SendOutcome::Sent,
        Err(e) => SendOutcome::Failed(e.to_string()),
    }
}

/// `List-Unsubscribe` — the one custom header the product actually sends,
/// lifted to a lettre Header so it rides the SMTP transport verbatim (the
/// Resend branch carries it in the JSON body).
#[derive(Clone)]
struct ListUnsubscribe(String);

impl Header for ListUnsubscribe {
    fn name() -> lettre::message::header::HeaderName {
        lettre::message::header::HeaderName::new_from_ascii_str("List-Unsubscribe")
    }
    fn parse(s: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(ListUnsubscribe(s.to_owned()))
    }
    fn display(&self) -> lettre::message::header::HeaderValue {
        // HeaderValue::new does the RFC 2047 encoding and line folding a raw
        // value needs; a URL is ASCII so it rides as-is.
        lettre::message::header::HeaderValue::new(Self::name(), self.0.clone())
    }
}

fn build_smtp_message(cfg: &EmailConfig, input: &EmailInput) -> Result<Message, String> {
    let from: Mailbox = cfg
        .from
        .parse()
        .map_err(|e| format!("invalid From address: {e}"))?;
    let to: Mailbox = input
        .to
        .parse()
        .map_err(|e| format!("invalid To address: {e}"))?;
    let mut builder = Message::builder().from(from).to(to).subject(&input.subject);
    for (k, v) in &input.headers {
        // Only the unsubscription header shape is carried today; anything
        // else an email composer wants, it should add here explicitly rather
        // than through a stringly hole.
        if k.eq_ignore_ascii_case("list-unsubscribe") {
            builder = builder.header(ListUnsubscribe(v.clone()));
        }
    }
    let text = input
        .text
        .clone()
        .unwrap_or_else(|| plain_fallback(&input.html));
    // alternative_plain_html takes the raw bodies and wires the
    // multipart/alternative boundary + content types itself.
    let body = MultiPart::alternative_plain_html(text, input.html.clone());
    builder
        .multipart(body)
        .map_err(|e| format!("could not build the message: {e}"))
}

/// The text part when a caller supplied none: strip the tags off the HTML so
/// the mail is not empty in a text-only client. Crude by design — the
/// callers that care (notifications, digests) always pass a real text part.
fn plain_fallback(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Escape a string for interpolation into email HTML. Notification titles and
/// bodies are user- and agent-written text, and they reach a mail client that
/// will happily render whatever tags survive.
pub fn email_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The one call-to-action button, so every mail's primary link looks the same.
pub fn email_button(href: &str, label: &str) -> String {
    format!(
        "<p style=\"margin:24px 0\"><a href=\"{}\" style=\"background:#1a1a18;color:#ffffff;text-decoration:none;padding:10px 18px;border-radius:8px;font-size:14px\">{}</a></p>",
        email_escape(href),
        email_escape(label)
    )
}

/// The shared shell for transactional mail — quiet, light, no images. All
/// three parts arrive ALREADY-ESCAPED or pre-composed: callers escape the
/// title, and a footer carrying a link
/// (the notification mail's does) arrives as markup. This function neither
/// knows nor checks — double-escaping is how a footer full of &lt;a&gt;
/// happens.
pub fn email_shell(title: &str, body_html: &str, footer: &str) -> String {
    format!(
        "<!doctype html><html><body style=\"margin:0;padding:32px 16px;background:#f6f6f4;font-family:-apple-system,'Segoe UI',Roboto,Helvetica,Arial,sans-serif\">\n<div style=\"max-width:520px;margin:0 auto;background:#ffffff;border:1px solid #e5e5e0;border-radius:12px;padding:32px\">\n<h1 style=\"margin:0 0 16px;font-size:18px;color:#1a1a18\">{}</h1>\n<div style=\"font-size:14px;line-height:1.6;color:#3a3a36\">{}</div>\n</div>\n<p style=\"max-width:520px;margin:16px auto 0;font-size:11px;color:#8a8a84;text-align:center\">{}</p>\n</body></html>",
        title, body_html, footer
    )
}

/// The default footer — notification and digest callers share it.
pub const DEFAULT_FOOTER: &str = "Sent by Talaria";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn escape_hits_exactly_the_five_ts_characters() {
        assert_eq!(email_escape("a&<\">'b"), "a&amp;&lt;&quot;&gt;&#39;b");
        assert_eq!(email_escape("plain"), "plain");
        // The full set and nothing else — accented and CJK text passes as-is.
        assert_eq!(email_escape("café 中文"), "café 中文");
    }

    #[test]
    fn the_button_and_shell_reproduce_the_ts_bytes() {
        assert_eq!(
            email_button("https://x/y?a=1&b=2", "Open"),
            "<p style=\"margin:24px 0\"><a href=\"https://x/y?a=1&amp;b=2\" style=\"background:#1a1a18;color:#ffffff;text-decoration:none;padding:10px 18px;border-radius:8px;font-size:14px\">Open</a></p>"
        );
        let shell = email_shell(&email_escape("Hi <there>"), "<p>body</p>", DEFAULT_FOOTER);
        assert!(shell.starts_with(
            "<!doctype html><html><body style=\"margin:0;padding:32px 16px;background:#f6f6f4;"
        ));
        assert!(shell.contains(
            "<h1 style=\"margin:0 0 16px;font-size:18px;color:#1a1a18\">Hi &lt;there&gt;</h1>"
        ));
        assert!(shell.contains("<p style=\"max-width:520px;margin:16px auto 0;font-size:11px;color:#8a8a84;text-align:center\">Sent by Talaria</p>"));
        assert!(shell.ends_with("</body></html>"));
        // Escaping is the CALLER's job: raw title bytes ride raw, and a footer
        // carrying a link arrives as markup (the notification mail's does).
        assert!(email_shell("<b>", "raw<b>", "f").contains("<b>"));
        assert!(email_shell("t", "b", "<a href=\"x\">y</a>").contains("<a href=\"x\">y</a>"));
    }

    #[test]
    fn config_parsing_fills_defaults_and_keeps_ciphers() {
        let cfg = config_from_stored(&json!({
            "provider": "resend",
            "from": "Talaria <t@x.com>",
            "smtp": { "host": "smtp.x.com", "user": "u", "passEnc": "v1:aa:bb:cc" },
            "resend": { "apiKeyEnc": "v2:1:aa:bb:cc" },
        }));
        assert_eq!(cfg.provider, Some(Provider::Resend));
        assert_eq!(cfg.from, "Talaria <t@x.com>");
        assert_eq!(cfg.smtp.host, "smtp.x.com");
        assert_eq!(cfg.smtp.port, 587); // default survives a partial smtp blob
        assert!(!cfg.smtp.secure);
        assert_eq!(cfg.smtp.user, "u");
        assert_eq!(cfg.smtp.pass_enc.as_deref(), Some("v1:aa:bb:cc"));
        assert_eq!(cfg.resend.api_key_enc.as_deref(), Some("v2:1:aa:bb:cc"));

        let empty = config_from_stored(&json!({}));
        assert_eq!(empty.provider, None);
        assert_eq!(empty.from, "");
        assert_eq!(empty.smtp.host, "");
        assert_eq!(empty.smtp.port, 587);
        // A hand-mangled provider string is no provider, not a crash.
        assert_eq!(
            config_from_stored(&json!({ "provider": "fax" })).provider,
            None
        );
    }

    #[test]
    fn plain_fallback_strips_tags() {
        assert_eq!(plain_fallback("<p>hi <b>there</b></p>"), "hi there");
        assert_eq!(plain_fallback("no tags"), "no tags");
    }
}
