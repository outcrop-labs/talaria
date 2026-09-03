// Registry reads — resolve the moving tag to a digest, and read the version
// label beside it. The digest IS the feature: a moving tag half-pushed
// mid-build is exactly what a roll must never boot, so nothing downstream
// (pulls, pins, state) ever names a tag.
//
// NOT safe_fetch, deliberately: the registry is operator-set first-party
// infrastructure (ghcr, a fork's registry, the E2E's localhost registry),
// not a URL that came from a user or an agent — the same exemption
// gateway::provider's client carries, and stated here for the same reason.
//
// THE PROTOCOL, distribution-spec, no docker CLI: /v2/ learns the auth
// challenge (401 + WWW-Authenticate Bearer → the token endpoint names
// itself), the token is anonymous-or-basic (private forks set
// TALARIA_UPDATE_REGISTRY_AUTH=user:pass — an ENV, never a settings row,
// because settings rows serialize to the panel and credentials must not),
// and the manifest HEAD carries every index/manifest Accept so multi-arch
// answers with the INDEX digest — the digest `repo@sha256:…` pins, and the
// one docker pull will resolve to the same amd64 manifest from.

use serde::Deserialize;

use super::layout::default_image_ref;
use super::state::Pin;

/// Every manifest media type in play, index shapes first: an
/// Accept-listing only manifests would make a multi-arch registry silently
/// answer with one platform's manifest, whose digest pins THAT platform —
/// a pin no other platform can pull.
const MANIFEST_ACCEPTS: &str = "application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json";

/// One parsed image reference: registry host (with port), repository path,
/// and the single moving tag the engine reads to resolve a digest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImageRef {
    pub registry: String,
    pub repository: String,
    pub tag: String,
}

/// Parse a docker-style reference. The first component carrying a dot or a
/// colon is the registry (docker's own rule); anything else is docker.io
/// with an implied `library/` namespace. A digest in place of a tag is not
/// accepted here — callers that already hold a digest never re-parse it.
pub fn parse_image_ref(input: &str) -> Option<ImageRef> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let (registry, rest) = match input.split_once('/') {
        Some((first, rest)) if first.contains('.') || first.contains(':') => {
            (first.to_string(), rest.to_string())
        }
        // No explicit registry: docker.io's namespace rules (official
        // images live under library/).
        _ => ("docker.io".to_string(), input.to_string()),
    };
    if !registry_is_wellformed(&registry) {
        return None;
    }
    let (path, tag) = match rest.rsplit_once(':') {
        // A colon after the last slash is a tag; before it, a port on a
        // registry host already consumed above (no slash follows a port).
        Some((p, t)) if !p.is_empty() && !t.is_empty() && !t.contains('/') => {
            (p.to_string(), t.to_string())
        }
        _ => (rest.to_string(), "latest".to_string()),
    };
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return None;
    }
    Some(ImageRef {
        registry,
        repository: path,
        tag,
    })
}

/// host, host:port, [ipv6], or [ipv6]:port — nothing else is a registry a
/// URL can be built from. A trailing colon ("host:") is the shape a typo
/// produces, and without this check it would parse as a registry and fail
/// at request time with a message about a malformed URL instead of a
/// sentence about the setting.
fn registry_is_wellformed(registry: &str) -> bool {
    if let Some(rest) = registry.strip_prefix('[') {
        let Some((_host, tail)) = rest.split_once(']') else {
            return false;
        };
        match tail.strip_prefix(':') {
            Some(port) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
            None => tail.is_empty(),
        }
    } else if let Some((host, port)) = registry.rsplit_once(':') {
        !host.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
    } else {
        !registry.is_empty()
    }
}

