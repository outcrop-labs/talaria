// Live-DB proof of the catalog's liveness reads (cargo test -- --ignored).
// Search's dead-last ORDER BY, its "hide the dead unless asked" filter and
// the hidden-dead count are SQL, and the status counts partition on a
// column the upsert rewrites — none of which a unit test can vouch for,
// because their guarantees ARE Postgres's. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-mcp-library --test catalog_live -- --ignored
//
// Each test builds its OWN schema (a copy of the mirror's table and of
// app_settings, indexes and generated `search` column included) and points
// the pool's search_path at it alone, so the real mirror and the real
// settings are never read or written, and concurrent tests never meet.

use std::str::FromStr;

use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use talaria_mcp_library::catalog::{self, CatalogHit, SearchFilters, SearchPage};
use talaria_mcp_library::featured::{FeaturedConfig, StandIn};
use talaria_mcp_library::{LibraryServer, Tier, library};

fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://talaria:talaria@127.0.0.1:5432/talaria".into())
}

/// A throwaway schema shaped like the mirror, and a pool that sees only it.
async fn scratch(tag: &str) -> (PgPool, PgPool, String) {
    let admin = PgPool::connect(&database_url())
        .await
        .expect("dev database");
    let schema = format!("mcp_catalog_live_{tag}_{}", std::process::id());
    // A failed assertion skips the drop at the end of its test, so sweep
    // what earlier runs of this tag left before making a new one.
    let prefix = format!("mcp_catalog_live_{tag}_");
    let stale: Vec<String> =
        sqlx::query_scalar("select nspname::text from pg_namespace where starts_with(nspname, $1)")
            .bind(&prefix)
            .fetch_all(&admin)
            .await
            .expect("list scratch schemas");
    for old in stale {
        drop_scratch(&admin, &old).await;
    }
    for sql in [
        format!("create schema {schema}"),
        format!(
            "create table {schema}.mcp_library_entries \
             (like public.mcp_library_entries including all)"
        ),
        format!("create table {schema}.app_settings (like public.app_settings including all)"),
        // Before the first walk completes, an empty search asks the live
        // registry; these tests are about the mirror alone.
        format!(
            "insert into {schema}.app_settings (key, value) \
             values ('mcp_library_sync', '{{\"fullDoneAt\":\"2026-01-01T00:00:00Z\"}}'::jsonb)"
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(&admin)
            .await
            .expect(&sql);
    }
    let opts = PgConnectOptions::from_str(&database_url())
        .expect("DATABASE_URL parses")
        .options([("search_path", schema.as_str())]);
    let pg = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .expect("scratch pool");
    (pg, admin, schema)
}

async fn drop_scratch(admin: &PgPool, schema: &str) {
    // AssertSqlSafe: the schema name is this file's own tag and a pid.
    let sql = format!("drop schema if exists {schema} cascade");
    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .execute(admin)
        .await
        .unwrap();
}

struct Row<'a> {
    name: &'a str,
    title: &'a str,
    domain: &'a str,
    tier: Tier,
    /// None = never checked.
    state: Option<&'a str>,
    package: bool,
}

async fn insert(pg: &PgPool, r: Row<'_>) {
    let server = LibraryServer {
        registry_name: r.name.into(),
        title: r.title.into(),
        description: Some(format!("{} for the catalog live test", r.title)),
        url: if r.package {
            format!("pkg:{}", r.name)
        } else {
            format!("https://mcp.{}/mcp", r.domain)
        },
        domain: Some(r.domain.into()),
        icon: None,
        tier: r.tier,
        required_headers: Vec::new(),
        package: None,
    };
    // `kind` is the filter search reads; the entry's package body is not.
    let entry = serde_json::to_value(&server).unwrap();
    sqlx::query(
        "insert into mcp_library_entries \
           (registry_name, source, title, description, domain, domains, tier, kind, entry, \
            live_state, live_checked_at, live_detail) \
         values ($1, 'registry', $2, $3, $4, array[$4], $5, $6, $7, $8::text, \
                 case when $8::text is null then null else now() end, \
                 case when $8::text is null then null else $8::text || ' detail' end)",
    )
    .bind(&server.registry_name)
    .bind(&server.title)
    .bind(&server.description)
    .bind(r.domain)
    .bind(r.tier.rank())
    .bind(if r.package { "package" } else { "hosted" })
    .bind(entry)
    .bind(r.state)
    .execute(pg)
    .await
    .unwrap();
}

