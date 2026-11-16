// LIVE: an agent's GitHub permissions on a host against a real database and a
// loopback GitHub — the editor's compare-and-set and an approval's merge, the
// credential door's re-check racing a narrowing, the revoke here and its
// broadcast, the activation row a holdings refresh writes, and what a grant
// write reports. `#[ignore]`d; ci.yml's api job runs it with the ignored lib
// tests in its live-DB steps.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-git-hosts --lib -- --ignored permissions_live

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use talaria_agent_auth::now_ms;
use talaria_git_host_types::{Level, PermissionSet};
use talaria_github::GithubConfig;
use talaria_realtime::RealtimeDeps;
use talaria_secretbox::SecretBox;

use crate::permissions::{self, Cas, PermissionsChange};
use crate::registry::get_host;
use crate::{RepoRef, add_grant, grant_login, granted_repos, holdings, revoke, set_granted_repos};

/// Runs before GitHub answers mint `n` (0 for the first).
type Hook = Arc<dyn Fn(usize) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Each request the stub answered: method, path, authorization, body.
type Log = Arc<Mutex<Vec<(String, String, String, String)>>>;

/// GitHub for one installation of an App that asks for contents, workflows
/// and metadata: it holds `held` (changeable), each mint answers
/// `tok-<n>` carrying exactly what was posted for `acme/widgets`, after
/// `before_mint(n)` has run, and a revoke is a 204.
async fn github(
    inst: &'static str,
    held: Arc<Mutex<Value>>,
    before_mint: Option<Hook>,
) -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback listener binds");
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let log: Log = Arc::default();
    let seen = log.clone();
    let mints = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let (seen, held, mints, hook) = (
                seen.clone(),
                held.clone(),
                mints.clone(),
                before_mint.clone(),
            );
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
                let held_now = held.lock().unwrap().clone();
                let (status, answer) = match (method.as_str(), path.as_str()) {
                    ("GET", "/app") => (
                        200,
                        json!({
                            "name": "Talaria Live",
                            "permissions": { "contents": "write", "workflows": "write", "metadata": "read" },
                        })
                        .to_string(),
                    ),
                    ("GET", p) if p == installation => (
                        200,
                        json!({
                            "id": inst.parse::<i64>().unwrap(),
                            "account": { "login": "acme" },
                            "permissions": held_now,
                            "repository_selection": "selected",
                        })
                        .to_string(),
                    ),
                    ("POST", p) if p == mint => {
                        let n = mints.fetch_add(1, Ordering::SeqCst);
                        if let Some(hook) = &hook {
                            hook(n).await;
                        }
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

fn calls(log: &Log, method: &str, path_end: &str) -> Vec<(String, String)> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(m, p, _, _)| m == method && p.ends_with(path_end))
        .map(|(_, _, auth, body)| (auth.clone(), body.clone()))
        .collect()
}

fn posted(body: &str) -> Value {
    serde_json::from_str::<Value>(body).unwrap()["permissions"].clone()
}

async fn pg() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

fn sb() -> SecretBox {
    SecretBox::from_parts(
        [7u8; 32],
        HashMap::from([(1u32, talaria_secretbox::new_dek().expect("entropy"))]),
        Some(1),
    )
}

fn tag() -> String {
    static N: AtomicUsize = AtomicUsize::new(0);
    format!(
        "{}{}",
        now_ms() % 1_000_000_000,
        N.fetch_add(1, Ordering::SeqCst)
    )
}

/// A Developer agent of its own. Returns its id.
async fn agent(pg: &PgPool, tag: &str) -> String {
    let model = format!("ghp4-{tag}");
    sqlx::query_scalar(
        "insert into agent_defs (slug, department, model, display_name, developer) \
         values ($1, $1, $1, $2, true) returning id::text",
    )
    .bind(&model)
    .bind(format!("Permissions Live {tag}"))
    .fetch_one(pg)
    .await
    .expect("the agent row")
}

