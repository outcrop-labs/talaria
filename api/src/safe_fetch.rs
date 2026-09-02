// SSRF guard. The door every outbound request to a URL that came from a
// USER, an AGENT, a DB row, or an upstream server's own response has to
// walk through.
//
// The threat: Talaria's server sits inside a private network with a cloud
// metadata service at 169.254.169.254, a Qdrant on loopback, a Postgres, a
// Redis, and whatever else the operator runs. Anyone who can persuade Talaria
// to fetch a URL of their choosing is, without this module, fetching from
// INSIDE that network with Talaria's own reachability — and (for the MCP OAuth
// flow) sometimes with a client_secret attached.
//
// What this module does:
//   • http(s) only — no file:, gopher:, ftp:, data:, jar:, …
//   • resolves the hostname and refuses loopback / RFC1918 / link-local
//     (169.254 — the metadata service) / CGNAT / multicast / reserved space,
//     IPv4 and IPv6, including IPv4-mapped, 6to4 and NAT64 disguises
//   • refuses .internal / .local / .home.arpa / localhost by name
//   • re-validates AFTER EVERY REDIRECT. Following redirects inside the HTTP
//     client resolves the chain where we never see the hops — so the safe
//     client never follows (`Policy::none`) and we drive the chain ourselves,
//     re-checking each Location. Without this, an allowed public host 302s
//     straight to 169.254.169.254.
//   • strips Authorization / Cookie / api-key-ish headers on a cross-origin
//     redirect, so a hostile upstream can't harvest credentials by bouncing us
//   • bounds every request: overall timeout + response size cap
//   • re-validates at CONNECT time through a custom resolver — the resolution
//     the socket actually uses is checked, not just the pre-flight one. The
//     resolver rides on the client, so every request this client makes dials
//     through it. Still best-effort against a service Talaria is SUPPOSED to
//     reach being hostile.
//
// Odd address spellings — "010.1.1.1", say — do not parse as addresses here;
// they fall through to the NAME path, where no resolver will answer them:
// failing closed rather than guessing at loose spellings.
//
// What deliberately does NOT come through here: Talaria's own first-party
// infrastructure — the gateway's provider endpoints, Qdrant, the embedder,
// object storage, the built-in MCP toolkit on loopback. Those URLs are set by
// the OPERATOR, not by a user or an agent, and they legitimately point at
// loopback/docker-internal addresses (gateway::provider::http is that client).
// Each such call site says so at the call site.
//
// TALARIA_FETCH_ALLOW_HOSTS is the escape hatch for operators who really do
// want user-configurable targets on their internal network (a self-hosted MCP
// server on the LAN, say). Comma/space separated, each entry either
//   a host        mcp.corp.example     (exact, case-insensitive)
//   a host suffix *.corp.example       (matches the domain and anything under it)
//   an address    127.0.0.1
//   a CIDR        10.42.0.0/16         (permits resolved addresses in range)
// A hostname match short-circuits address checking entirely; a CIDR match
// permits addresses in that range wherever they were resolved from.

use std::net::IpAddr;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use reqwest::Method;
use reqwest::header::{HeaderMap, HeaderName};

// ── Address classification ──────────────────────────────────────────────────

/// Parsed address bytes, with v6 wrappers around a v4 address unwrapped so the
/// v4 rules apply to ::ffff:10.0.0.1, 2002:0a00:0001::, 64:ff9b::10.0.0.1.
/// Brackets and a zone suffix are tolerated on the way in.
fn ip_bytes(ip: &str) -> Option<Vec<u8>> {
    let s = ip.strip_prefix('[').unwrap_or(ip);
    let s = s.strip_suffix(']').unwrap_or(s);
    let s = s.split('%').next().unwrap_or("");
    match s.parse::<IpAddr>().ok()? {
        IpAddr::V4(v4) => Some(v4.octets().to_vec()),
        IpAddr::V6(v6) => {
            let b = v6.octets();
            let is_prefix = |p: &[u8]| p.iter().enumerate().all(|(i, x)| b[i] == *x);
            if is_prefix(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff]) {
                return Some(b[12..].to_vec()); // ::ffff:a.b.c.d
            }
            if is_prefix(&[0x20, 0x02]) {
                return Some(b[2..6].to_vec()); // 6to4
            }
            if is_prefix(&[0x00, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0]) {
                return Some(b[12..].to_vec()); // NAT64
            }
            Some(b.to_vec())
        }
    }
}