/// A content digest as registries spell it. The one format the engine ever
/// records as a pin.
pub fn is_digest(s: &str) -> bool {
    let Some(hex) = s.strip_prefix("sha256:") else {
        return false;
    };
    // Lowercase only: registries normalize hex down, and a pin that records
    // an uppercase spelling is a pin `docker pull` may not accept.
    hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The API scheme for a registry host. Loopback registries are http —
/// docker's own insecure-registry default, and the shape the E2E's
/// localhost registry runs; everything else is https.
fn scheme_for(registry: &str) -> &'static str {
    // Bracketed IPv6 ([::1]:5000) names its host inside the brackets; the
    // naive first-colon split would read "[".
    let host = registry
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_else(|| registry.split(':').next().unwrap_or(registry));
    if host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    {
        "http"
    } else {
        "https"
    }
}

/// The bearer challenge a 401 /v2/ carries: where to get a token and for
/// what service.
#[derive(Debug, PartialEq, Eq)]
struct BearerChallenge {
    realm: String,
    service: String,
}

/// Parse `Bearer realm="…",service="…"` — the challenge header's value,
/// already stripped of the scheme word.
fn parse_bearer_challenge(value: &str) -> Option<BearerChallenge> {
    let mut realm = None;
    let mut service = None;
    for part in value.split(',') {
        let (k, v) = part.trim().split_once('=')?;
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "realm" => realm = Some(v.to_string()),
            "service" => service = Some(v.to_string()),
            _ => {}
        }
    }
    Some(BearerChallenge {
        realm: realm?,
        service: service?,
    })
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

/// A ready-to-use registry client: the base URL and the bearer token (empty
/// when the registry wanted none).
struct RegistrySession {
    base: String,
    auth: Option<String>,
}

impl RegistrySession {
    /// Open the session: learn the challenge, fetch a token if there is
    /// one. `Err` sentences are for the panel — human, not protocol.
    async fn open(image: &ImageRef) -> Result<Self, String> {
        let registry = &image.registry;
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("registry client would not build: {e}"))?;
        let base = format!("{}://{registry}/v2", scheme_for(registry));
        // GET, not HEAD: the distribution spec defines GET /v2/ as the
        // endpoint-check, and ghcr answers 405 to a HEAD on it.
        let probe = client
            .get(format!("{base}/"))
            .send()
            .await
            .map_err(|e| format!("could not reach {registry}: {e}"))?;
        if probe.status().as_u16() != 401 {
            // 200 = no auth; anything else is not a distribution registry.
            if !probe.status().is_success() {
                return Err(format!(
                    "{registry} answered {} on /v2/ — not a registry?",
                    probe.status()
                ));
            }
            return Ok(Self { base, auth: None });
        }
        let challenge = probe
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .and_then(parse_bearer_challenge)
            .ok_or_else(|| format!("{registry} wants an auth this engine does not speak"))?;
        let mut token_url = reqwest::Url::parse(&challenge.realm)
            .map_err(|e| format!("the registry's token endpoint is malformed: {e}"))?;
        token_url
            .query_pairs_mut()
            .append_pair("service", &challenge.service)
            .append_pair("scope", &format!("repository:{}:pull", image.repository));
        let mut req = client.get(token_url);
        // Private forks: basic auth at the TOKEN endpoint (the spec's shape
        // for password-grant pulls). From the env, never the settings row —
        // see the header.
        if let Ok(userpass) = std::env::var("TALARIA_UPDATE_REGISTRY_AUTH")
            && !userpass.is_empty()
        {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(userpass);
            req = req.header(reqwest::header::AUTHORIZATION, format!("Basic {b64}"));
        }
        let token: TokenResponse = req
            .send()
            .await
            .map_err(|e| format!("the registry's token endpoint is unreachable: {e}"))?
            .error_for_status()
            .map_err(|e| format!("the registry refused the pull token: {e}"))?
            .json()
            .await
            .map_err(|e| format!("the registry's token endpoint answered nonsense: {e}"))?;
        let token = token
            .token
            .or(token.access_token)
            .ok_or_else(|| "the registry's token endpoint returned no token".to_string())?;
        Ok(Self {
            base,
            auth: Some(format!("Bearer {token}")),
        })
    }

    fn get(&self, client: &reqwest::Client, path: &str) -> reqwest::RequestBuilder {
        let mut r = client.get(format!("{}{path}", self.base));
        if let Some(auth) = &self.auth {
            r = r.header(reqwest::header::AUTHORIZATION, auth);
        }
        r
    }

    fn head(&self, client: &reqwest::Client, path: &str) -> reqwest::RequestBuilder {
        let mut r = client.head(format!("{}{path}", self.base));
        if let Some(auth) = &self.auth {
            r = r.header(reqwest::header::AUTHORIZATION, auth);
        }
        r
    }
}

