//! Instance discovery: turn user input into an http(s) origin, then ask it
//! for the instance beacon (`GET /api/well-known/talaria-instance` →
//! `{ "instance": "<uuid>", "companyName": string|null }`, served by
//! api/src/routes/system/well_known_talaria_instance.rs). A valid beacon
//! uuid is what makes a URL "a Talaria instance" — not just something that
//! answers on 443.

use std::time::Duration;

use serde::Deserialize;
use url::Url;

pub const BEACON_PATH: &str = "/api/well-known/talaria-instance";

/// How long a probe may wait for the TCP (and TLS) connection. A black-holed
/// address answers nothing at all, so this is what bounds the wait there.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the whole probe may take once connected. The beacon is a trivial
/// read, but an instance behind a cold proxy or a busy single box can take
/// several seconds to answer — the old 8 s total refused a valid instance that
/// answered in 10. The add can be cancelled, so a generous bound costs nothing.
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Beacon {
    pub instance: String,
    pub company_name: Option<String>,
}

/// A normalized origin, and whether the person typed the scheme or the shell
/// assumed https (which decides whether a TLS failure earns an http hint).
#[derive(Debug, Clone)]
pub struct Origin {
    pub url: Url,
    pub scheme_assumed: bool,
}

/// Normalize user input into an http(s) origin. Bare hosts get https://
/// assumed; scheme must be http/https; paths, queries, fragments and
/// credentials are stripped — an instance IS its origin.
pub fn normalize_origin(input: &str) -> Result<Origin, String> {
    let input = input.trim();
    let scheme_assumed = !input.contains("://");
    let candidate = if scheme_assumed {
        format!("https://{input}")
    } else {
        input.to_string()
    };
    let mut url = Url::parse(&candidate).map_err(|_| {
        format!("“{input}” is not a web address. Enter one like https://talaria.example.com")
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "“{}” addresses are not supported — use http:// or https://",
            url.scheme()
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Addresses with a user name or password in them are not accepted".to_string());
    }
    if url.host_str().is_none() {
        return Err("That address has no host name".to_string());
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(Origin {
        url,
        scheme_assumed,
    })
}

/// The origin as a string, without the trailing slash the url crate adds —
/// registry URLs concatenate BEACON_PATH onto it.
pub fn origin_string(url: &Url) -> String {
    url.as_str().trim_end_matches('/').to_string()
}

/// Ask `origin` for its beacon. Every failure is a sentence a person can act
/// on: the transport error is classified (refused, unresolvable, timed out,
/// TLS) instead of reqwest's "error sending request for url (…)", which reads
/// the same for all of them.
pub async fn probe(client: &reqwest::Client, origin: &Origin) -> Result<Beacon, String> {
    let base = origin_string(&origin.url);
    let url = format!("{base}{BEACON_PATH}");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| unreachable(&base, origin, &e))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "{base} answered {status} at {BEACON_PATH} — it does not look like a Talaria instance"
        ));
    }
    let not_talaria =
        || format!("{base} answered, but not like a Talaria instance — check the address");
    let beacon: Beacon = response.json().await.map_err(|e| {
        if e.is_timeout() {
            unreachable(&base, origin, &e)
        } else {
            not_talaria()
        }
    })?;
    uuid::Uuid::parse_str(&beacon.instance).map_err(|_| not_talaria())?;
    Ok(beacon)
}

fn unreachable(base: &str, origin: &Origin, e: &reqwest::Error) -> String {
    let failure = classify(e);
    let mut text = format!("Could not reach {base}: {}", failure.reason(&origin.url));
    if failure == Failure::Tls && origin.scheme_assumed && origin.url.scheme() == "https" {
        let mut plain = origin.url.clone();
        let _ = plain.set_scheme("http");
        text.push_str(&format!(
            ". The shell assumed https — if it serves plain http, add {} instead",
            origin_string(&plain)
        ));
    }
    text
}