#[derive(Clone)]
struct Cidr {
    bytes: Vec<u8>,
    bits: u32,
}

fn parse_cidr(spec: &str) -> Option<Cidr> {
    let (addr, mask) = match spec.split_once('/') {
        Some((a, m)) => (a, Some(m)),
        None => (spec, None),
    };
    if addr.is_empty() {
        return None;
    }
    let bytes = ip_bytes(addr.trim())?;
    let bits = match mask {
        None => (bytes.len() * 8) as u32,
        Some(m) => {
            let bits: u32 = m.parse().ok()?;
            if bits > (bytes.len() * 8) as u32 {
                return None;
            }
            bits
        }
    };
    Some(Cidr { bytes, bits })
}

fn in_cidr(ip: &[u8], c: &Cidr) -> bool {
    if ip.len() != c.bytes.len() {
        return false;
    }
    let full = (c.bits >> 3) as usize;
    if ip[..full.min(ip.len())] != c.bytes[..full.min(c.bytes.len())] {
        return false;
    }
    let rem = c.bits & 7;
    if rem == 0 {
        return true;
    }
    let mask = 0xffu8 << (8 - rem);
    (ip[full] & mask) == (c.bytes[full] & mask)
}

// Everything that is not globally routable public internet. Fail closed:
// anything reserved, private, or special-use is refused.
const BLOCKED: &[(&str, &str)] = &[
    ("0.0.0.0/8", "unspecified/this-network"),
    ("10.0.0.0/8", "private network (RFC1918)"),
    ("100.64.0.0/10", "carrier-grade NAT (RFC6598)"),
    ("127.0.0.0/8", "loopback"),
    (
        "169.254.0.0/16",
        "link-local — cloud instance metadata lives here",
    ),
    ("172.16.0.0/12", "private network (RFC1918)"),
    ("192.0.0.0/24", "IETF protocol assignments"),
    ("192.0.2.0/24", "documentation range"),
    ("192.88.99.0/24", "6to4 relay anycast"),
    ("192.168.0.0/16", "private network (RFC1918)"),
    ("198.18.0.0/15", "benchmarking range"),
    ("198.51.100.0/24", "documentation range"),
    ("203.0.113.0/24", "documentation range"),
    ("224.0.0.0/4", "multicast"),
    ("240.0.0.0/4", "reserved"),
    ("::/128", "unspecified"),
    ("::1/128", "IPv6 loopback"),
    ("fc00::/7", "IPv6 unique-local (ULA)"),
    ("fe80::/10", "IPv6 link-local"),
    ("ff00::/8", "IPv6 multicast"),
    ("2001:db8::/32", "documentation range"),
];

static BLOCKED_NETS: LazyLock<Vec<(Cidr, &'static str)>> = LazyLock::new(|| {
    BLOCKED
        .iter()
        .map(|(cidr, why)| {
            let net =
                parse_cidr(cidr).unwrap_or_else(|| panic!("safe-fetch: bad built-in CIDR {cidr}"));
            (net, *why)
        })
        .collect()
});

/// Why this address is refused, or None when it looks like public internet.
pub fn blocked_address_reason(ip: &str) -> Option<&'static str> {
    let bytes = ip_bytes(ip)?;
    BLOCKED_NETS
        .iter()
        .find(|(net, _)| in_cidr(&bytes, net))
        .map(|(_, why)| *why)
}

// Names that mean "inside", by convention or by RFC.
const BLOCKED_SUFFIXES: &[&str] = &[
    ".internal",
    ".local",
    ".localhost",
    ".home.arpa",
    ".intranet",
    ".lan",
];
const BLOCKED_NAMES: &[&str] = &["localhost", "ip6-localhost", "ip6-loopback"];

// ── Operator allowlist ──────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct AllowList {
    spec: String,
    hosts: Vec<String>,
    suffixes: Vec<String>,
    nets: Vec<Cidr>,
}

