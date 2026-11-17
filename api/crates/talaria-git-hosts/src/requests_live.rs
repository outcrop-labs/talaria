// LIVE: an agent's GitHub permission requests against a real database — the
// one filing and its refusals, a second raise appending to the open request,
// an approval merging into the agent's set in one transaction (and a 409's
// cause when its grants went first), a grant removal withdrawing what is
// left with no repo on the host, and a request approved ahead of GitHub
// settling when a holdings refresh finds the installation holds it.
// `#[ignore]`d; ci.yml's api job runs it with the ignored lib tests in its
// live-DB steps.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-git-hosts --lib -- --ignored requests_live

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_git_host_types::{Level, PermissionSet};
use talaria_github::GithubConfig;

use crate::permissions::{self, default_set};
use crate::permissions_live::{agent, audit_rows, cleanup, github, grant, host, pg, sb, tag};
use crate::requests::{
    self, Ask, DecideError, Decider, Filing, OPEN, Raised, file_git_permission_request,
};
use crate::{RepoRef, holdings, set_granted_repos};

/// The agent's model, as the verb passes it.
async fn model_of(pg: &PgPool, agent: &str) -> String {
    sqlx::query_scalar("select model from agent_defs where id = $1::uuid")
        .bind(agent)
        .fetch_one(pg)
        .await
        .expect("the agent")
}

/// `agent` asks for `permissions` on `repo` (of `host`, when given).
async fn ask(
    pg: &PgPool,
    agent: &str,
    repo: &str,
    host: Option<&str>,
    permissions: &[(&str, &str)],
    why: &str,
) -> Result<Filing, String> {
    let model = model_of(pg, agent).await;
    let permissions: Vec<(String, String)> = permissions
        .iter()
        .map(|(k, l)| (k.to_string(), l.to_string()))
        .collect();
    file_git_permission_request(
        pg,
        &Ask {
            agent_id: agent,
            agent_model: &model,
            repo,
            host,
            permissions: &permissions,
            why,
            task_id: None,
        },
    )
    .await
}

/// The one request a filing made or added to.
fn only(filing: &Filing) -> (String, String, bool) {
    assert_eq!(filing.permissions.len(), 1, "{filing:?}");
    match &filing.permissions[0].raised {
        Raised::Filed { id, status } => (id.clone(), status.clone(), true),
        Raised::AlreadyOpen { id, status } => (id.clone(), status.clone(), false),
        Raised::Carried => panic!("carried, not filed: {filing:?}"),
    }
}

/// What the installation `inst` of `host` holds, as the record says.
async fn record(pg: &PgPool, host: &str, inst: &str, held: Value) {
    sqlx::query(
        "insert into git_host_installations \
           (host_id, installation_id, account, app_permissions, held, checked_at) \
         values ($1, $2, 'acme', $3, $3, now()) \
         on conflict (host_id, installation_id) do update set \
           held = excluded.held, app_permissions = excluded.app_permissions, checked_at = now()",
    )
    .bind(host)
    .bind(inst.parse::<i64>().unwrap())
    .bind(held)
    .execute(pg)
    .await
    .expect("the record");
}

async fn admin(pg: &PgPool, tag: &str) -> String {
    sqlx::query_scalar(
        "insert into users (sub, email, name, role) values ($1, $1, 'Admin', 'admin') returning id::text",
    )
    .bind(format!("ghp6:{tag}"))
    .fetch_one(pg)
    .await
    .expect("the admin")
}

