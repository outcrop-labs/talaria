// What the workbench routes' live tests share: a state on the scratch
// database, people with sessions, an agent, and a loopback GitHub. Each test
// makes its own rows under its own tag and removes them.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use talaria_session::{SessionUser, create_session};
use talaria_state::AppState;

pub(crate) async fn state() -> AppState {
    let cfg = talaria_config::Config::from_parts(
        std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the live tests"),
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into()),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        "5274".into(),
    )
    .expect("test config");
    let pg = sqlx::PgPool::connect(&cfg.database_url)
        .await
        .expect("DATABASE_URL reachable");
    AppState::new(pg, Arc::new(cfg))
}

/// A name no other test's rows share: twelve random letters and digits.
pub(crate) fn tag() -> String {
    talaria_session::random_token()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// A person with `role`, and the headers their browser sends.
pub(crate) async fn person(
    state: &AppState,
    tag: &str,
    who: &str,
    role: &str,
) -> (String, HeaderMap) {
    let sub = format!("wb-live:{tag}:{who}");
    let id: String = sqlx::query_scalar(
        "insert into users (sub, email, name, role) values ($1, $1, $2, $3) returning id::text",
    )
    .bind(&sub)
    .bind(who)
    .bind(role)
    .fetch_one(&state.pg)
    .await
    .expect("the user");
    let sid = create_session(
        state,
        &SessionUser {
            id: id.clone(),
            sub: sub.clone(),
            email: Some(sub),
            name: Some(who.to_string()),
            picture: None,
            role: role.to_string(),
            provider: "password".into(),
        },
    )
    .await
    .expect("REDIS_URL reachable");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("{}={sid}", talaria_session::SESSION_COOKIE)
            .parse()
            .unwrap(),
    );
    (id, headers)
}

/// A Developer agent `manager` manages. Returns its id.
pub(crate) async fn agent(state: &AppState, tag: &str, manager: &str) -> String {
    let model = format!("wb-live-{tag}");
    let id: String = sqlx::query_scalar(
        "insert into agent_defs (slug, department, model, display_name, developer) \
         values ($1, $1, $1, $2, true) returning id::text",
    )
    .bind(&model)
    .bind(format!("Workbench Live {tag}"))
    .fetch_one(&state.pg)
    .await
    .expect("the agent");
    sqlx::query("insert into agent_managers (agent_id, user_id) values ($1::uuid, $2::uuid)")
        .bind(&id)
        .bind(manager)
        .execute(&state.pg)
        .await
        .expect("the manager");
    id
}

pub(crate) async fn cleanup(state: &AppState, agents: &[&str], users: &[&str], hosts: &[&str]) {
    for a in agents {
        sqlx::query("delete from audit_log where target_id = $1")
            .bind(*a)
            .execute(&state.pg)
            .await
            .unwrap();
        sqlx::query("delete from agent_defs where id = $1::uuid")
            .bind(*a)
            .execute(&state.pg)
            .await
            .unwrap();
    }
    for u in users {
        sqlx::query("delete from users where id = $1::uuid")
            .bind(*u)
            .execute(&state.pg)
            .await
            .unwrap();
    }
    for h in hosts {
        sqlx::query("delete from git_hosts where id = $1")
            .bind(*h)
            .execute(&state.pg)
            .await
            .unwrap();
    }
}