impl AllowList {
    /// Pure parser for the TALARIA_FETCH_ALLOW_HOSTS spec — pure so the shapes
    /// are testable without touching process env from a parallel test binary.
    fn parse(spec: &str) -> AllowList {
        let mut out = AllowList {
            spec: spec.to_string(),
            ..AllowList::default()
        };
        for raw in spec.split([',', ' ', '\t']) {
            let entry = raw.trim().to_lowercase();
            if entry.is_empty() {
                continue;
            }
            if let Some(rest) = entry.strip_prefix("*.") {
                out.suffixes.push(format!(".{rest}")); // ".corp.example"
                out.hosts.push(rest.to_string());
                continue;
            }
            if entry.contains('/')
                && let Some(net) = parse_cidr(&entry)
            {
                out.nets.push(net);
                continue;
            }
            match ip_bytes(&entry) {
                Some(bare) => {
                    let bits = (bare.len() * 8) as u32;
                    out.nets.push(Cidr { bytes: bare, bits });
                }
                None => out.hosts.push(entry),
            }
        }
        out
    }

    fn host_allowed(&self, hostname: &str) -> bool {
        let h = hostname.trim_end_matches('.').to_lowercase();
        self.hosts.contains(&h) || self.suffixes.iter().any(|s| h.ends_with(s.as_str()))
    }

    fn address_allowed(&self, ip: &str) -> bool {
        match ip_bytes(ip) {
            Some(bytes) => self.nets.iter().any(|n| in_cidr(&bytes, n)),
            None => false,
        }
    }
}

static ALLOW_CACHE: LazyLock<Mutex<Option<AllowList>>> = LazyLock::new(|| Mutex::new(None));

/// Read per call, not from a validated snapshot: the cache keys on the raw
/// spec so an edited/overridden value takes effect on the next fetch. The key
/// itself is validated and documented in the env schema, which runs at boot.
fn allow_list() -> AllowList {
    let spec = std::env::var("TALARIA_FETCH_ALLOW_HOSTS").unwrap_or_default();
    let mut cached = ALLOW_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(a) = cached.as_ref()
        && a.spec == spec
    {
        return a.clone();
    }
    let parsed = AllowList::parse(&spec);
    *cached = Some(parsed.clone());
    parsed
}

/// An allowlisted hostname skips address checking entirely — the operator has
/// said this specific name is theirs.
pub fn host_allowed(hostname: &str) -> bool {
    allow_list().host_allowed(hostname)
}

fn address_allowed(ip: &str) -> bool {
    allow_list().address_allowed(ip)
}

// ── URL validation ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct BlockedUrlError(pub String);

impl std::fmt::Display for BlockedUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for BlockedUrlError {}

fn host_of(url: &reqwest::Url) -> Option<String> {
    let h = url.host_str()?;
    let h = h.strip_prefix('[').unwrap_or(h);
    let h = h.strip_suffix(']').unwrap_or(h);
    Some(h.trim_end_matches('.').to_lowercase())
}

/// The per-address check both the pre-flight walk and the connect-time
/// resolver run: allowlisted ranges pass, everything refused by the blocked
/// table says why, public internet says nothing.
fn address_refusal(ip: &IpAddr) -> Option<String> {
    let ip = ip.to_string();
    if address_allowed(&ip) {
        return None;
    }
    blocked_address_reason(&ip).map(|why| format!("it resolves to {ip} ({why})"))
}

async fn resolve_all(host: &str) -> Result<Vec<IpAddr>, String> {
    let h = host.to_string();
    tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        (h.as_str(), 0u16)
            .to_socket_addrs()
            .map(|it| it.map(|sa| sa.ip()).collect::<Vec<_>>())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Errors BlockedUrlError unless this URL is http(s) and lands on a public
/// address (or somewhere the operator allowlisted). Exported for callers that
/// want to validate a stored URL without fetching it.
pub async fn assert_fetchable_url(input: &str) -> Result<reqwest::Url, BlockedUrlError> {
    let truncate = |s: &str| s.chars().take(120).collect::<String>();
    let url = reqwest::Url::parse(input)
        .map_err(|_| BlockedUrlError(format!("not a valid URL: {}", truncate(input))))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(BlockedUrlError(format!(
            "refusing {}// — only http and https are allowed",
            url.scheme()
        )));
    }
    let Some(host) = host_of(&url) else {
        return Err(BlockedUrlError("URL has no host".into()));
    };
    if host_allowed(&host) {
        return Ok(url);
    }
    if ip_bytes(&host).is_some() {
        if address_allowed(&host) {
            return Ok(url);
        }
        if let Some(why) = blocked_address_reason(&host) {
            return Err(BlockedUrlError(format!("refusing {host} — {why}")));
        }
        return Ok(url);
    }
    if BLOCKED_NAMES.contains(&host.as_str()) || BLOCKED_SUFFIXES.iter().any(|s| host.ends_with(s))
    {
        return Err(BlockedUrlError(format!(
            "refusing {host} — internal-only hostname"
        )));
    }
    let addrs = resolve_all(&host)
        .await
        .map_err(|_| BlockedUrlError(format!("could not resolve {host}")))?;
    if addrs.is_empty() {
        return Err(BlockedUrlError(format!("could not resolve {host}")));
    }
    for ip in &addrs {
        if let Some(refusal) = address_refusal(ip) {
            return Err(BlockedUrlError(format!("refusing {host} — {refusal}")));
        }
    }
    Ok(url)
}