async fn drop_user(pg: &PgPool, id: &str) {
    sqlx::query("delete from users where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await
        .expect("cleanup");
}

async fn status_of(pg: &PgPool, id: &str) -> String {
    sqlx::query_scalar("select status from workbench_permission_requests where id = $1::uuid")
        .bind(id)
        .fetch_one(pg)
        .await
        .expect("the request")
}

#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn an_agent_asks_and_only_an_unknown_key_an_ungranted_repo_or_a_hostwide_host_is_refused() {
    let pg = pg().await;
    let sb = sb();
    let t = tag();
    let a = agent(&pg, &t).await;
    let h = host(&pg, &sb, &t, "", "96001").await;
    grant(&pg, &a, &h, "acme/widgets").await;
    record(
        &pg,
        &h,
        "96001",
        json!({ "contents": "write", "metadata": "read", "pull_requests": "write" }),
    )
    .await;

    // A key or a level GitHub does not know.
    for bad in [
        ("sudo", "write"),
        ("workflows", "read"),
        ("workflows", "always"),
    ] {
        let why = ask(&pg, &a, "acme/widgets", None, &[bad], "x")
            .await
            .unwrap_err();
        assert!(why.contains(bad.0), "{bad:?}: {why}");
    }
    // A repo the agent is not granted.
    let why = ask(&pg, &a, "acme/other", None, &[("workflows", "write")], "x")
        .await
        .unwrap_err();
    assert!(why.contains("not granted to you"), "{why}");
    // A host whose own token decides: a PAT.
    let pat_host = format!("ghp6-pat-{t}");
    let pat = GithubConfig {
        mode: Some("pat".into()),
        pat_token_enc: Some(sb.seal("pat-test").unwrap()),
        app_id: String::new(),
        installation_ids: vec![],
        private_key_enc: None,
    };
    sqlx::query(
        "insert into git_hosts (id, kind, name, base_url, auth) values ($1, 'github', $1, $2, $3)",
    )
    .bind(&pat_host)
    .bind(format!("https://{pat_host}.example.invalid"))
    .bind(pat.to_auth())
    .execute(&pg)
    .await
    .unwrap();
    grant(&pg, &a, &pat_host, "acme/widgets").await;
    let why = ask(
        &pg,
        &a,
        "acme/widgets",
        Some(&pat_host),
        &[("workflows", "write")],
        "x",
    )
    .await
    .unwrap_err();
    assert!(why.contains("its own token decides"), "{why}");
    // Without `host`, the repo granted on two hosts names neither.
    let why = ask(
        &pg,
        &a,
        "acme/widgets",
        None,
        &[("workflows", "write")],
        "x",
    )
    .await
    .unwrap_err();
    assert!(why.contains("more than one host"), "{why}");

    // Administration and an organization key are asked for like any other:
    // no key is the agent's to be refused.
    let filing = ask(
        &pg,
        &a,
        "acme/widgets",
        Some(&h),
        &[
            ("administration", "write"),
            ("organization_secrets", "read"),
        ],
        "set up the deploy environment",
    )
    .await
    .expect("filed");
    assert_eq!(filing.host_id, h);
    assert_eq!(filing.permissions.len(), 2);
    for p in &filing.permissions {
        assert!(
            matches!(&p.raised, Raised::Filed { status, .. } if status == "pending"),
            "{p:?}"
        );
    }

    // A second raise appends to the open request: its words, its evidence,
    // and a higher level when it asks for more.
    let (id, status, filed) = only(
        &ask(
            &pg,
            &a,
            "acme/widgets",
            Some(&h),
            &[("secrets", "read")],
            "list the deploy secrets",
        )
        .await
        .unwrap(),
    );
    assert!(filed);
    assert_eq!(status, "pending");
    let (again, _, filed) = only(
        &ask(
            &pg,
            &a,
            "acme/widgets",
            Some(&h),
            &[("secrets", "write")],
            "and set DEPLOY_KEY",
        )
        .await
        .unwrap(),
    );
    assert!(!filed, "one open request per (agent, host, permission)");
    assert_eq!(again, id);
    let req = requests::get_request(&pg, &id).await.unwrap().unwrap();
    assert_eq!(req.level, "write", "raised to what the second raise asked");
    assert_eq!(req.why, "list the deploy secrets\n\nand set DEPLOY_KEY");
    assert_eq!(req.evidence.as_array().map(Vec::len), Some(2));
    assert_eq!(req.evidence[0]["source"], "agent");
    assert_eq!(req.evidence[0]["verified"], false);
    assert_eq!(req.evidence[1]["repo"], "acme/widgets");
    let open: i64 = sqlx::query_scalar(
        "select count(*) from workbench_permission_requests \
         where agent_id = $1::uuid and permission = 'secrets'",
    )
    .bind(&a)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(open, 1);

    // What the agent's set carries and the installation holds is carried:
    // nothing to ask. What it carries and GitHub lacks waits on GitHub.
    let carried = ask(
        &pg,
        &a,
        "acme/widgets",
        Some(&h),
        &[("contents", "write")],
        "push",
    )
    .await
    .unwrap();
    assert_eq!(carried.permissions[0].raised, Raised::Carried);
    let set = default_set().union(&PermissionSet::of(&[("checks", Level::Read)]));
    let from = permissions::host_permissions(&pg, &a, &h).await.unwrap();
    let change = permissions::PermissionsChange {
        actor: "requests-live",
        user_id: None,
        agent_label: "Requests Live",
        note: json!({}),
    };
    permissions::set_agent_host_permissions_cas(&pg, &a, &h, &from, Some(&set), &change)
        .await
        .unwrap();
    let (_, status, _) = only(
        &ask(
            &pg,
            &a,
            "acme/widgets",
            Some(&h),
            &[("checks", "read")],
            "read CI",
        )
        .await
        .unwrap(),
    );
    assert_eq!(status, "awaiting_github");

    // list_repos' read: newest first.
    let mine = requests::agent_requests(&pg, &a, 30).await.unwrap();
    assert_eq!(mine.len(), 4, "{mine:?}");
    assert_eq!(mine[0].permission, "checks");
    assert!(mine.iter().all(|r| OPEN.contains(&r.status.as_str())));

    // The agent goes first: its grant on the PAT host holds that host.
    cleanup(&pg, &[&a], &h).await;
    sqlx::query("delete from git_hosts where id = $1")
        .bind(&pat_host)
        .execute(&pg)
        .await
        .expect("cleanup");
}

