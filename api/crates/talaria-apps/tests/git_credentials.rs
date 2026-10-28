// The token plumbing of app install and update, against a mock: a local
// smart-HTTP git server (a CGI bridge to `git http-backend`) that demands a
// Basic login, and mock credentials in place of the registered git hosts and
// the workspace secrets. No network, no live GitHub.
//
// What it holds the plumbing to: the token reaches git only through askpass
// (never in a URL the server sees, never in an argument), git asks for it only
// for the host the fetch was pointed at, a missing credential is an error and
// never a prompt, and a repository on a registered host gets that host's read
// login while a refusal from the host falls back to an anonymous fetch.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use talaria_apps::git::{GitAuth, fetch_into, remote_head};
use talaria_apps::lifecycle::{Credentials, auth_for};
use talaria_apps::source::{GitSource, parse_source};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[path = "support/git.rs"]
mod support_git;
use support_git::git;

const TOKEN: &str = "tok-0123456789abcdef";

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "talaria-gitcred-{tag}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A bare repository `<root>/crm.git` holding one app commit; its sha.
fn bare_repo(root: &Path) -> String {
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet", "-b", "main"]);
    std::fs::write(work.join("talaria.json"), r#"{"name":"CRM","sdk":"^1.0"}"#).unwrap();
    std::fs::write(work.join("server.ts"), "export default {}\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "--quiet", "-m", "first"]);
    let sha = git(&work, &["rev-parse", "HEAD"]);
    git(root, &["clone", "--quiet", "--bare", "work", "crm.git"]);
    sha
}

#[derive(Default)]
struct Seen {
    /// Every request line (method and target) the server received.
    lines: Vec<String>,
    /// Every Authorization header value, decoded.
    logins: Vec<String>,
}

/// Serve `root` over smart HTTP on loopback, admitting only `user:TOKEN`.
async fn serve(root: PathBuf, seen: Arc<Mutex<Seen>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let root = root.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let _ = handle(&mut sock, &root, &seen).await;
            });
        }
    });
    port
}

async fn handle(
    sock: &mut tokio::net::TcpStream,
    root: &Path,
    seen: &Mutex<Seen>,
) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut body: Vec<u8> = buf[head_end + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let header = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    // The body: by length, or chunked.
    if let Some(len) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        while body.len() < len {
            let n = sock.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
    } else if header("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        let mut raw = body;
        loop {
            if raw.windows(5).any(|w| w == b"0\r\n\r\n") {
                break;
            }
            let n = sock.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
        }
        let mut out = Vec::new();
        let mut rest = &raw[..];
        while let Some(i) = rest.windows(2).position(|w| w == b"\r\n") {
            let size =
                usize::from_str_radix(String::from_utf8_lossy(&rest[..i]).trim(), 16).unwrap_or(0);
            if size == 0 {
                break;
            }
            out.extend_from_slice(&rest[i + 2..i + 2 + size]);
            rest = &rest[i + 2 + size + 2..];
        }
        body = out;
    }

    let login = header("authorization")
        .and_then(|v| v.strip_prefix("Basic ").map(str::to_string))
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    {
        let mut s = seen.lock().unwrap();
        s.lines.push(request_line.clone());
        if let Some(l) = &login {
            s.logins.push(l.clone());
        }
    }
    let admitted = login
        .as_deref()
        .is_some_and(|l| l.ends_with(&format!(":{TOKEN}")));
    if !admitted {
        sock.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"git\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await?;
        return Ok(());
    }

    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let mut cgi = tokio::process::Command::new("git");
    cgi.arg("http-backend")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REQUEST_METHOD", &method)
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("CONTENT_TYPE", header("content-type").unwrap_or_default())
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("REMOTE_USER", "test")
        .env("REMOTE_ADDR", "127.0.0.1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(p) = header("git-protocol") {
        cgi.env("GIT_PROTOCOL", p);
    }
    if let Some(e) = header("content-encoding") {
        cgi.env("HTTP_CONTENT_ENCODING", e);
    }
    let mut child = cgi.spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&body).await?;
    drop(stdin);
    let out = child.wait_with_output().await?;
    let raw = out.stdout;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)))
        .unwrap_or((0, 0));
    let cgi_head = String::from_utf8_lossy(&raw[..split.0]).into_owned();
    let cgi_body = &raw[split.0 + split.1..];
    let mut status = "200 OK".to_string();
    let mut resp = Vec::new();
    for l in cgi_head.lines() {
        if let Some(s) = l.strip_prefix("Status:") {
            status = s.trim().to_string();
        } else if !l.trim().is_empty() {
            resp.extend_from_slice(l.trim().as_bytes());
            resp.extend_from_slice(b"\r\n");
        }
    }
    let mut msg = format!("HTTP/1.1 {status}\r\n").into_bytes();
    msg.extend_from_slice(&resp);
    msg.extend_from_slice(
        format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            cgi_body.len()
        )
        .as_bytes(),
    );
    msg.extend_from_slice(cgi_body);
    sock.write_all(&msg).await?;
    Ok(())
}