// ── Connect-time re-validation (closes most of the rebinding window) ─────────

/// A reqwest resolver that re-checks the address at CONNECT time — the
/// resolution the socket actually uses, not just the pre-flight one. The
/// resolver rides on the client, so every request the safe client makes
/// dials through this.
#[derive(Clone, Default)]
pub struct ValidatingResolver;

impl reqwest::dns::Resolve for ValidatingResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let h = host.clone();
            let addrs = tokio::task::spawn_blocking(move || {
                use std::net::ToSocketAddrs;
                (h.as_str(), 0u16)
                    .to_socket_addrs()
                    .map(|it| it.map(|sa| sa.ip()).collect::<Vec<_>>())
            })
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            if !host_allowed(&host) {
                for ip in &addrs {
                    if let Some(refusal) = address_refusal(ip) {
                        return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                            "refusing {host} — {refusal}"
                        )));
                    }
                }
            }
            let sockets: Vec<std::net::SocketAddr> = addrs
                .iter()
                .map(|ip| std::net::SocketAddr::new(*ip, 0))
                .collect();
            Ok(Box::new(sockets.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

// ── The fetch ───────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct SafeFetch<'a> {
    pub method: Option<&'a str>,
    pub headers: Vec<(&'a str, &'a str)>,
    /// Replayable by design — a redirect re-sends it.
    pub body: Option<&'a [u8]>,
    /// Whole-request budget, redirects included. Default 15s.
    pub timeout_ms: Option<u64>,
    /// Response body cap; a larger body (or Content-Length) is refused. Default 5MB.
    pub max_bytes: Option<u64>,
    /// Default 5.
    pub max_redirects: Option<u32>,
}

#[derive(Debug)]
pub enum SafeError {
    Blocked(BlockedUrlError),
    Fetch(String),
    Timeout,
}

impl std::fmt::Display for SafeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SafeError::Blocked(e) => write!(f, "{e}"),
            SafeError::Fetch(e) => f.write_str(e),
            SafeError::Timeout => f.write_str("safe fetch timed out"),
        }
    }
}
impl std::error::Error for SafeError {}

impl From<BlockedUrlError> for SafeError {
    fn from(e: BlockedUrlError) -> Self {
        SafeError::Blocked(e)
    }
}

pub struct SafeResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

const REDIRECTS: &[u16] = &[301, 302, 303, 307, 308];
const NULL_BODY: &[u16] = &[101, 204, 205, 304];

/// Anything that could carry a credential gets dropped when the redirect
/// leaves the origin we validated it for: the three exact names
/// (authorization, cookie, proxy-authorization) and any header whose name
/// carries key, token, secret, auth, or credential.
fn is_credential_header(name: &str) -> bool {
    let n = name.to_lowercase();
    n == "authorization"
        || n == "cookie"
        || n == "proxy-authorization"
        || n.contains("key")
        || n.contains("token")
        || n.contains("secret")
        || n.contains("auth")
        || n.contains("credential")
}

fn strip_credentials(headers: &mut HeaderMap) {
    let names: Vec<HeaderName> = headers
        .keys()
        .filter(|k| is_credential_header(k.as_str()))
        .cloned()
        .collect();
    for name in names {
        headers.remove(&name);
    }
}

/// 303 always becomes GET; 301/302 only when leaving POST.
fn downgrade_to_get(status: u16, method: &Method) -> bool {
    status == 303 || ((status == 301 || status == 302) && *method == Method::POST)
}

/// The guarded client. Redirects are NEVER followed by the client itself
/// (`Policy::none`) — safe_fetch drives the chain so every hop is validated.
/// Its resolver is the validating one, closing the rebinding window at
/// connect time.
fn safe_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .dns_resolver(Arc::new(ValidatingResolver))
                .build()
                .expect("safe client builds")
        })
        .clone()
}