/// Resolve the tracked tag (`layout::default_image_ref`) to the pin a roll
/// would apply: the index digest plus the version label. This is the
/// engine's ONLY read of a moving tag, and the answer never carries the tag
/// onward — only the digest.
pub async fn resolve_latest() -> Result<Pin, String> {
    let image = parse_image_ref(&default_image_ref()).ok_or_else(|| {
        format!(
            "TALARIA_UPDATE_IMAGE is not a readable reference: {}",
            default_image_ref()
        )
    })?;
    let digest = resolve_digest(&image).await?;
    let version = fetch_version_label(&image, &digest)
        .await
        .unwrap_or_else(|_| "unknown".into());
    Ok(Pin { digest, version })
}

/// HEAD a reference's manifest and take the `Docker-Content-Digest` — any
/// tag, by the caller's choice (resolve_latest's default; adopt's
/// main-or-running question). Public because adoption is a second reader
/// with a different ref, not a test hook.
pub async fn resolve_digest(image: &ImageRef) -> Result<String, String> {
    let ImageRef {
        registry,
        repository,
        tag,
    } = image;
    let session = RegistrySession::open(image).await?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("registry client would not build: {e}"))?;
    let head = session
        .head(&client, &format!("/{repository}/manifests/{tag}"))
        .header(reqwest::header::ACCEPT, MANIFEST_ACCEPTS)
        .send()
        .await
        .map_err(|e| format!("could not ask {registry} for the manifest: {e}"))?;
    if !head.status().is_success() {
        return Err(format!(
            "{registry}/{repository}:{tag} answered {} — the tag is missing or the registry is unwilling",
            head.status()
        ));
    }
    let digest = head
        .headers()
        .get("docker-content-digest")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| format!("{registry} answered without a content digest — refusing to pin"))?;
    if !is_digest(&digest) {
        return Err(format!(
            "{registry} named a digest this engine refuses to pin: {digest}"
        ));
    }
    Ok(digest)
}