fn names(page: &SearchPage) -> Vec<&str> {
    page.hits
        .iter()
        .map(|h| h.server.registry_name.as_str())
        .collect()
}

fn state_of<'a>(page: &'a SearchPage, name: &str) -> Option<&'a str> {
    page.hits
        .iter()
        .find(|h| h.server.registry_name == name)
        .and_then(|h| h.live.as_ref())
        .map(|l| l.state.as_str())
}

fn filters(include_dead: bool) -> SearchFilters {
    SearchFilters {
        include_dead,
        ..SearchFilters::default()
    }
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn search_hides_dead_rows_counts_them_and_sinks_them_when_asked() {
    let (pg, admin, schema) = scratch("search").await;
    let lib = library();
    // Tiers are chosen so every live row is separated by a key the ranking
    // reads before text rank (brand, title prefix, tier), making the order
    // exact. The dead row is the BEST match by every other key — the brand match,
    // first-party, the title prefix — so only the dead key can put it last.
    for r in [
        Row {
            name: "acme/dead",
            title: "Acme",
            domain: "acme.com",
            tier: Tier::FirstParty,
            state: Some("dead"),
            package: false,
        },
        Row {
            name: "acme/ok",
            title: "Acme tools",
            domain: "acme-tools.dev",
            tier: Tier::FirstParty,
            state: Some("ok"),
            package: false,
        },
        Row {
            name: "acme/auth",
            title: "Acme auth",
            domain: "acme-auth.dev",
            tier: Tier::Community,
            state: Some("auth"),
            package: false,
        },
        Row {
            name: "acme/unknown",
            title: "Zed acme",
            domain: "zed.dev",
            tier: Tier::Verified,
            state: Some("unknown"),
            package: false,
        },
        Row {
            name: "acme/never",
            title: "Zz acme",
            domain: "zz.dev",
            tier: Tier::Community,
            state: None,
            package: false,
        },
        Row {
            name: "acme/dead2",
            title: "Acme two",
            domain: "acme-two.dev",
            tier: Tier::Verified,
            state: Some("dead"),
            package: false,
        },
        Row {
            name: "acme/pkg",
            title: "Acme package",
            domain: "acme-pkg.dev",
            tier: Tier::Verified,
            state: None,
            package: true,
        },
        Row {
            name: "other/thing",
            title: "Unrelated",
            domain: "unrelated.dev",
            tier: Tier::FirstParty,
            state: Some("dead"),
            package: false,
        },
    ] {
        insert(&pg, r).await;
    }

    // Default: the dead are left out and counted — only the ones that matched.
    let page = catalog::search(&pg, &lib, "acme", &filters(false)).await;
    assert_eq!(
        names(&page),
        [
            "acme/ok",
            "acme/pkg",
            "acme/auth",
            "acme/unknown",
            "acme/never"
        ],
        "{page:?}"
    );
    assert_eq!(
        page.hidden_dead, 2,
        "acme/dead and acme/dead2, not other/thing"
    );
    assert_eq!(
        state_of(&page, "acme/never"),
        None,
        "never checked has no verdict"
    );
    assert_eq!(state_of(&page, "acme/auth"), Some("auth"));

    // includeDead: the same order, then the dead — in their own rank order
    // (first-party brand match before the verified one) — flagged, and
    // nothing reported hidden.
    let page = catalog::search(&pg, &lib, "acme", &filters(true)).await;
    assert_eq!(
        names(&page),
        [
            "acme/ok",
            "acme/pkg",
            "acme/auth",
            "acme/unknown",
            "acme/never",
            "acme/dead",
            "acme/dead2"
        ],
        "{page:?}"
    );
    assert_eq!(page.hidden_dead, 0);
    assert!(page.hits.iter().rev().take(2).all(CatalogHit::is_dead));
    let dead = page
        .hits
        .iter()
        .find(|h| h.server.registry_name == "acme/dead")
        .unwrap();
    let live = dead.live.as_ref().unwrap();
    assert_eq!(live.detail.as_deref(), Some("dead detail"));
    assert!(
        live.checked_at.is_some_and(|t| t > 1_700_000_000_000),
        "epoch ms: {live:?}"
    );

    // The limit applies to what is shown; the count still reads every match.
    let page = catalog::search(
        &pg,
        &lib,
        "acme",
        &SearchFilters {
            limit: Some(2),
            ..filters(false)
        },
    )
    .await;
    assert_eq!(names(&page), ["acme/ok", "acme/pkg"]);
    assert_eq!(page.hidden_dead, 2);
    // With includeDead and a short limit the dead never push a live row out.
    let page = catalog::search(
        &pg,
        &lib,
        "acme",
        &SearchFilters {
            limit: Some(5),
            ..filters(true)
        },
    )
    .await;
    assert!(!page.hits.iter().any(CatalogHit::is_dead), "{page:?}");

    // The count honours the same filters as the page: kind and tier.
    let page = catalog::search(
        &pg,
        &lib,
        "acme",
        &SearchFilters {
            kind: Some("package".into()),
            ..filters(false)
        },
    )
    .await;
    assert_eq!(names(&page), ["acme/pkg"]);
    assert_eq!(page.hidden_dead, 0, "the dead rows are hosted");
    let page = catalog::search(
        &pg,
        &lib,
        "acme",
        &SearchFilters {
            max_tier: Some(Tier::FirstParty),
            ..filters(false)
        },
    )
    .await;
    assert_eq!(names(&page), ["acme/ok"]);
    assert_eq!(page.hidden_dead, 1, "only acme/dead is first-party");

    // Everything that matched is dead: an empty page that still says how
    // many — the picker's "Only unreachable servers match".
    let page = catalog::search(&pg, &lib, "unrelated", &filters(false)).await;
    assert!(page.hits.is_empty());
    assert_eq!(page.hidden_dead, 1);
    let page = catalog::search(&pg, &lib, "unrelated", &filters(true)).await;
    assert_eq!(names(&page), ["other/thing"]);
    assert_eq!(state_of(&page, "other/thing"), Some("dead"));

    // The empty query (the filter-only view) ranks by tier then title, the
    // dead still last and still counted.
    let page = catalog::search(&pg, &lib, "", &filters(false)).await;
    assert_eq!(page.hits.len(), 5);
    assert_eq!(page.hidden_dead, 3);
    let page = catalog::search(&pg, &lib, "", &filters(true)).await;
    assert_eq!(
        names(&page)[5..],
        ["acme/dead", "other/thing", "acme/dead2"],
        "dead last, in tier/title order among themselves"
    );

    drop_scratch(&admin, &schema).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn status_counts_partition_on_the_verdict() {
    let (pg, admin, schema) = scratch("status").await;
    for r in [
        Row {
            name: "s/ok",
            title: "Ok",
            domain: "ok.dev",
            tier: Tier::Community,
            state: Some("ok"),
            package: false,
        },
        Row {
            name: "s/auth",
            title: "Auth",
            domain: "auth.dev",
            tier: Tier::Community,
            state: Some("auth"),
            package: false,
        },
        Row {
            name: "s/dead",
            title: "Dead",
            domain: "dead.dev",
            tier: Tier::Community,
            state: Some("dead"),
            package: false,
        },
        Row {
            name: "s/unknown",
            title: "Unknown",
            domain: "unknown.dev",
            tier: Tier::Community,
            state: Some("unknown"),
            package: false,
        },
        Row {
            name: "s/never",
            title: "Never",
            domain: "never.dev",
            tier: Tier::Community,
            state: None,
            package: false,
        },
        Row {
            name: "s/pkg",
            title: "Pkg",
            domain: "pkg.dev",
            tier: Tier::Community,
            state: None,
            package: true,
        },
    ] {
        insert(&pg, r).await;
    }
    let l = catalog::status(&pg).await.liveness;
    assert_eq!(
        (l.checked, l.dead, l.unknown, l.pending),
        (4, 1, 1, 1),
        "{l:?}"
    );

    // An operator stand-in goes through the sync's own upsert. Probed dead,
    // then rewritten under the SAME url (a new title): it loses its stamp
    // (the job re-asks it first) but keeps the verdict search hides it by —
    // so it is checked and dead, not also pending. Never "unreachable >
    // checked".
    let mut cfg = FeaturedConfig {
        stand_ins: vec![StandIn {
            domain: "standin.dev".into(),
            title: "Stand-in".into(),
            url: "https://mcp.standin.dev/mcp".into(),
            description: None,
        }],
        ..FeaturedConfig::default()
    };
    catalog::write_operator_stand_ins(&pg, &cfg).await.unwrap();
    let l = catalog::status(&pg).await.liveness;
    assert_eq!(
        (l.checked, l.pending),
        (4, 2),
        "a new stand-in is pending: {l:?}"
    );
    sqlx::query(
        "update mcp_library_entries set live_state = 'dead', live_checked_at = now(), \
         live_detail = 'HTTP 404' where source = 'operator'",
    )
    .execute(&pg)
    .await
    .unwrap();
    cfg.stand_ins[0].title = "Stand-in, renamed".into();
    catalog::write_operator_stand_ins(&pg, &cfg).await.unwrap();
    let (state, stamped): (Option<String>, bool) = sqlx::query_as(
        "select live_state, live_checked_at is not null from mcp_library_entries \
         where source = 'operator'",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!((state.as_deref(), stamped), (Some("dead"), false));
    let l = catalog::status(&pg).await.liveness;
    assert_eq!(
        (l.checked, l.dead, l.unknown, l.pending),
        (5, 2, 1, 1),
        "{l:?}"
    );
    assert!(l.dead <= l.checked);

    // A changed URL is a different endpoint: the verdict goes, it is pending.
    cfg.stand_ins[0].url = "https://mcp2.standin.dev/mcp".into();
    catalog::write_operator_stand_ins(&pg, &cfg).await.unwrap();
    let l = catalog::status(&pg).await.liveness;
    assert_eq!((l.checked, l.dead, l.pending), (4, 1, 2), "{l:?}");

    // And the wire carries the same four numbers.
    let wire = catalog::status_wire(&catalog::status(&pg).await);
    assert_eq!(
        wire["liveness"],
        json!({ "checked": 4, "dead": 1, "unknown": 1, "pending": 2 })
    );

    drop_scratch(&admin, &schema).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_shelf_passes_over_a_dead_publisher_row() {
    let (pg, admin, schema) = scratch("shelf").await;
    let lib = library();
    // github.com, linear.app and asana.com are built-ins. GitHub's
    // first-party row answers; Linear's and Asana's only rows are dead, so
    // those domains leave the shelf and are counted — and with includeDead
    // they come back last, in shelf order.
    for r in [
        Row {
            name: "app.linear/mcp",
            title: "Linear",
            domain: "linear.app",
            tier: Tier::FirstParty,
            state: Some("dead"),
            package: false,
        },
        Row {
            name: "com.asana/mcp",
            title: "Asana",
            domain: "asana.com",
            tier: Tier::FirstParty,
            state: Some("dead"),
            package: false,
        },
        Row {
            name: "com.github/mcp",
            title: "GitHub",
            domain: "github.com",
            tier: Tier::FirstParty,
            state: Some("ok"),
            package: false,
        },
    ] {
        insert(&pg, r).await;
    }
    let page = catalog::featured(&pg, &lib, false).await;
    let shown = names(&page);
    assert!(shown.contains(&"com.github/mcp"), "{shown:?}");
    assert!(
        !shown.contains(&"app.linear/mcp") && !shown.contains(&"com.asana/mcp"),
        "{shown:?}"
    );
    assert_eq!(page.hidden_dead, 2);
    let page = catalog::featured(&pg, &lib, true).await;
    let shown = names(&page);
    assert_eq!(
        &shown[shown.len() - 2..],
        ["app.linear/mcp", "com.asana/mcp"],
        "dead last"
    );
    assert_eq!(page.hidden_dead, 0);

    drop_scratch(&admin, &schema).await;
}