async fn cap_body(mut res: reqwest::Response, max_bytes: u64) -> Result<SafeResponse, SafeError> {
    if let Some(declared) = res.content_length()
        && declared > max_bytes
    {
        return Err(SafeError::Blocked(BlockedUrlError(format!(
            "response declares {declared} bytes, over the {max_bytes}-byte cap"
        ))));
    }
    let status = res.status().as_u16();
    let mut headers = res.headers().clone();
    let mut body = Vec::new();
    if !NULL_BODY.contains(&status) {
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|e| SafeError::Fetch(e.to_string()))?
        {
            body.extend_from_slice(&chunk);
            if body.len() as u64 > max_bytes {
                return Err(SafeError::Blocked(BlockedUrlError(format!(
                    "response exceeded the {max_bytes}-byte cap"
                ))));
            }
        }
    }
    // The body is returned as the bytes the caller will read; length and
    // encoding headers describe the wire shape, not this buffer. (This client
    // sends no Accept-Encoding, so encoding is not in play — but the delete
    // keeps the contract honest if that ever changes.)
    headers.remove(reqwest::header::CONTENT_ENCODING);
    headers.remove(reqwest::header::CONTENT_LENGTH);
    Ok(SafeResponse {
        status,
        headers,
        body,
    })
}

/// fetch() for URLs Talaria does not control. Same shape as fetch, minus
/// redirect following (we always drive the chain) — every hop is re-validated,
/// the whole thing is time-bounded, and the body is size-capped.
///
/// Errors are SafeError::Blocked for refusals and caps; network errors surface
/// as SafeError::Fetch; the whole-budget timeout as SafeError::Timeout.
pub async fn safe_fetch(input: &str, init: SafeFetch<'_>) -> Result<SafeResponse, SafeError> {
    let timeout_ms = init.timeout_ms.unwrap_or(15_000);
    let max_bytes = init.max_bytes.unwrap_or(5 * 1024 * 1024);
    let max_redirects = init.max_redirects.unwrap_or(5);

    let whole = async {
        let mut target = assert_fetchable_url(input).await?;
        let mut method = init
            .method
            .unwrap_or("GET")
            .parse::<Method>()
            .map_err(|e| SafeError::Fetch(format!("bad method: {e}")))?;
        let mut body = init.body.map(|b| b.to_vec());
        let mut headers = HeaderMap::new();
        for (name, value) in &init.headers {
            let name: HeaderName = name
                .parse()
                .map_err(|e| SafeError::Fetch(format!("bad header name {name}: {e}")))?;
            let value = value
                .parse()
                .map_err(|e| SafeError::Fetch(format!("bad header value: {e}")))?;
            headers.insert(name, value);
        }

        let mut hop = 0u32;
        loop {
            let mut req = safe_client().request(method.clone(), target.clone());
            if let Some(b) = &body {
                req = req.body(b.clone());
            }
            req = req.headers(headers.clone());
            let res = req
                .send()
                .await
                .map_err(|e| SafeError::Fetch(e.to_string()))?;
            let status = res.status().as_u16();
            let location = if REDIRECTS.contains(&status) {
                res.headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            } else {
                None
            };
            let Some(location) = location else {
                return cap_body(res, max_bytes).await;
            };
            if hop >= max_redirects {
                return Err(SafeError::Blocked(BlockedUrlError(format!(
                    "too many redirects (over {max_redirects})"
                ))));
            }
            // Drain the hop's body before moving on.
            let _ = res.bytes().await;

            // THE point of driving redirects by hand: the new target is
            // validated before we dial it, so a public host cannot bounce us
            // onto the metadata service or a loopback admin port.
            let joined = target
                .join(&location)
                .map_err(|e| SafeError::Fetch(format!("bad redirect location: {e}")))?;
            let next = assert_fetchable_url(joined.as_str()).await?;
            if next.origin() != target.origin() {
                strip_credentials(&mut headers);
            }
            if downgrade_to_get(status, &method) {
                method = Method::GET;
                body = None;
                headers.remove(reqwest::header::CONTENT_TYPE);
                headers.remove(reqwest::header::CONTENT_LENGTH);
            }
            target = next;
            hop += 1;
        }
    };
    tokio::time::timeout(Duration::from_millis(timeout_ms), whole)
        .await
        .map_err(|_| SafeError::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reserved_v4_family_is_refused_with_its_reason() {
        assert_eq!(
            blocked_address_reason("0.0.0.1"),
            Some("unspecified/this-network")
        );
        assert_eq!(
            blocked_address_reason("10.1.2.3"),
            Some("private network (RFC1918)")
        );
        assert_eq!(
            blocked_address_reason("100.64.0.1"),
            Some("carrier-grade NAT (RFC6598)")
        );
        // The /10 CGNAT edge — 100.127.255.255 is the last address in it.
        assert_eq!(
            blocked_address_reason("100.127.255.255"),
            Some("carrier-grade NAT (RFC6598)")
        );
        assert_eq!(blocked_address_reason("100.128.0.0"), None);
        assert_eq!(blocked_address_reason("127.0.0.1"), Some("loopback"));
        assert_eq!(
            blocked_address_reason("169.254.169.254"),
            Some("link-local — cloud instance metadata lives here")
        );
        assert_eq!(
            blocked_address_reason("172.16.0.1"),
            Some("private network (RFC1918)")
        );
        assert_eq!(
            blocked_address_reason("172.31.255.255"),
            Some("private network (RFC1918)")
        );
        assert_eq!(blocked_address_reason("172.32.0.1"), None);
        assert_eq!(
            blocked_address_reason("192.168.1.1"),
            Some("private network (RFC1918)")
        );
        assert_eq!(
            blocked_address_reason("192.0.2.9"),
            Some("documentation range")
        );
        assert_eq!(
            blocked_address_reason("198.18.0.1"),
            Some("benchmarking range")
        );
        assert_eq!(
            blocked_address_reason("203.0.113.5"),
            Some("documentation range")
        );
        assert_eq!(blocked_address_reason("224.0.0.1"), Some("multicast"));
        assert_eq!(blocked_address_reason("240.0.0.1"), Some("reserved"));
        // Public internet says nothing.
        assert_eq!(blocked_address_reason("8.8.8.8"), None);
        assert_eq!(blocked_address_reason("1.1.1.1"), None);
        // Unparseable is refused, not guessed.
        assert_eq!(blocked_address_reason("not-an-ip"), None);
    }

    #[test]
    fn every_reserved_v6_family_is_refused() {
        assert_eq!(blocked_address_reason("::"), Some("unspecified"));
        assert_eq!(blocked_address_reason("::1"), Some("IPv6 loopback"));
        assert_eq!(
            blocked_address_reason("fc00::1"),
            Some("IPv6 unique-local (ULA)")
        );
        assert_eq!(
            blocked_address_reason("fd12:3456::1"),
            Some("IPv6 unique-local (ULA)")
        );
        assert_eq!(blocked_address_reason("fe80::1"), Some("IPv6 link-local"));
        assert_eq!(blocked_address_reason("ff02::1"), Some("IPv6 multicast"));
        assert_eq!(
            blocked_address_reason("2001:db8::1"),
            Some("documentation range")
        );
        assert_eq!(blocked_address_reason("2606:4700::1111"), None);
    }

    #[test]
    fn v6_wrappers_around_v4_get_the_v4_rules() {
        // ::ffff:a.b.c.d — IPv4-mapped.
        assert_eq!(
            blocked_address_reason("::ffff:10.0.0.1"),
            Some("private network (RFC1918)")
        );
        assert_eq!(blocked_address_reason("::ffff:8.8.8.8"), None);
        // 2002:0a00:0001:: — 6to4 carrying 10.0.0.1.
        assert_eq!(
            blocked_address_reason("2002:0a00:0001::"),
            Some("private network (RFC1918)")
        );
        // 64:ff9b::10.0.0.1 — NAT64.
        assert_eq!(
            blocked_address_reason("64:ff9b::10.0.0.1"),
            Some("private network (RFC1918)")
        );
    }

    #[test]
    fn cidrs_parse_and_match_on_partial_octets() {
        let c = parse_cidr("10.42.0.0/16").unwrap();
        assert!(in_cidr(&ip_bytes("10.42.9.9").unwrap(), &c));
        assert!(!in_cidr(&ip_bytes("10.43.0.1").unwrap(), &c));
        let c = parse_cidr("10.0.0.1").unwrap(); // no mask = the whole address
        assert!(in_cidr(&ip_bytes("10.0.0.1").unwrap(), &c));
        assert!(!in_cidr(&ip_bytes("10.0.0.2").unwrap(), &c));
        assert!(parse_cidr("300.0.0.0/8").is_none());
        assert!(parse_cidr("10.0.0.0/33").is_none());
        assert!(parse_cidr("abc").is_none());
        assert!(parse_cidr("/16").is_none());
        // v4 and v6 nets never cross-match.
        let c = parse_cidr("::/128").unwrap();
        assert!(!in_cidr(&ip_bytes("0.0.0.0").unwrap(), &c));
    }

    #[test]
    fn the_allowlist_takes_hosts_suffixes_addresses_and_cidrs() {
        let a = AllowList::parse("mcp.corp.example, *.shop.internal 10.42.0.0/16 127.0.0.1 ::1");
        assert!(a.host_allowed("mcp.corp.example"));
        assert!(!a.host_allowed("other.corp.example"));
        // *.shop.internal matches the domain and anything under it.
        assert!(a.host_allowed("shop.internal"));
        assert!(a.host_allowed("a.b.shop.internal"));
        assert!(!a.host_allowed("xshop.internal"));
        // Trailing dots and case fold away.
        assert!(a.host_allowed("MCP.Corp.Example."));
        assert!(a.address_allowed("10.42.9.9"));
        assert!(!a.address_allowed("10.43.0.1"));
        assert!(a.address_allowed("127.0.0.1"));
        assert!(a.address_allowed("::1"));
        assert!(!a.address_allowed("::")); // a v6 entry allows itself, nothing wider
        assert!(!a.address_allowed("169.254.169.254"));
    }

    async fn blocked_reason(input: &str) -> String {
        assert_fetchable_url(input)
            .await
            .err()
            .unwrap_or_else(|| panic!("expected {input} to be blocked"))
            .0
    }

    #[tokio::test]
    async fn literal_and_protocol_refusals_never_touch_dns() {
        assert_eq!(
            blocked_reason("file:///etc/passwd").await,
            "refusing file// — only http and https are allowed"
        );
        assert!(
            blocked_reason("not a url at all")
                .await
                .starts_with("not a valid URL")
        );
        assert_eq!(
            blocked_reason("http://127.0.0.1:8080/admin").await,
            "refusing 127.0.0.1 — loopback"
        );
        assert_eq!(
            blocked_reason("https://169.254.169.254/meta").await,
            "refusing 169.254.169.254 — link-local — cloud instance metadata lives here"
        );
        assert_eq!(
            blocked_reason("https://[::1]/x").await,
            "refusing ::1 — IPv6 loopback"
        );
        assert_eq!(
            blocked_reason("http://10.0.0.5/x").await,
            "refusing 10.0.0.5 — private network (RFC1918)"
        );
        // Names that mean "inside" are refused by NAME, before resolution.
        assert_eq!(
            blocked_reason("http://localhost:9/x").await,
            "refusing localhost — internal-only hostname"
        );
        assert_eq!(
            blocked_reason("http://box.internal/x").await,
            "refusing box.internal — internal-only hostname"
        );
        assert_eq!(
            blocked_reason("http://thing.home.arpa/x").await,
            "refusing thing.home.arpa — internal-only hostname"
        );
    }

    #[test]
    fn credential_headers_are_recognized_like_the_ts_regex() {
        for kept in ["content-type", "user-agent", "accept", "x-request-id"] {
            assert!(!is_credential_header(kept), "{kept} must survive");
        }
        for stripped in [
            "authorization",
            "Authorization",
            "cookie",
            "proxy-authorization",
            "x-api-key",
            "X-Api-Key",
            "openai-token",
            "anthropic-secret",
            "bearer-auth",
            "proxy-credential",
        ] {
            assert!(is_credential_header(stripped), "{stripped} must be dropped");
        }
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer x".parse().unwrap());
        headers.insert("x-api-key", "k".parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());
        strip_credentials(&mut headers);
        assert!(headers.get("authorization").is_none());
        assert!(headers.get("x-api-key").is_none());
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn only_303_always_and_301_302_from_post_downgrade_to_get() {
        assert!(downgrade_to_get(303, &Method::POST));
        assert!(downgrade_to_get(303, &Method::GET)); // stays GET — a no-op
        assert!(downgrade_to_get(301, &Method::POST));
        assert!(downgrade_to_get(302, &Method::POST));
        assert!(!downgrade_to_get(301, &Method::GET));
        assert!(!downgrade_to_get(302, &Method::PUT));
        assert!(!downgrade_to_get(307, &Method::POST));
        assert!(!downgrade_to_get(308, &Method::POST));
    }
}