/// Read `org.opencontainers.image.version` off the image the digest names —
/// the human word beside the pin. Best-effort by design (the digest alone
/// rolls fine); callers decide whether "unknown" is worth showing. Public
/// for the live suite and adoption's "what does the running digest call
/// itself" question.
pub async fn fetch_version_label(image: &ImageRef, digest: &str) -> Result<String, String> {
    let session = RegistrySession::open(image).await?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("registry client would not build: {e}"))?;

    // Walk index → amd64 manifest → config blob. An index digest must be
    // walked (its config lives one level down); a plain manifest answers
    // its config directly.
    let manifest: serde_json::Value = session
        .get(
            &client,
            &format!("/{}/manifests/{digest}", image.repository),
        )
        .header(reqwest::header::ACCEPT, MANIFEST_ACCEPTS)
        .send()
        .await
        .map_err(|e| format!("could not fetch the manifest: {e}"))?
        .error_for_status()
        .map_err(|e| format!("the manifest fetch failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("the manifest is not JSON: {e}"))?;
    let manifest = match manifest.get("manifests").and_then(|m| m.as_array()) {
        Some(index) => {
            let amd64 = index
                .iter()
                .find(|m| {
                    m.get("platform")
                        .map(|p| (p["architecture"].as_str(), p["os"].as_str()))
                        == Some((Some("amd64"), Some("linux")))
                })
                .ok_or_else(|| "the image index carries no linux/amd64 manifest".to_string())?;
            let digest = amd64["digest"]
                .as_str()
                .ok_or_else(|| "the amd64 manifest entry names no digest".to_string())?;
            session
                .get(
                    &client,
                    &format!("/{}/manifests/{digest}", image.repository),
                )
                .header(reqwest::header::ACCEPT, MANIFEST_ACCEPTS)
                .send()
                .await
                .map_err(|e| format!("could not fetch the platform manifest: {e}"))?
                .error_for_status()
                .map_err(|e| format!("the platform manifest fetch failed: {e}"))?
                .json()
                .await
                .map_err(|e| format!("the platform manifest is not JSON: {e}"))?
        }
        None => manifest,
    };
    let config_digest = manifest["config"]["digest"]
        .as_str()
        .ok_or_else(|| "the manifest names no config blob".to_string())?;
    let config: serde_json::Value = session
        .get(
            &client,
            &format!("/{}/blobs/{config_digest}", image.repository),
        )
        .send()
        .await
        .map_err(|e| format!("could not fetch the image config: {e}"))?
        .error_for_status()
        .map_err(|e| format!("the image config fetch failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("the image config is not JSON: {e}"))?;
    config["config"]["Labels"]["org.opencontainers.image.version"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the image carries no version label".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_parse_by_dockers_own_rule() {
        let ghcr = parse_image_ref("ghcr.io/outcrop-labs/talaria:main").unwrap();
        assert_eq!(ghcr.registry, "ghcr.io");
        assert_eq!(ghcr.repository, "outcrop-labs/talaria");
        assert_eq!(ghcr.tag, "main");

        let local = parse_image_ref("localhost:5000/talaria:main").unwrap();
        assert_eq!(local.registry, "localhost:5000");
        assert_eq!(local.repository, "talaria");

        // Implicit docker.io, implied latest.
        let official = parse_image_ref("traefik").unwrap();
        assert_eq!(official.registry, "docker.io");
        assert_eq!(official.tag, "latest");

        // A port on the registry host, not a tag separator.
        let port = parse_image_ref("registry.corp:5000/team/talaria").unwrap();
        assert_eq!(port.registry, "registry.corp:5000");
        assert_eq!(port.tag, "latest");

        for bad in [
            "",
            "/leads/with/slash",
            "ghcr.io//double",
            "host:/empty-port",
        ] {
            assert!(parse_image_ref(bad).is_none(), "refused: {bad}");
        }
    }

    #[test]
    fn digests_are_exactly_sha256_plus_64_hex() {
        let good = format!("sha256:{}", "a".repeat(64));
        assert!(is_digest(&good));
        assert!(!is_digest("sha256:short"));
        assert!(
            !is_digest(&format!("sha256:{}", "A".repeat(64))),
            "uppercase is not a registry digest"
        );
        assert!(!is_digest(&format!("sha512:{}", "a".repeat(128))));
        assert!(!is_digest("main"));
    }

    #[test]
    fn loopback_registries_are_http_and_everything_else_https() {
        assert_eq!(scheme_for("localhost:5000"), "http");
        assert_eq!(scheme_for("127.0.0.1"), "http");
        assert_eq!(scheme_for("[::1]:5000"), "http");
        assert_eq!(scheme_for("ghcr.io"), "https");
        assert_eq!(scheme_for("registry.corp:5000"), "https");
    }

    #[test]
    fn challenges_parse_realm_and_service() {
        let c = parse_bearer_challenge(
            r#"realm="https://ghcr.io/token",service="ghcr.io",scope="repository:foo:pull""#,
        )
        .unwrap();
        assert_eq!(c.realm, "https://ghcr.io/token");
        assert_eq!(c.service, "ghcr.io");
        assert!(parse_bearer_challenge("Basic realm=x").is_none());
        assert!(parse_bearer_challenge(r#"realm="only""#).is_none());
    }

    #[test]
    fn the_accept_list_names_every_index_and_manifest_shape() {
        for mt in [
            "application/vnd.oci.image.index.v1+json",
            "application/vnd.docker.distribution.manifest.list.v2+json",
            "application/vnd.oci.image.manifest.v1+json",
            "application/vnd.docker.distribution.manifest.v2+json",
        ] {
            assert!(MANIFEST_ACCEPTS.contains(mt), "missing {mt}");
        }
    }
}