#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn an_approval_merges_into_the_set_atomically_and_a_removed_grant_withdraws_what_is_left() {
    let pg = pg().await;
    let sb = sb();
    let t = tag();
    let a = agent(&pg, &t).await;
    let user = admin(&pg, &t).await;
    let by = Decider {
        actor: "admin@example.com",
        user_id: &user,
    };
    let h = host(&pg, &sb, &t, "", "96002").await;
    let r = |repo: &str| RepoRef {
        host_id: h.clone(),
        repo: repo.into(),
    };
    set_granted_repos(&pg, &a, &[r("acme/widgets"), r("acme/site")])
        .await
        .unwrap();
    record(
        &pg,
        &h,
        "96002",
        json!({ "contents": "write", "metadata": "read", "pull_requests": "write", "workflows": "write" }),
    )
    .await;
    let (id, _, _) = only(
        &ask(
            &pg,
            &a,
            "acme/widgets",
            None,
            &[("workflows", "write")],
            "CI",
        )
        .await
        .unwrap(),
    );

    // Approved: the key joins the default (no row before), the request is
    // granted (the installation holds it), and it applies to both repos.
    let decided = requests::approve(&pg, &id, &by, Some("for the deploy"))
        .await
        .expect("approved");
    assert_eq!(decided.request.status, "granted");
    assert_eq!(
        decided.request.decision_note.as_deref(),
        Some("for the deploy")
    );
    assert_eq!(decided.request.decided_by.as_deref(), Some(user.as_str()));
    assert_eq!(
        decided.permissions,
        Some(default_set().union(&PermissionSet::of(&[("workflows", Level::Write)])))
    );
    assert_eq!(decided.repos, ["acme/site", "acme/widgets"]);
    assert_eq!(
        permissions::host_permissions(&pg, &a, &h).await.unwrap(),
        decided.permissions.clone().unwrap()
    );
    let rows = audit_rows(&pg, "workbench.github_permission_grant", &a).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["status"], "granted");
    assert_eq!(rows[0]["appliesTo"], json!(["acme/site", "acme/widgets"]));
    // Decided once.
    assert_eq!(
        requests::approve(&pg, &id, &by, None).await.unwrap_err(),
        DecideError::NotOpen
    );
    assert_eq!(
        requests::deny(&pg, &id, &by, None).await.unwrap_err(),
        DecideError::NotOpen
    );

    // Removing one of two grants on the host leaves an open request open:
    // the permission is the host's.
    let (open, _, _) = only(
        &ask(
            &pg,
            &a,
            "acme/site",
            None,
            &[("secrets", "write")],
            "deploy key",
        )
        .await
        .unwrap(),
    );
    let change = set_granted_repos(&pg, &a, &[r("acme/widgets")])
        .await
        .unwrap();
    assert_eq!(change.removed, [r("acme/site")]);
    assert!(change.withdrawn.is_empty(), "{change:?}");
    assert_eq!(status_of(&pg, &open).await, "pending");

    // The grants going first: the approval finds nothing to apply to.
    sqlx::query("delete from workbench_repos where agent_id = $1::uuid")
        .bind(&a)
        .execute(&pg)
        .await
        .unwrap();
    assert_eq!(
        requests::approve(&pg, &open, &by, None).await.unwrap_err(),
        DecideError::GrantGone
    );
    assert_eq!(status_of(&pg, &open).await, "pending", "nothing written");
    assert!(
        !permissions::host_permissions(&pg, &a, &h)
            .await
            .unwrap()
            .contains("secrets")
    );

    // Through the grant write, the last grant on the host withdraws it.
    set_granted_repos(&pg, &a, &[r("acme/widgets")])
        .await
        .unwrap();
    let change = set_granted_repos(&pg, &a, &[]).await.unwrap();
    assert_eq!(change.withdrawn, [open.clone()]);
    assert_eq!(status_of(&pg, &open).await, "withdrawn");
    assert_eq!(
        requests::approve(&pg, &open, &by, None).await.unwrap_err(),
        DecideError::NotOpen
    );

    // A denial: its note is the agent's to read, its audit row says so.
    grant(&pg, &a, &h, "acme/widgets").await;
    let (denied, _, _) = only(
        &ask(
            &pg,
            &a,
            "acme/widgets",
            None,
            &[("actions", "write")],
            "rerun CI",
        )
        .await
        .unwrap(),
    );
    let decided = requests::deny(&pg, &denied, &by, Some("rerun it yourself in the UI"))
        .await
        .unwrap();
    assert_eq!(decided.request.status, "denied");
    let rows = audit_rows(&pg, "workbench.github_permission_deny", &a).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["note"], "rerun it yourself in the UI");

    cleanup(&pg, &[&a], &h).await;
    drop_user(&pg, &user).await;
}

