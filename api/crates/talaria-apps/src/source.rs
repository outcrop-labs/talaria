// Where an app's code comes from: one git repository, read at one ref. The
// install URL is parsed here once, and what install records (`app_sources`)
// and update re-reads is the parsed shape, never a re-guess.

/// How the URL reaches its repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Https,
    /// Plain http to this machine's loopback, with local sources opted in.
    Http,
    /// A repository on this machine's disk, with local sources opted in.
    File,
}

/// A parsed install source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    /// The URL git reads, as given (trimmed, no trailing slash).
    pub url: String,
    pub scheme: Scheme,
    /// The host a credential is spent at, lowercased, without the port.
    /// Empty for `file://`.
    pub host: String,
    /// The repository path on that host, without a leading slash or a
    /// trailing `.git`: `owner/name` on GitHub.
    pub repo: String,
}

impl GitSource {
    /// `owner/name` when this is a repository on github.com — the one host
    /// the GitHub connection can mint a token for.
    pub fn github_repo(&self) -> Option<&str> {
        if self.scheme != Scheme::Https || self.host != "github.com" {
            return None;
        }
        let mut parts = self.repo.split('/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(owner), Some(name), None) if !owner.is_empty() && !name.is_empty() => {
                Some(&self.repo)
            }
            _ => None,
        }
    }

    /// The last path segment, the name a slug is derived from.
    pub fn name(&self) -> &str {
        self.repo.rsplit('/').next().unwrap_or(&self.repo)
    }
}

/// Whether this instance installs from local sources (`file://`, and plain
/// `http://` to loopback): `TALARIA_APPS_LOCAL_SOURCES=1`. Off by default and
/// never needed in production — it exists so a development stack can install
/// and update an app from a repository on the same machine through the same
/// path a GitHub install takes.
pub fn local_sources_allowed() -> bool {
    std::env::var("TALARIA_APPS_LOCAL_SOURCES").is_ok_and(|v| v.trim() == "1")
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

/// Parse an install URL. `https://host/path` always; with `local_ok`, also
/// `file:///abs/path` and `http://` to loopback. A URL that carries a user or
/// password is refused: a credential belongs in a workspace secret, never in a
/// URL that is recorded, audited and shown.
pub fn parse_source(raw: &str, local_ok: bool) -> Result<GitSource, String> {
    let u = raw.trim().trim_end_matches('/');
    if u.is_empty() || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("install URL must be https://".into());
    }
    let parsed = url::Url::parse(u).map_err(|_| "install URL must be https://".to_string())?;
    let scheme = match parsed.scheme() {
        "https" => Scheme::Https,
        "http" if local_ok => Scheme::Http,
        "file" if local_ok => Scheme::File,
        _ if local_ok => {
            return Err("install URL must be https://, or file:// or http:// to loopback".into());
        }
        _ => return Err("install URL must be https://".into()),
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(
            "the install URL carries a credential: remove it, and name a workspace secret instead"
                .into(),
        );
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("the install URL must not carry a query or a fragment".into());
    }
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    match scheme {
        Scheme::Https if host.is_empty() => return Err("install URL must name a host".into()),
        Scheme::Http if !is_loopback(&host) => {
            return Err("plain http:// is accepted only to this machine's loopback".into());
        }
        Scheme::File if !host.is_empty() => {
            return Err("a file:// URL must be a local path (file:///…)".into());
        }
        _ => {}
    }
    let path = parsed.path().trim_matches('/');
    let repo = path.strip_suffix(".git").unwrap_or(path).to_string();
    if repo.is_empty()
        || repo
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err("install URL must name a repository".into());
    }
    Ok(GitSource {
        url: u.to_string(),
        scheme,
        host,
        repo,
    })
}

/// A ref an admin may name for install or update: a branch, a tag or a
/// commit. Refused when git could read it as an option or a path trick.
pub fn ref_ok(r: &str) -> Result<(), String> {
    let ok = !r.is_empty()
        && r.len() <= 200
        && !r.starts_with('-')
        && !r.starts_with('/')
        && !r.ends_with('/')
        && !r.ends_with(".lock")
        && !r.contains("..")
        && !r.contains("//")
        && !r.contains("@{")
        && r.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "\"{r}\" is not a usable git ref (a branch, tag or commit)"
        ))
    }
}

/// A full commit id: 40 (SHA-1) or 64 (SHA-256) hex digits.
pub fn is_commit(r: &str) -> bool {
    matches!(r.len(), 40 | 64) && r.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_https_url_parses_to_host_and_repo() {
        let s = parse_source("https://github.com/acme/talaria-app-crm.git/", false).unwrap();
        assert_eq!(s.scheme, Scheme::Https);
        assert_eq!(s.host, "github.com");
        assert_eq!(s.repo, "acme/talaria-app-crm");
        assert_eq!(s.github_repo(), Some("acme/talaria-app-crm"));
        assert_eq!(s.name(), "talaria-app-crm");
        let gl = parse_source("https://GitLab.example.com:8443/group/sub/app", false).unwrap();
        assert_eq!(gl.host, "gitlab.example.com");
        assert_eq!(gl.repo, "group/sub/app");
        assert_eq!(gl.github_repo(), None);
        // Deeper than owner/name is not a GitHub repository URL.
        let deep = parse_source("https://github.com/acme/crm/tree/main", false).unwrap();
        assert_eq!(deep.github_repo(), None);
    }

    #[test]
    fn local_sources_need_the_opt_in_and_loopback() {
        assert!(parse_source("file:///srv/repos/crm", false).is_err());
        assert!(parse_source("http://127.0.0.1:9418/crm.git", false).is_err());
        let f = parse_source("file:///srv/repos/crm", true).unwrap();
        assert_eq!(
            (f.scheme, f.host.as_str(), f.repo.as_str()),
            (Scheme::File, "", "srv/repos/crm")
        );
        let h = parse_source("http://127.0.0.1:9418/crm.git", true).unwrap();
        assert_eq!(
            (h.scheme, h.host.as_str(), h.repo.as_str()),
            (Scheme::Http, "127.0.0.1", "crm")
        );
        let err = parse_source("http://example.com/crm.git", true).unwrap_err();
        assert!(err.contains("loopback"), "{err}");
        assert!(parse_source("ssh://git@github.com/acme/crm", true).is_err());
        assert!(parse_source("ext::sh -c touch% /tmp/pwned", true).is_err());
    }

    #[test]
    fn a_url_with_a_credential_is_refused() {
        for u in [
            "https://x-access-token:ghs_abc@github.com/acme/crm",
            "https://token@github.com/acme/crm",
        ] {
            let err = parse_source(u, false).unwrap_err();
            assert!(err.contains("workspace secret"), "{u}: {err}");
        }
        assert!(parse_source("https://github.com/acme/crm?ref=main", false).is_err());
        assert!(parse_source("https://github.com/", false).is_err());
        assert!(
            parse_source("https://github.com/acme/../crm", false)
                .is_ok_and(|s| !s.repo.contains(".."))
        );
        assert!(parse_source("https://github.com/a b", false).is_err());
    }

    #[test]
    fn refs() {
        for good in ["main", "release/1.2", "v1.0.0", "a1b2c3", "feature_x-2"] {
            assert!(ref_ok(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "-upload-pack=x",
            "../x",
            "a..b",
            "x.lock",
            "/main",
            "main/",
            "a b",
            "x@{1}",
            "a//b",
        ] {
            assert!(ref_ok(bad).is_err(), "{bad}");
        }
        assert!(is_commit(&"a".repeat(40)));
        assert!(!is_commit("main"));
    }
}