/// A GitHub App host at `api` using installation `inst` (empty `api`: a host
/// nothing is asked of). Returns its id.
async fn host(pg: &PgPool, sb: &SecretBox, tag: &str, api: &str, inst: &str) -> String {
    let pem = include_str!("../../../tests/it/fixtures/github-jwt-traditional.pem");
    let cfg = GithubConfig {
        mode: Some("app".into()),
        pat_token_enc: None,
        app_id: "12345".into(),
        installation_ids: vec![inst.into()],
        private_key_enc: Some(sb.seal(pem).expect("the key seals")),
    };
    let id = format!("ghp4-{tag}");
    sqlx::query(
        "insert into git_hosts (id, kind, name, base_url, api_url, auth) \
         values ($1, 'github', $1, $2, $3, $4)",
    )
    .bind(&id)
    .bind(format!("https://{id}.example.invalid"))
    .bind(if api.is_empty() { None } else { Some(api) })
    .bind(cfg.to_auth())
    .execute(pg)
    .await
    .expect("the host row");
    id
}

async fn grant(pg: &PgPool, agent: &str, host: &str, repo: &str) {
    add_grant(pg, agent, host, repo).await.expect("the grant");
}

/// Set what the agent holds on the host, from what it holds now.
async fn set(pg: &PgPool, agent: &str, host: &str, to: &PermissionSet) {
    let from = permissions::host_permissions(pg, agent, host)
        .await
        .expect("read");
    let change = PermissionsChange {
        actor: "permissions-live",
        user_id: None,
        agent_label: "Permissions Live",
        note: json!({}),
    };
    assert_eq!(
        permissions::set_agent_host_permissions_cas(pg, agent, host, &from, to, &change)
            .await
            .expect("the write"),
        Cas::Ok
    );
}

async fn audit_rows(pg: &PgPool, action: &str, agent: &str) -> Vec<Value> {
    sqlx::query_scalar::<_, Option<Value>>(
        "select after from audit_log where action = $1 and target_id = $2 order by created_at",
    )
    .bind(action)
    .bind(agent)
    .fetch_all(pg)
    .await
    .expect("read")
    .into_iter()
    .map(Option::unwrap_or_default)
    .collect()
}

async fn cleanup(pg: &PgPool, agents: &[&str], host: &str) {
    for a in agents {
        sqlx::query("delete from audit_log where target_id = $1")
            .bind(*a)
            .execute(pg)
            .await
            .expect("cleanup");
        sqlx::query("delete from agent_defs where id = $1::uuid")
            .bind(*a)
            .execute(pg)
            .await
            .expect("cleanup");
    }
    sqlx::query("delete from audit_log where target_id = $1")
        .bind(host)
        .execute(pg)
        .await
        .expect("cleanup");
    sqlx::query("delete from git_hosts where id = $1")
        .bind(host)
        .execute(pg)
        .await
        .expect("cleanup");
}

fn wf() -> PermissionSet {
    PermissionSet::of(&[("workflows", Level::Write)])
}