pub(crate) async fn answer(res: Result<Response, Response>) -> (StatusCode, Value) {
    let res = res.unwrap_or_else(|r| r);
    let status = res.status();
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

/// Each request the stub answered: method, path, authorization, body.
pub(crate) type Log = Arc<Mutex<Vec<(String, String, String, String)>>>;

/// What the stub's App asks for and its installation holds: the default an
/// agent carries, and the keys the full-set tests give one (administration,
/// secrets, workflows, an organization key).
pub(crate) fn held() -> Value {
    json!({
        "administration": "write",
        "contents": "write",
        "metadata": "read",
        "organization_secrets": "read",
        "pull_requests": "write",
        "secrets": "write",
        "workflows": "write",
    })
}

/// GitHub for installation `inst` of an App, as far as a mint, a revoke and
/// the repository listing of `acme/widgets` need: the installation holds
/// [`held`]; each mint answers `tok-<n>` carrying what was posted.
pub(crate) async fn github(inst: &'static str) -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let log: Log = Arc::default();
    let seen = log.clone();
    let mints = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let (seen, mints) = (seen.clone(), mints.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, body) = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break (
                                text[..end].to_string(),
                                text[end + 4..end + 4 + len].to_string(),
                            );
                        }
                    }
                };
                let mut words = head.lines().next().unwrap_or_default().split(' ');
                let method = words.next().unwrap_or_default().to_string();
                let path = words.next().unwrap_or_default().to_string();
                let auth = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("authorization")
                            .then(|| v.trim().to_string())
                    })
                    .unwrap_or_default();
                seen.lock()
                    .unwrap()
                    .push((method.clone(), path.clone(), auth, body.clone()));
                let installation = format!("/app/installations/{inst}");
                let mint = format!("{installation}/access_tokens");
                let (status, answer) = match (method.as_str(), path.as_str()) {
                    ("GET", p) if p == installation => (
                        200,
                        json!({
                            "id": inst.parse::<i64>().unwrap(),
                            "account": { "login": "acme" },
                            "permissions": held(),
                        })
                        .to_string(),
                    ),
                    ("POST", p) if p == mint => {
                        let n = mints.fetch_add(1, Ordering::SeqCst);
                        let asked: Value = serde_json::from_str(&body).unwrap_or_default();
                        (
                            201,
                            json!({
                                "token": format!("tok-{n}"),
                                "expires_at": "2099-01-01T00:00:00Z",
                                "permissions": asked["permissions"],
                                "repository_selection": "selected",
                                "repositories": [{ "full_name": "acme/widgets" }],
                            })
                            .to_string(),
                        )
                    }
                    ("GET", "/installation/repositories?per_page=100") => (
                        200,
                        json!({ "repositories": [{ "full_name": "acme/widgets" }] }).to_string(),
                    ),
                    ("DELETE", "/installation/token") => (204, String::new()),
                    _ => (500, r#"{"message":"unexpected in this test"}"#.to_string()),
                };
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (base, log)
}

/// A GitHub App host at `api`, connected through installation `inst` with
/// the App key sealed by this state's secretbox, and a record that says the
/// App asks for (and the installation holds) [`held`]. Returns its id.
pub(crate) async fn github_host(state: &AppState, tag: &str, api: &str, inst: &str) -> String {
    let sb = state.secretbox().await.expect("secretbox loads");
    let pem = include_str!("../../../../tests/it/fixtures/github-jwt-traditional.pem");
    let id = format!("wb-live-{tag}");
    let auth = json!({
        "mode": "app",
        "app": {
            "appId": "12345",
            "installationIds": [inst],
            "privateKeyEnc": sb.seal(pem).expect("the key seals"),
        },
    });
    sqlx::query(
        "insert into git_hosts (id, kind, name, base_url, api_url, auth) \
         values ($1, 'github', $1, $2, $3, $4)",
    )
    .bind(&id)
    .bind(format!("https://{id}.example.invalid"))
    .bind(api)
    .bind(auth)
    .execute(&state.pg)
    .await
    .expect("the host");
    let all = held();
    sqlx::query(
        "insert into git_host_installations \
           (host_id, installation_id, account, app_permissions, held, checked_at) \
         values ($1, $2, 'acme', $3, $3, now())",
    )
    .bind(&id)
    .bind(inst.parse::<i64>().unwrap())
    .bind(all)
    .execute(&state.pg)
    .await
    .expect("the record");
    id
}

pub(crate) fn calls(log: &Log, method: &str, path: &str) -> Vec<String> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(m, p, _, _)| m == method && p == path)
        .map(|(_, _, auth, _)| auth.clone())
        .collect()
}

/// The bodies of each `method` request to `path`, in order.
pub(crate) fn bodies(log: &Log, method: &str, path: &str) -> Vec<String> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(m, p, _, _)| m == method && p == path)
        .map(|(_, _, _, body)| body.clone())
        .collect()
}