/// What went wrong on the wire, in the terms a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Refused,
    Unresolvable,
    ConnectTimeout,
    AnswerTimeout,
    Unreachable,
    Closed,
    Certificate(String),
    Tls,
    Other(String),
}

impl Failure {
    pub fn reason(&self, url: &Url) -> String {
        let host = url.host_str().unwrap_or("that host");
        match self {
            Self::Refused => "nothing is accepting connections there (connection refused)".into(),
            Self::Unresolvable => format!("the name “{host}” does not resolve to an address"),
            Self::ConnectTimeout => format!(
                "nothing answered within {} seconds — the address may be wrong, offline or firewalled",
                CONNECT_TIMEOUT.as_secs()
            ),
            Self::AnswerTimeout => format!(
                "it accepted the connection but did not answer within {} seconds",
                TOTAL_TIMEOUT.as_secs()
            ),
            Self::Unreachable => "the network or the host is unreachable from here".into(),
            Self::Closed => "the connection was closed before it answered".into(),
            Self::Certificate(detail) => format!("its certificate is not trusted ({detail})"),
            Self::Tls => "the secure (https) connection could not be set up".into(),
            Self::Other(detail) => detail.clone(),
        }
    }
}

/// Classify a reqwest transport error by walking its source chain. reqwest's
/// own Display stops at the outermost layer, which is the same sentence for a
/// refused port, a typo'd host and a dead network.
pub fn classify(e: &reqwest::Error) -> Failure {
    use std::error::Error as _;
    let mut io_kind = None;
    let mut texts = Vec::new();
    let mut source = e.source();
    while let Some(err) = source {
        if io_kind.is_none()
            && let Some(io) = err.downcast_ref::<std::io::Error>()
        {
            io_kind = Some(io.kind());
        }
        texts.push(err.to_string());
        source = err.source();
    }
    let all = texts.join(" | ").to_lowercase();
    let deepest = texts.last().cloned().unwrap_or_else(|| e.to_string());

    use std::io::ErrorKind as K;
    if e.is_timeout() || io_kind == Some(K::TimedOut) {
        return if e.is_connect() {
            Failure::ConnectTimeout
        } else {
            Failure::AnswerTimeout
        };
    }
    if all.contains("dns error") || all.contains("failed to lookup address") {
        return Failure::Unresolvable;
    }
    match io_kind {
        Some(K::ConnectionRefused) => return Failure::Refused,
        Some(K::NetworkUnreachable | K::HostUnreachable) => return Failure::Unreachable,
        _ => {}
    }
    if all.contains("certificate") {
        return Failure::Certificate(deepest);
    }
    if io_kind == Some(K::InvalidData)
        || all.contains("tls")
        || all.contains("handshake")
        || all.contains("corrupt message")
    {
        return Failure::Tls;
    }
    match io_kind {
        Some(K::ConnectionReset | K::ConnectionAborted | K::UnexpectedEof | K::BrokenPipe) => {
            Failure::Closed
        }
        _ if all.contains("connection closed") || all.contains("incomplete message") => {
            Failure::Closed
        }
        _ => Failure::Other(deepest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(input: &str) -> Origin {
        normalize_origin(input).unwrap()
    }

    #[test]
    fn bare_hosts_become_https() {
        let o = origin("outcrop.example");
        assert_eq!(origin_string(&o.url), "https://outcrop.example");
        assert!(o.scheme_assumed);
    }

    #[test]
    fn scheme_host_port_survive_but_nothing_else() {
        let o = origin("http://127.0.0.1:5302/some/path?x=1#frag");
        assert_eq!(origin_string(&o.url), "http://127.0.0.1:5302");
        assert!(!o.scheme_assumed);
    }

    #[test]
    fn non_http_schemes_are_refused() {
        assert!(normalize_origin("ftp://example.com").is_err());
        assert!(normalize_origin("file:///etc/passwd").is_err());
    }

    #[test]
    fn embedded_credentials_are_refused() {
        assert!(normalize_origin("https://user:pass@example.com").is_err());
    }

    #[test]
    fn garbage_is_refused_in_plain_words() {
        let err = normalize_origin("not a url at all").unwrap_err();
        assert!(err.contains("is not a web address"), "{err}");
        assert!(!err.contains("international"), "{err}");
        assert!(normalize_origin("https://").is_err());
    }

    /// One canned answer on an ephemeral port; returns its origin.
    fn serve_once(response: String) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    /// Accepts and then says nothing, for as long as the test runs.
    fn serve_silence() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept() {
                held.push(stream);
            }
        });
        format!("http://{addr}")
    }

    /// A port that was free a moment ago: nothing listens there.
    fn closed_port() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}")
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(Duration::from_millis(600))
            .build()
            .unwrap()
    }

    fn http(status_line: &str, body: &str) -> String {
        format!(
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn a_valid_beacon_parses() {
        let o = origin(&serve_once(http(
            "HTTP/1.1 200 OK",
            r#"{"instance":"11111111-1111-1111-1111-111111111111","companyName":"Outcrop"}"#,
        )));
        let beacon = probe(&client(), &o).await.unwrap();
        assert_eq!(beacon.company_name.as_deref(), Some("Outcrop"));
    }

    #[tokio::test]
    async fn company_name_may_be_null() {
        let o = origin(&serve_once(http(
            "HTTP/1.1 200 OK",
            r#"{"instance":"11111111-1111-1111-1111-111111111111","companyName":null}"#,
        )));
        let beacon = probe(&client(), &o).await.unwrap();
        assert!(beacon.company_name.is_none());
    }

    #[tokio::test]
    async fn non_talaria_answers_are_refused() {
        let o = origin(&serve_once(http(
            "HTTP/1.1 200 OK",
            r#"{"instance":"not-a-uuid"}"#,
        )));
        let err = probe(&client(), &o).await.unwrap_err();
        assert!(err.contains("not like a Talaria instance"), "{err}");
    }

    #[tokio::test]
    async fn an_html_page_is_not_talaria_not_a_decode_error() {
        let o = origin(&serve_once(http("HTTP/1.1 200 OK", "<!doctype html><p>hi")));
        let err = probe(&client(), &o).await.unwrap_err();
        assert!(err.contains("not like a Talaria instance"), "{err}");
        assert!(!err.contains("decoding"), "{err}");
    }

    #[tokio::test]
    async fn error_statuses_are_refused() {
        let o = origin(&serve_once(http("HTTP/1.1 404 Not Found", r#"{}"#)));
        assert!(probe(&client(), &o).await.is_err());
    }

    #[tokio::test]
    async fn a_closed_port_reads_refused() {
        let o = origin(&closed_port());
        let err = probe(&client(), &o).await.unwrap_err();
        assert!(err.contains("connection refused"), "{err}");
        assert!(!err.contains("error sending request"), "{err}");
    }

    #[tokio::test]
    async fn a_silent_server_reads_as_a_timeout() {
        let o = origin(&serve_silence());
        let err = probe(&client(), &o).await.unwrap_err();
        assert!(err.contains("did not answer within"), "{err}");
    }

    #[tokio::test]
    async fn tls_against_plain_http_suggests_http_for_a_bare_host() {
        // A bare host:port gets https assumed; the server speaks plain http.
        let plain = serve_once(http("HTTP/1.1 200 OK", "{}"));
        let bare = plain.trim_start_matches("http://").to_string();
        let o = origin(&bare);
        let err = probe(&client(), &o).await.unwrap_err();
        assert!(err.contains("secure (https) connection"), "{err}");
        assert!(err.contains(&format!("add {plain} instead")), "{err}");
    }

    #[tokio::test]
    async fn an_explicit_https_gets_no_http_hint() {
        let plain = serve_once(http("HTTP/1.1 200 OK", "{}"));
        let o = origin(&plain.replace("http://", "https://"));
        let err = probe(&client(), &o).await.unwrap_err();
        assert!(err.contains("secure (https) connection"), "{err}");
        assert!(!err.contains("instead"), "{err}");
    }
}