#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn the_editors_write_is_a_compare_and_set_with_its_audit_row_and_a_merge_only_raises() {
    let pg = pg().await;
    let sb = sb();
    let t = tag();
    let a = agent(&pg, &t).await;
    let h = host(&pg, &sb, &t, "", "94001").await;
    let user: String = sqlx::query_scalar(
        "insert into users (sub, email, name, role) values ($1, $1, 'Admin', 'admin') returning id::text",
    )
    .bind(format!("ghp4:{t}"))
    .fetch_one(&pg)
    .await
    .expect("the user");
    let change = |note: Value| PermissionsChange {
        actor: "admin@example.com",
        user_id: Some(user.as_str()),
        agent_label: "Permissions Live",
        note,
    };
    let none = PermissionSet::new();

    let wrote = permissions::set_agent_host_permissions_cas(
        &pg,
        &a,
        &h,
        &none,
        &wf(),
        &change(json!({ "acknowledged": true, "protection": [{ "repo": "acme/widgets", "state": "unprotected" }] })),
    )
    .await
    .expect("the write");
    assert_eq!(wrote, Cas::Ok);
    assert_eq!(
        permissions::host_permissions(&pg, &a, &h).await.unwrap(),
        wf()
    );
    let rows = permissions::agent_permissions(&pg, &a).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].updated_by.as_deref(), Some(user.as_str()));
    let audit = audit_rows(&pg, "workbench.github_permissions", &a).await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["host"], h.as_str());
    assert_eq!(audit[0]["permissions"], json!({ "workflows": "write" }));
    assert_eq!(audit[0]["acknowledged"], true);
    assert_eq!(audit[0]["protection"][0]["state"], "unprotected");

    // A second editor that read before the first wrote: refused, with what
    // is there now, and nothing written.
    let stale =
        permissions::set_agent_host_permissions_cas(&pg, &a, &h, &none, &none, &change(json!({})))
            .await
            .expect("the read");
    assert_eq!(stale, Cas::Changed(wf()));
    assert_eq!(
        audit_rows(&pg, "workbench.github_permissions", &a)
            .await
            .len(),
        1
    );

    // A merge adds a key, and never lowers one the agent holds.
    let mut tx = pg.begin().await.unwrap();
    let after = permissions::merge_agent_host_permission(
        &mut tx,
        &a,
        &h,
        "workflows",
        Level::Read,
        Some(&user),
    )
    .await
    .expect("the merge");
    assert_eq!(after, wf(), "write stays write");
    let after = permissions::merge_agent_host_permission(
        &mut tx,
        &a,
        &h,
        "actions",
        Level::Read,
        Some(&user),
    )
    .await
    .expect("the merge");
    tx.commit().await.unwrap();
    assert_eq!(
        after,
        wf().union(&PermissionSet::of(&[("actions", Level::Read)]))
    );
    // Two merges at once both land.
    let (pg1, pg2, a1, a2, h1, h2) = (
        pg.clone(),
        pg.clone(),
        a.clone(),
        a.clone(),
        h.clone(),
        h.clone(),
    );
    let one = tokio::spawn(async move {
        let mut tx = pg1.begin().await.unwrap();
        permissions::merge_agent_host_permission(&mut tx, &a1, &h1, "statuses", Level::Write, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    let two = tokio::spawn(async move {
        let mut tx = pg2.begin().await.unwrap();
        permissions::merge_agent_host_permission(&mut tx, &a2, &h2, "checks", Level::Read, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    one.await.unwrap();
    two.await.unwrap();
    let now = permissions::host_permissions(&pg, &a, &h).await.unwrap();
    assert_eq!(now.get("statuses"), Some(Level::Write));
    assert_eq!(now.get("checks"), Some(Level::Read));

    // Back to the base: the row goes.
    assert_eq!(
        permissions::set_agent_host_permissions_cas(&pg, &a, &h, &now, &none, &change(json!({})))
            .await
            .unwrap(),
        Cas::Ok
    );
    assert!(
        permissions::agent_permissions(&pg, &a)
            .await
            .unwrap()
            .is_empty()
    );

    sqlx::query("delete from users where id = $1::uuid")
        .bind(&user)
        .execute(&pg)
        .await
        .expect("cleanup");
    cleanup(&pg, &[&a], &h).await;
}

#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn a_mint_raced_by_a_narrowing_is_revoked_and_minted_again_against_the_grant_as_it_stands() {
    let pg = pg().await;
    let sb = sb();
    let t = tag();
    let a = agent(&pg, &t).await;
    // The narrowing lands while GitHub is minting: mint 0 finds the agent's
    // workflows taken back, mint 2 finds its grant gone.
    let ids: Arc<Mutex<Option<(String, String)>>> = Arc::default();
    let (hook_pg, hook_ids) = (pg.clone(), ids.clone());
    let hook: Hook = Arc::new(
        move |n: usize| -> Pin<Box<dyn Future<Output = ()> + Send>> {
            let (pg, ids) = (hook_pg.clone(), hook_ids.clone());
            Box::pin(async move {
                let Some((a, h)) = ids.lock().unwrap().clone() else {
                    return;
                };
                match n {
                    0 => {
                        sqlx::query("delete from agent_github_permissions where agent_id = $1::uuid and host_id = $2")
                        .bind(&a)
                        .bind(&h)
                        .execute(&pg)
                        .await
                        .unwrap();
                    }
                    2 => {
                        sqlx::query(
                        "delete from workbench_repos where agent_id = $1::uuid and host_id = $2",
                    )
                    .bind(&a)
                    .bind(&h)
                    .execute(&pg)
                    .await
                    .unwrap();
                    }
                    _ => {}
                }
            })
        },
    );
    let held = Arc::new(Mutex::new(
        json!({ "contents": "write", "workflows": "write", "metadata": "read" }),
    ));
    let (base, log) = github("94002", held, Some(hook)).await;
    let h = host(&pg, &sb, &t, &base, "94002").await;
    *ids.lock().unwrap() = Some((a.clone(), h.clone()));
    grant(&pg, &a, &h, "acme/widgets").await;
    set(&pg, &a, &h, &wf()).await;
    let row = get_host(&pg, &h).await.unwrap().expect("the host");

    let got = grant_login(&pg, &sb, &row, "acme/widgets", &a)
        .await
        .expect("the door answers")
        .expect("a credential");
    let mints = calls(&log, "POST", "/access_tokens");
    assert_eq!(mints.len(), 2, "{mints:?}");
    assert_eq!(
        posted(&mints[0].1),
        json!({ "contents": "write", "metadata": "read", "workflows": "write" }),
        "the first mint carried the grant as it was read"
    );
    assert_eq!(
        posted(&mints[1].1),
        json!({ "contents": "write", "metadata": "read" }),
        "the second carries the grant as it stands"
    );
    assert_eq!(
        calls(&log, "DELETE", "/installation/token")
            .into_iter()
            .map(|(auth, _)| auth)
            .collect::<Vec<_>>(),
        ["Bearer tok-0"],
        "the fresh token minted against the old grant is revoked"
    );
    assert_eq!(got.login.password, "tok-1");
    assert_eq!(got.carried.as_ref().and_then(|c| c.get("workflows")), None);

    // The grant itself goes while a mint is out: nothing is handed over,
    // and that token is revoked too.
    talaria_github::revoke_agent_tokens(Some(h.as_str()), &a, None).await;
    let gone = grant_login(&pg, &sb, &row, "acme/widgets", &a)
        .await
        .expect("the door answers");
    assert!(gone.is_none(), "a grant removed mid-mint answers nothing");
    let deleted: Vec<String> = calls(&log, "DELETE", "/installation/token")
        .into_iter()
        .map(|(auth, _)| auth)
        .collect();
    assert!(deleted.contains(&"Bearer tok-2".to_string()), "{deleted:?}");

    cleanup(&pg, &[&a], &h).await;
}

#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn a_narrowing_revokes_the_agents_tokens_here_and_tells_every_other_process() {
    let pg = pg().await;
    let sb = sb();
    let t = tag();
    let (a, other) = (agent(&pg, &t).await, agent(&pg, &format!("{t}o")).await);
    let held = Arc::new(Mutex::new(
        json!({ "contents": "write", "workflows": "write", "metadata": "read" }),
    ));
    let (base, log) = github("94003", held, None).await;
    let h = host(&pg, &sb, &t, &base, "94003").await;
    for agent in [&a, &other] {
        grant(&pg, agent, &h, "acme/widgets").await;
        set(&pg, agent, &h, &wf()).await;
    }
    let row = get_host(&pg, &h).await.unwrap().expect("the host");
    let mine = grant_login(&pg, &sb, &row, "acme/widgets", &a)
        .await
        .unwrap()
        .unwrap();
    let theirs = grant_login(&pg, &sb, &row, "acme/widgets", &other)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        mine.login.password, theirs.login.password,
        "one token per agent"
    );

    let published: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let sink = published.clone();
    let rt = RealtimeDeps {
        publish: Arc::new(move |topic: &str, payload: &str| {
            sink.lock().unwrap().push((topic.into(), payload.into()));
        }),
        subscribe: RealtimeDeps::publish_only(None).subscribe,
    };
    let out = revoke::revoke_agent_tokens(
        &pg,
        &rt,
        Some(h.as_str()),
        &a,
        None,
        "an admin took back workflows:write",
    )
    .await;
    assert_eq!((out.revoked, out.failed), (1, 0), "{out:?}");
    assert_eq!(
        calls(&log, "DELETE", "/installation/token")
            .into_iter()
            .map(|(auth, _)| auth)
            .collect::<Vec<_>>(),
        [format!("Bearer {}", mine.login.password)],
        "the other agent's token is untouched"
    );
    let published = published.lock().unwrap().clone();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].0, revoke::TOKEN_REVOKE_TOPIC);
    let heard = revoke::read_broadcast(&published[0].1).expect("a broadcast");
    assert_eq!(
        (heard.host.as_deref(), heard.agent.as_str()),
        (Some(h.as_str()), a.as_str())
    );
    let audit = audit_rows(&pg, "github.token_revoked", &a).await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["revoked"], 1);
    assert_eq!(audit[0]["carried"]["workflows"], "write");

    // Another process hearing the broadcast does the same with its own
    // cache: here, the token the agent was handed since.
    let again = grant_login(&pg, &sb, &row, "acme/widgets", &a)
        .await
        .unwrap()
        .unwrap();
    assert!(again.minted);
    let done = revoke::revoke_here(&pg, &heard).await;
    assert_eq!(done.revoked, 1);
    assert!(
        calls(&log, "DELETE", "/installation/token")
            .iter()
            .any(|(auth, _)| auth == &format!("Bearer {}", again.login.password))
    );

    cleanup(&pg, &[&a, &other], &h).await;
}