/// Credentials as a test sets them: one registered host's answer (at
/// github.com — any other origin has no host registered) and one named
/// secret.
struct Mock {
    github: Result<Option<String>, String>,
    secret: Option<(&'static str, String, String)>,
    asked: Mutex<Vec<String>>,
}

impl Credentials for Mock {
    async fn host_read_login(&self, src: &GitSource) -> Result<Option<(String, String)>, String> {
        self.asked
            .lock()
            .unwrap()
            .push(format!("host:{}/{}", src.host, src.repo));
        if src.host != "github.com" {
            return Ok(None);
        }
        self.github
            .clone()
            .map(|t| t.map(|t| ("x-access-token".to_string(), t)))
    }

    async fn secret_login(&self, name: &str, host: &str) -> Result<(String, String), String> {
        self.asked
            .lock()
            .unwrap()
            .push(format!("secret:{name}@{host}"));
        match &self.secret {
            Some((n, u, p)) if *n == name => Ok((u.clone(), p.clone())),
            _ => Err(format!("there is no workspace secret named \"{name}\"")),
        }
    }
}

fn mock(secret_value: &str) -> Mock {
    Mock {
        github: Ok(None),
        secret: Some(("deploy", "x-access-token".into(), secret_value.into())),
        asked: Mutex::new(Vec::new()),
    }
}

#[tokio::test]
async fn a_named_secret_reaches_git_through_askpass_and_never_a_url() {
    let root = scratch("secret");
    let sha = bare_repo(&root);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let port = serve(root.clone(), seen.clone()).await;
    let src = parse_source(&format!("http://127.0.0.1:{port}/crm.git"), true).unwrap();
    let creds = mock(TOKEN);

    let (auth, note) = auth_for(&creds, &src, Some("deploy")).await.unwrap();
    assert!(note.is_none());
    assert!(matches!(&auth, GitAuth::Login { host, .. } if host == "127.0.0.1"));
    let dest = root.join("fetched");
    let got = fetch_into(&dest, &src, None, &auth)
        .await
        .expect("an authorized fetch");
    assert_eq!(got, sha);
    assert!(dest.join("talaria.json").is_file());
    // The update check reads the same ref through the same login.
    assert_eq!(
        remote_head(&src, None, &auth).await.unwrap().as_deref(),
        Some(sha.as_str())
    );
    assert_eq!(
        remote_head(&src, Some("main"), &auth)
            .await
            .unwrap()
            .as_deref(),
        Some(sha.as_str())
    );

    let s = seen.lock().unwrap();
    assert!(
        s.logins
            .iter()
            .any(|l| l == &format!("x-access-token:{TOKEN}")),
        "{:?}",
        s.logins
    );
    for line in &s.lines {
        assert!(
            !line.contains(TOKEN),
            "the token rode in a request line: {line}"
        );
    }
    // Nothing kept it: no credential helper wrote it into the fetched repo.
    let config = std::fs::read_to_string(dest.join(".git/config")).unwrap();
    assert!(!config.contains(TOKEN));
    drop(s);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn without_a_credential_the_fetch_fails_instead_of_prompting() {
    let root = scratch("anon");
    bare_repo(&root);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let port = serve(root.clone(), seen.clone()).await;
    let src = parse_source(&format!("http://127.0.0.1:{port}/crm.git"), true).unwrap();
    let dest = root.join("fetched");
    let started = std::time::Instant::now();
    let err = fetch_into(&dest, &src, None, &GitAuth::Anonymous)
        .await
        .expect_err("a private repository without a credential");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "it waited on a prompt"
    );
    assert!(
        err.contains("terminal prompts disabled") || err.contains("could not read Username"),
        "{err}"
    );
    assert!(!dest.exists(), "a failed fetch leaves nothing behind");
    // A wrong token is refused by the server, and the error never repeats it.
    let wrong = GitAuth::Login {
        host: "127.0.0.1".into(),
        username: "x-access-token".into(),
        password: "not-the-token".into(),
    };
    let err = fetch_into(&dest, &src, None, &wrong)
        .await
        .expect_err("wrong token");
    assert!(!err.contains("not-the-token"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn askpass_answers_only_for_the_host_the_fetch_was_pointed_at() {
    let root = scratch("host");
    bare_repo(&root);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let port = serve(root.clone(), seen.clone()).await;
    let src = parse_source(&format!("http://127.0.0.1:{port}/crm.git"), true).unwrap();
    // A login meant for another host: git asks 127.0.0.1, askpass says nothing.
    let elsewhere = GitAuth::Login {
        host: "github.com".into(),
        username: "x-access-token".into(),
        password: TOKEN.into(),
    };
    let err = fetch_into(&root.join("fetched"), &src, None, &elsewhere)
        .await
        .expect_err("askpass refused another host");
    assert!(!err.contains(TOKEN));
    assert!(
        seen.lock()
            .unwrap()
            .logins
            .iter()
            .all(|l| !l.contains(TOKEN)),
        "the token reached a host it was not for"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A host typed with capitals: the credential is minted for the lowercased
/// host, and git must be pointed at that same host, or askpass (which matches
/// the host exactly) never answers and the fetch dies on a disabled prompt.
#[tokio::test]
async fn a_mixed_case_host_still_gets_its_credential() {
    let root = scratch("case");
    let sha = bare_repo(&root);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let port = serve(root.clone(), seen.clone()).await;
    let src = parse_source(&format!("http://LocalHost:{port}/crm.git"), true).unwrap();
    assert_eq!(src.host, "localhost");
    let creds = mock(TOKEN);
    let (auth, _) = auth_for(&creds, &src, Some("deploy")).await.unwrap();
    let got = fetch_into(&root.join("fetched"), &src, None, &auth)
        .await
        .expect("the credential reached git for a mixed-case host");
    assert_eq!(got, sha);
    assert_eq!(
        remote_head(&src, None, &auth).await.unwrap().as_deref(),
        Some(sha.as_str())
    );
    assert!(
        seen.lock()
            .unwrap()
            .logins
            .iter()
            .any(|l| l == &format!("x-access-token:{TOKEN}"))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_github_repository_gets_the_connections_read_token() {
    let src = parse_source("https://github.com/acme/talaria-app-crm.git", false).unwrap();
    let creds = Mock {
        github: Ok(Some("ghs_read".into())),
        secret: None,
        asked: Mutex::new(Vec::new()),
    };
    let (auth, note) = auth_for(&creds, &src, None).await.unwrap();
    assert!(note.is_none());
    match auth {
        GitAuth::Login {
            host,
            username,
            password,
        } => {
            assert_eq!(
                (host.as_str(), username.as_str(), password.as_str()),
                ("github.com", "x-access-token", "ghs_read")
            );
        }
        GitAuth::Anonymous => panic!("expected the read token"),
    }
    assert_eq!(
        *creds.asked.lock().unwrap(),
        ["host:github.com/acme/talaria-app-crm"]
    );

    // Not configured: anonymous, nothing to say. Refused: anonymous, and the
    // reason rides along for the error should the clone then fail.
    let none = Mock {
        github: Ok(None),
        secret: None,
        asked: Mutex::new(Vec::new()),
    };
    assert_eq!(
        auth_for(&none, &src, None).await.unwrap(),
        (GitAuth::Anonymous, None)
    );
    let refused = Mock {
        github: Err("GitHub installation token failed (422) for acme/talaria-app-crm".into()),
        secret: None,
        asked: Mutex::new(Vec::new()),
    };
    let (auth, note) = auth_for(&refused, &src, None).await.unwrap();
    assert_eq!(auth, GitAuth::Anonymous);
    assert!(note.unwrap().contains("422"));

    // An origin no host is registered at fetches anonymously; a named secret
    // wins over any registered host.
    let gl = parse_source("https://gitlab.example.com/acme/crm", false).unwrap();
    let creds = mock("glpat");
    assert_eq!(
        auth_for(&creds, &gl, None).await.unwrap().0,
        GitAuth::Anonymous
    );
    let (auth, _) = auth_for(&creds, &gl, Some("deploy")).await.unwrap();
    assert!(
        matches!(auth, GitAuth::Login { ref host, ref password, .. } if host == "gitlab.example.com" && password == "glpat")
    );
    // The registry was asked about the anonymous fetch (and had no host
    // there); the named secret was spent without asking it.
    assert_eq!(
        *creds.asked.lock().unwrap(),
        [
            "host:gitlab.example.com/acme/crm",
            "secret:deploy@gitlab.example.com"
        ]
    );
    let err = auth_for(&creds, &gl, Some("nope")).await.unwrap_err();
    assert!(err.contains("no workspace secret named"), "{err}");
}