#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn an_approval_ahead_of_github_waits_and_a_refresh_that_finds_it_held_grants_it() {
    let pg = pg().await;
    let sb = sb();
    let t = tag();
    let a = agent(&pg, &t).await;
    let other = agent(&pg, &format!("{t}o")).await;
    let user = admin(&pg, &t).await;
    let by = Decider {
        actor: "admin@example.com",
        user_id: &user,
    };
    let held = Arc::new(Mutex::new(
        json!({ "contents": "write", "metadata": "read", "pull_requests": "write" }),
    ));
    let (base, _log) = github("96003", held.clone(), None).await;
    let h = host(&pg, &sb, &t, &base, "96003").await;
    grant(&pg, &a, &h, "acme/widgets").await;
    grant(&pg, &other, &h, "acme/widgets").await;
    holdings::refresh_host(&pg, &sb, &h)
        .await
        .expect("recorded");

    let (id, _, _) = only(
        &ask(
            &pg,
            &a,
            "acme/widgets",
            None,
            &[("workflows", "write")],
            "CI",
        )
        .await
        .unwrap(),
    );
    let decided = requests::approve(&pg, &id, &by, None).await.unwrap();
    assert_eq!(
        decided.request.status, "awaiting_github",
        "the installation does not hold it"
    );
    assert!(decided.request.is_open(), "still in the census");

    // The other agent's request is approved too, and then an admin takes
    // the permission back out of its set before GitHub holds it.
    let (taken, _, _) = only(
        &ask(
            &pg,
            &other,
            "acme/widgets",
            None,
            &[("workflows", "write")],
            "CI",
        )
        .await
        .unwrap(),
    );
    requests::approve(&pg, &taken, &by, None).await.unwrap();
    let from = permissions::host_permissions(&pg, &other, &h)
        .await
        .unwrap();
    let change = permissions::PermissionsChange {
        actor: "requests-live",
        user_id: None,
        agent_label: "Requests Live",
        note: json!({}),
    };
    permissions::set_agent_host_permissions_cas(&pg, &other, &h, &from, None, &change)
        .await
        .unwrap();

    // A refresh that still finds it missing settles nothing for `a`, and
    // withdraws the other's.
    holdings::refresh_host(&pg, &sb, &h)
        .await
        .expect("recorded");
    assert_eq!(status_of(&pg, &id).await, "awaiting_github");
    assert_eq!(status_of(&pg, &taken).await, "withdrawn");

    // An org owner accepts it on GitHub: the next refresh grants it.
    *held.lock().unwrap() = json!({
        "contents": "write", "metadata": "read", "pull_requests": "write", "workflows": "write"
    });
    holdings::refresh_host(&pg, &sb, &h)
        .await
        .expect("recorded");
    assert_eq!(status_of(&pg, &id).await, "granted");
    let rows = audit_rows(&pg, "workbench.github_permission_request_granted", &a).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["request"], id.as_str());
    let rows = audit_rows(&pg, "workbench.github_permission_request_withdrawn", &other).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    // Settled once.
    assert_eq!(requests::settle_awaiting(&pg, &h).await, 0);

    cleanup(&pg, &[&a, &other], &h).await;
    drop_user(&pg, &user).await;
}