#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn an_installation_accepting_what_an_agent_was_given_writes_its_activation_row() {
    let pg = pg().await;
    let sb = sb();
    let t = tag();
    let a = agent(&pg, &t).await;
    let held = Arc::new(Mutex::new(
        json!({ "contents": "write", "metadata": "read" }),
    ));
    let (base, _log) = github("94004", held.clone(), None).await;
    let h = host(&pg, &sb, &t, &base, "94004").await;
    grant(&pg, &a, &h, "acme/widgets").await;
    set(&pg, &a, &h, &wf()).await;

    // The first read has nothing to compare with: no activation, and the
    // agent's workflows is dormant (given, not accepted).
    let first = holdings::refresh_host(&pg, &sb, &h)
        .await
        .expect("recorded");
    assert_eq!((first.checked, first.activated), (1, 0), "{first:?}");
    let recorded = holdings::recorded_holdings(&pg).await.unwrap();
    let extras = permissions::all_extras(&pg).await.unwrap();
    let dormant: Vec<_> = permissions::dormant_extras(&recorded, &extras)
        .into_iter()
        .filter(|d| d.agent_id == a)
        .collect();
    assert_eq!(dormant.len(), 1, "{dormant:?}");
    assert_eq!(dormant[0].missing, wf());
    assert_eq!(dormant[0].repos, ["acme/widgets"]);

    // An org owner accepts it.
    *held.lock().unwrap() =
        json!({ "contents": "write", "workflows": "write", "metadata": "read" });
    let second = holdings::refresh_host(&pg, &sb, &h)
        .await
        .expect("recorded");
    assert_eq!(second.activated, 1, "{second:?}");
    let rows = audit_rows(&pg, "workbench.github_permission_activated", &a).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["host"], h.as_str());
    assert_eq!(rows[0]["installation"], "94004");
    assert_eq!(rows[0]["activated"], json!({ "workflows": "write" }));
    assert_eq!(rows[0]["repos"], json!(["acme/widgets"]));
    let recorded = holdings::recorded_holdings(&pg).await.unwrap();
    let extras = permissions::all_extras(&pg).await.unwrap();
    assert!(
        permissions::dormant_extras(&recorded, &extras)
            .iter()
            .all(|d| d.agent_id != a),
        "accepted is no longer dormant"
    );

    cleanup(&pg, &[&a], &h).await;
}

