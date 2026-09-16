//! Instance discovery: turn user input into an http(s) origin, then ask it
//! for the instance beacon (`GET /api/well-known/talaria-instance` →
//! `{ "instance": "<uuid>", "companyName": string|null }`, served by
//! api/src/routes/system/well_known_talaria_instance.rs). A valid beacon
//! uuid is what makes a URL "a Talaria instance" — not just something that
//! answers on 443.

use serde::Deserialize;
use url::Url;

pub const BEACON_PATH: &str = "/api/well-known/talaria-instance";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Beacon {
    pub instance: String,
    pub company_name: Option<String>,
}

/// Normalize user input into an http(s) origin. Bare hosts get https://
/// assumed; scheme must be http/https; paths, queries, fragments and
/// credentials are stripped — an instance IS its origin.
pub fn normalize_origin(input: &str) -> Result<Url, String> {
    let input = input.trim();
    let candidate = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let mut url = Url::parse(&candidate).map_err(|e| format!("not a valid URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("“{}” is not an http(s) URL", url.scheme()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with embedded credentials are not accepted".to_string());
    }
    if url.host_str().is_none() {
        return Err("the URL has no host".to_string());
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

/// The origin as a string, without the trailing slash the url crate adds —
/// registry URLs concatenate BEACON_PATH onto it.
pub fn origin_string(url: &Url) -> String {
    url.as_str().trim_end_matches('/').to_string()
}

pub async fn probe(client: &reqwest::Client, origin: &str) -> Result<Beacon, String> {
    let url = format!("{origin}{BEACON_PATH}");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("could not reach {origin}: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "{origin} answered {status} at {BEACON_PATH} — it does not look like a Talaria instance"
        ));
    }
    let beacon: Beacon = response
        .json()
        .await
        .map_err(|e| format!("{origin} did not answer instance info: {e}"))?;
    uuid::Uuid::parse_str(&beacon.instance)
        .map_err(|_| format!("{origin} answered, but not like a Talaria instance"))?;
    Ok(beacon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_hosts_become_https() {
        let url = normalize_origin("outcrop.example").unwrap();
        assert_eq!(origin_string(&url), "https://outcrop.example");
    }

    #[test]
    fn scheme_host_port_survive_but_nothing_else() {
        let url = normalize_origin("http://127.0.0.1:5302/some/path?x=1#frag").unwrap();
        assert_eq!(origin_string(&url), "http://127.0.0.1:5302");
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
    fn garbage_is_refused() {
        assert!(normalize_origin("not a url at all").is_err());
        assert!(normalize_origin("https://").is_err());
    }

    /// One canned HTTP response on an ephemeral port; returns its origin.
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

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
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
        let origin = serve_once(http(
            "HTTP/1.1 200 OK",
            r#"{"instance":"11111111-1111-1111-1111-111111111111","companyName":"Outcrop"}"#,
        ));
        let beacon = probe(&client(), &origin).await.unwrap();
        assert_eq!(beacon.company_name.as_deref(), Some("Outcrop"));
    }

    #[tokio::test]
    async fn company_name_may_be_null() {
        let origin = serve_once(http(
            "HTTP/1.1 200 OK",
            r#"{"instance":"11111111-1111-1111-1111-111111111111","companyName":null}"#,
        ));
        let beacon = probe(&client(), &origin).await.unwrap();
        assert!(beacon.company_name.is_none());
    }

    #[tokio::test]
    async fn non_talaria_answers_are_refused() {
        let origin = serve_once(http("HTTP/1.1 200 OK", r#"{"instance":"not-a-uuid"}"#));
        assert!(probe(&client(), &origin).await.is_err());
    }

    #[tokio::test]
    async fn error_statuses_are_refused() {
        let origin = serve_once(http("HTTP/1.1 404 Not Found", r#"{}"#));
        assert!(probe(&client(), &origin).await.is_err());
    }
}