#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn a_grant_write_says_what_it_added_and_removed_and_an_approval_adds_one() {
    let pg = pg().await;
    let sb = sb();
    let t = tag();
    let a = agent(&pg, &t).await;
    let h = host(&pg, &sb, &t, "", "94005").await;
    let r = |repo: &str| RepoRef {
        host_id: h.clone(),
        repo: repo.into(),
    };
    let first = set_granted_repos(&pg, &a, &[r("acme/b"), r("acme/a")])
        .await
        .unwrap();
    assert_eq!(first.added, [r("acme/a"), r("acme/b")]);
    assert!(first.removed.is_empty());
    let second = set_granted_repos(&pg, &a, &[r("acme/b"), r("acme/c")])
        .await
        .unwrap();
    assert_eq!(second.added, [r("acme/c")]);
    assert_eq!(second.removed, [r("acme/a")]);
    assert!(
        set_granted_repos(&pg, &a, &[r("acme/b"), r("acme/c")])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(add_grant(&pg, &a, &h, "acme/d").await.unwrap());
    assert!(
        !add_grant(&pg, &a, &h, "acme/d").await.unwrap(),
        "already granted"
    );
    assert_eq!(
        granted_repos(&pg, &a).await,
        [r("acme/b"), r("acme/c"), r("acme/d")],
        "an approval's grant leaves the others as they are"
    );
    cleanup(&pg, &[&a], &h).await;
}
