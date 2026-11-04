// The marketplace catalogs — the feeds Manage → Apps → Discover reads, as a
// list the operator fills. Each catalog is one URL serving a JSON index
// (`{"apps": [{slug, name, repo, …}]}`); Discover shows every enabled one's
// apps, each tagged with the catalog it came from.
//
// IT WAS ONE URL (`apps_catalog_url`) that defaulted to Outcrop's registry.
// The migration that created `app_catalogs` moved that value in as the first
// row, or seeded Outcrop's registry when there was none, so nothing an
// existing install showed went missing. That migration is the ONLY place a
// catalog is ever added for the operator: nothing here re-seeds, so an
// operator who removes every catalog gets an empty Discover tab that says so,
// and install-from-git keeps working without one.
//
// Every fetch goes through the SSRF guard (`talaria_safe_fetch`): a catalog
// URL is admin-typed, but it is still a URL this server dials.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;

use talaria_users::slug_ok;

use crate::source::local_sources_allowed;

/// The longest catalog name the list keeps.
pub const NAME_MAX: usize = 80;
/// The longest catalog URL the list keeps.
pub const URL_MAX: usize = 2000;
/// The most catalogs one instance lists — a runaway guard, not a quota.
pub const CATALOGS_MAX: usize = 50;

/// One catalog as the list serves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    /// Its place in the list: an app two catalogs both list is shown from
    /// the earlier one.
    pub position: i32,
    pub created_by: Option<String>,
}

type CatalogRow = (String, String, String, bool, i32, Option<String>);

impl From<CatalogRow> for Catalog {
    fn from(r: CatalogRow) -> Self {
        Catalog {
            id: r.0,
            name: r.1,
            url: r.2,
            enabled: r.3,
            position: r.4,
            created_by: r.5,
        }
    }
}

const COLS: &str = "id::text, name, url, enabled, position, created_by";

/// Why a catalog write was refused. `Invalid` and `Duplicate` are the
/// sentence the admin reads; `NotFound` names an id that is not in the list.
#[derive(Debug)]
pub enum CatalogError {
    Invalid(String),
    Duplicate(String),
    NotFound,
    Db(sqlx::Error),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::Invalid(m) | CatalogError::Duplicate(m) => f.write_str(m),
            CatalogError::NotFound => f.write_str("no such catalog"),
            CatalogError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl From<sqlx::Error> for CatalogError {
    fn from(e: sqlx::Error) -> Self {
        CatalogError::Db(e)
    }
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

/// A catalog URL as the list stores it: `https://` with a host, no
/// credential, no fragment — trimmed. With `local_ok` (the dev stack's
/// `TALARIA_APPS_LOCAL_SOURCES=1`, the same opt-in install-from-git reads),
/// plain `http://` to loopback too. The SSRF guard still decides at fetch
/// time whether the address may be dialled.
pub fn catalog_url_ok(raw: &str, local_ok: bool) -> Result<String, String> {
    let u = raw.trim();
    if u.is_empty() {
        return Err("a catalog needs a URL".into());
    }
    if u.chars().count() > URL_MAX {
        return Err(format!(
            "the catalog URL is longer than {URL_MAX} characters"
        ));
    }
    let https_only = || {
        if local_ok {
            "a catalog URL must be https://, or http:// to loopback".to_string()
        } else {
            "a catalog URL must be https://".to_string()
        }
    };
    let parsed = url::Url::parse(u).map_err(|_| https_only())?;
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    match parsed.scheme() {
        "https" => {}
        "http" if local_ok && is_loopback(&host) => {}
        _ => return Err(https_only()),
    }
    if host.is_empty() {
        return Err("a catalog URL must name a host".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(
            "the catalog URL carries a credential: a catalog is a public index, and its URL is shown to every admin"
                .into(),
        );
    }
    if parsed.fragment().is_some() {
        return Err("the catalog URL must not carry a fragment".into());
    }
    Ok(u.to_string())
}

/// A catalog name: trimmed, 1 to [`NAME_MAX`] characters.
pub fn catalog_name_ok(raw: &str) -> Result<String, String> {
    let n = raw.trim();
    if n.is_empty() {
        return Err("a catalog needs a name".into());
    }
    if n.chars().count() > NAME_MAX {
        return Err(format!("a catalog name is at most {NAME_MAX} characters"));
    }
    if n.chars().any(char::is_control) {
        return Err("a catalog name is one line of text".into());
    }
    Ok(n.to_string())
}

/// Every catalog, in list order.
pub async fn list_catalogs(pg: &PgPool) -> Result<Vec<Catalog>, sqlx::Error> {
    let rows: Vec<CatalogRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLS} from app_catalogs order by position, created_at, id"
    )))
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(Catalog::from).collect())
}

/// Add a catalog at the end of the list, enabled.
pub async fn create_catalog(
    pg: &PgPool,
    name: &str,
    url: &str,
    actor: &str,
) -> Result<Catalog, CatalogError> {
    let name = catalog_name_ok(name).map_err(CatalogError::Invalid)?;
    let url = catalog_url_ok(url, local_sources_allowed()).map_err(CatalogError::Invalid)?;
    let count: i64 = sqlx::query_scalar("select count(*) from app_catalogs")
        .fetch_one(pg)
        .await?;
    if count as usize >= CATALOGS_MAX {
        return Err(CatalogError::Invalid(format!(
            "this instance already lists {CATALOGS_MAX} catalogs — remove one first"
        )));
    }
    let row: Result<CatalogRow, sqlx::Error> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into app_catalogs (name, url, position, created_by) \
         values ($1, $2, (select coalesce(max(position) + 1, 0) from app_catalogs), $3) \
         returning {COLS}"
    )))
    .bind(&name)
    .bind(&url)
    .bind(actor)
    .fetch_one(pg)
    .await;
    match row {
        Ok(r) => Ok(r.into()),
        Err(e) if is_unique_violation(&e) => Err(CatalogError::Duplicate(format!(
            "{url} is already in the catalog list"
        ))),
        Err(e) => Err(e.into()),
    }
}

/// What an edit changes; `None` keeps the field.
#[derive(Debug, Default, Clone)]
pub struct CatalogPatch {
    pub name: Option<String>,
    pub url: Option<String>,
    pub enabled: Option<bool>,
}

/// Edit one catalog. The answer is the row as it now stands.
pub async fn update_catalog(
    pg: &PgPool,
    id: &str,
    patch: &CatalogPatch,
) -> Result<Catalog, CatalogError> {
    let name = match &patch.name {
        Some(n) => Some(catalog_name_ok(n).map_err(CatalogError::Invalid)?),
        None => None,
    };
    let url = match &patch.url {
        Some(u) => Some(catalog_url_ok(u, local_sources_allowed()).map_err(CatalogError::Invalid)?),
        None => None,
    };
    let row: Result<Option<CatalogRow>, sqlx::Error> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "update app_catalogs set name = coalesce($2, name), url = coalesce($3, url), \
               enabled = coalesce($4, enabled), updated_at = now() \
             where id = $1::uuid returning {COLS}"
        )))
        .bind(id)
        .bind(&name)
        .bind(&url)
        .bind(patch.enabled)
        .fetch_optional(pg)
        .await;
    match row {
        Ok(Some(r)) => Ok(r.into()),
        Ok(None) => Err(CatalogError::NotFound),
        Err(e) if is_unique_violation(&e) => Err(CatalogError::Duplicate(format!(
            "{} is already in the catalog list",
            url.unwrap_or_default()
        ))),
        Err(e) => Err(e.into()),
    }
}

/// Remove one catalog. The answer is the row that was removed. Removing the
/// last one leaves the list empty — nothing puts a default back.
pub async fn delete_catalog(pg: &PgPool, id: &str) -> Result<Catalog, CatalogError> {
    let row: Option<CatalogRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "delete from app_catalogs where id = $1::uuid returning {COLS}"
    )))
    .bind(id)
    .fetch_optional(pg)
    .await?;
    row.map(Catalog::from).ok_or(CatalogError::NotFound)
}

/// Put the list in `order`, which must name every catalog exactly once.
pub async fn reorder_catalogs(pg: &PgPool, order: &[String]) -> Result<Vec<Catalog>, CatalogError> {
    let mut tx = pg.begin().await?;
    let ids: Vec<String> = sqlx::query_scalar("select id::text from app_catalogs for update")
        .fetch_all(&mut *tx)
        .await?;
    let have: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let want: HashSet<&str> = order.iter().map(String::as_str).collect();
    if want.len() != order.len() || have != want {
        return Err(CatalogError::Invalid(
            "the order must name every catalog in the list exactly once".into(),
        ));
    }
    for (i, id) in order.iter().enumerate() {
        sqlx::query(
            "update app_catalogs set position = $2, updated_at = now() where id = $1::uuid",
        )
        .bind(id)
        .bind(i as i32)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(list_catalogs(pg).await?)
}

// ── Reading a catalog ────────────────────────────────────────────────────────

/// One catalog index's apps, admitted: a real slug, a name and an https repo,
/// everything else coerced to defaults. Anything that is not
/// `{"apps": [...]}` JSON is an error, never an empty catalog.
pub fn parse_catalog(body: &[u8]) -> Result<Vec<Value>, String> {
    let Ok(j) = serde_json::from_slice::<Value>(body) else {
        return Err("expected a JSON catalog".to_string());
    };
    let Some(apps) = j.get("apps").and_then(Value::as_array) else {
        return Err("expected a JSON catalog with an \"apps\" list".to_string());
    };
    let mut out = Vec::new();
    for a in apps {
        let field = |k: &str| a.get(k).and_then(|v| v.as_str()).map(String::from);
        let Some(slug) = field("slug").filter(|s| slug_ok(s)) else {
            continue;
        };
        let (Some(name), Some(repo)) = (field("name"), field("repo")) else {
            continue;
        };
        if !repo.starts_with("https://") {
            continue;
        }
        let mut obj = serde_json::Map::new();
        obj.insert("slug".into(), slug.into());
        obj.insert("name".into(), name.into());
        obj.insert(
            "icon".into(),
            field("icon").unwrap_or_else(|| "⬡".into()).into(),
        );
        obj.insert(
            "description".into(),
            field("description").unwrap_or_default().into(),
        );
        obj.insert("repo".into(), repo.into());
        obj.insert(
            "author".into(),
            field("author").unwrap_or_else(|| "community".into()).into(),
        );
        obj.insert(
            "official".into(),
            Value::Bool(a.get("official").and_then(Value::as_bool) == Some(true)),
        );
        // version is OMITTED unless present and non-empty.
        if let Some(v) = field("version").filter(|v| !v.is_empty()) {
            obj.insert("version".into(), v.into());
        }
        out.push(Value::Object(obj));
    }
    Ok(out)
}

/// Fetch and admit one catalog index, through the SSRF guard.
pub async fn fetch_one(url: &str) -> Result<Vec<Value>, String> {
    let resp = talaria_safe_fetch::safe_fetch(
        url,
        talaria_safe_fetch::SafeFetch {
            headers: vec![("accept", "application/json")],
            timeout_ms: Some(10_000),
            max_bytes: Some(2 * 1024 * 1024),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    if !(200..300).contains(&resp.status) {
        return Err(format!("catalog fetch failed ({})", resp.status));
    }
    parse_catalog(&resp.body)
}

/// One catalog in the Discover answer: the row, and what reading it gave.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogStatus {
    #[serde(flatten)]
    pub catalog: Catalog,
    /// How many of its apps it listed after admission. None when it was not
    /// read (disabled) or the read failed.
    pub apps: Option<usize>,
    /// Why it could not be read.
    pub error: Option<String>,
    /// Slugs it lists that an earlier catalog already listed: Discover shows
    /// those from the earlier one.
    pub shadowed: Vec<String>,
}

/// What Discover shows: every enabled catalog's apps, one per slug, each
/// tagged `catalog: {id, name}`; and each catalog's own status.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogFeed {
    pub apps: Vec<Value>,
    pub catalogs: Vec<CatalogStatus>,
}

/// Merge the read catalogs, in list order. A slug listed twice (by two
/// catalogs, or twice by one) is shown once, from the first; the later
/// listing is recorded as shadowed on its catalog. `None` is a catalog that
/// was not read (disabled).
pub fn merge_catalogs(read: Vec<(Catalog, Option<Result<Vec<Value>, String>>)>) -> CatalogFeed {
    let mut seen: HashSet<String> = HashSet::new();
    let mut apps = Vec::new();
    let mut catalogs = Vec::new();
    for (catalog, fetched) in read {
        let mut status = CatalogStatus {
            catalog,
            apps: None,
            error: None,
            shadowed: Vec::new(),
        };
        match fetched {
            None => {}
            Some(Err(e)) => status.error = Some(e),
            Some(Ok(list)) => {
                status.apps = Some(list.len());
                for mut app in list {
                    let slug = app
                        .get("slug")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if !seen.insert(slug.clone()) {
                        status.shadowed.push(slug);
                        continue;
                    }
                    if let Some(obj) = app.as_object_mut() {
                        obj.insert(
                            "catalog".into(),
                            serde_json::json!({
                                "id": status.catalog.id,
                                "name": status.catalog.name,
                            }),
                        );
                    }
                    apps.push(app);
                }
            }
        }
        catalogs.push(status);
    }
    CatalogFeed { apps, catalogs }
}

/// Read every enabled catalog at once and merge them ([`merge_catalogs`]).
/// One catalog failing is that catalog's error; the others still show.
pub async fn fetch_catalogs(pg: &PgPool) -> Result<CatalogFeed, sqlx::Error> {
    let list = list_catalogs(pg).await?;
    let reads = futures_util::future::join_all(list.iter().map(|c| async move {
        if c.enabled {
            Some(fetch_one(&c.url).await)
        } else {
            None
        }
    }))
    .await;
    Ok(merge_catalogs(list.into_iter().zip(reads).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cat(id: &str, name: &str, position: i32, enabled: bool) -> Catalog {
        Catalog {
            id: id.into(),
            name: name.into(),
            url: format!("https://{id}.example/index.json"),
            enabled,
            position,
            created_by: None,
        }
    }

    fn app(slug: &str) -> Value {
        json!({ "slug": slug, "name": slug, "repo": format!("https://git.example/{slug}") })
    }

    #[test]
    fn a_catalog_url_is_https_with_a_host_and_nothing_secret() {
        assert_eq!(
            catalog_url_ok("  https://raw.example/index.json ", false).unwrap(),
            "https://raw.example/index.json"
        );
        for bad in [
            "",
            "   ",
            "http://raw.example/index.json",
            "ftp://raw.example/index.json",
            "file:///tmp/index.json",
            "not a url",
            "https://user:pw@raw.example/index.json",
            "https://token@raw.example/index.json",
            "https://raw.example/index.json#apps",
        ] {
            assert!(catalog_url_ok(bad, false).is_err(), "{bad:?} was accepted");
        }
        let long = format!("https://raw.example/{}", "a".repeat(URL_MAX));
        assert!(catalog_url_ok(&long, false).is_err());
    }

    #[test]
    fn plain_http_is_loopback_only_and_only_with_local_sources_on() {
        assert!(catalog_url_ok("http://127.0.0.1:8090/index.json", false).is_err());
        assert!(catalog_url_ok("http://127.0.0.1:8090/index.json", true).is_ok());
        assert!(catalog_url_ok("http://localhost:8090/index.json", true).is_ok());
        assert!(catalog_url_ok("http://[::1]:8090/index.json", true).is_ok());
        assert!(catalog_url_ok("http://raw.example/index.json", true).is_err());
        assert!(catalog_url_ok("http://10.0.0.5/index.json", true).is_err());
        assert!(catalog_url_ok("file:///tmp/index.json", true).is_err());
    }

    #[test]
    fn a_catalog_name_is_one_short_line() {
        assert_eq!(catalog_name_ok("  Outcrop Labs ").unwrap(), "Outcrop Labs");
        assert!(catalog_name_ok("").is_err());
        assert!(catalog_name_ok("   ").is_err());
        assert!(catalog_name_ok("two\nlines").is_err());
        assert!(catalog_name_ok(&"n".repeat(NAME_MAX)).is_ok());
        assert!(catalog_name_ok(&"n".repeat(NAME_MAX + 1)).is_err());
    }

    #[test]
    fn a_catalog_admits_only_real_entries_and_says_when_it_is_not_one() {
        let body = json!({ "apps": [
            { "slug": "crm", "name": "CRM", "repo": "https://github.com/acme/crm", "official": true, "version": "1.2.0" },
            { "slug": "Bad Slug", "name": "x", "repo": "https://github.com/acme/x" },
            { "slug": "noname", "repo": "https://github.com/acme/noname" },
            { "slug": "plain", "name": "Plain", "repo": "http://github.com/acme/plain" },
            { "slug": "notes", "name": "Notes", "repo": "https://git.example/notes", "version": "" },
        ]})
        .to_string();
        let apps = parse_catalog(body.as_bytes()).unwrap();
        let slugs: Vec<&str> = apps.iter().map(|a| a["slug"].as_str().unwrap()).collect();
        assert_eq!(slugs, ["crm", "notes"]);
        assert_eq!(apps[0]["official"], json!(true));
        assert_eq!(apps[0]["version"], json!("1.2.0"));
        assert_eq!(apps[1]["icon"], json!("⬡"));
        assert_eq!(apps[1]["author"], json!("community"));
        assert!(
            apps[1].get("version").is_none(),
            "an empty version is omitted"
        );
        assert!(parse_catalog(b"<html>").is_err());
        assert!(parse_catalog(b"{\"items\": []}").is_err());
        assert_eq!(
            parse_catalog(b"{\"apps\": []}").unwrap(),
            Vec::<Value>::new()
        );
    }

    #[test]
    fn the_merge_keeps_list_order_tags_each_app_and_shows_a_slug_once() {
        let feed = merge_catalogs(vec![
            (
                cat("a", "First", 0, true),
                Some(Ok(vec![app("crm"), app("notes"), app("crm")])),
            ),
            (cat("b", "Off", 1, false), None),
            (
                cat("c", "Broken", 2, true),
                Some(Err("catalog fetch failed (404)".into())),
            ),
            (
                cat("d", "Second", 3, true),
                Some(Ok(vec![app("notes"), app("wiki")])),
            ),
        ]);
        let shown: Vec<(&str, &str)> = feed
            .apps
            .iter()
            .map(|a| {
                (
                    a["slug"].as_str().unwrap(),
                    a["catalog"]["name"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [("crm", "First"), ("notes", "First"), ("wiki", "Second")]
        );
        assert_eq!(feed.apps[2]["catalog"]["id"], json!("d"));

        let status: Vec<(&str, Option<usize>, Option<&str>, Vec<&str>)> = feed
            .catalogs
            .iter()
            .map(|s| {
                (
                    s.catalog.name.as_str(),
                    s.apps,
                    s.error.as_deref(),
                    s.shadowed.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            status,
            [
                ("First", Some(3), None, vec!["crm"]),
                ("Off", None, None, vec![]),
                ("Broken", None, Some("catalog fetch failed (404)"), vec![]),
                ("Second", Some(2), None, vec!["notes"]),
            ]
        );
    }

    #[test]
    fn no_catalogs_is_an_empty_feed_not_a_default() {
        let feed = merge_catalogs(Vec::new());
        assert!(feed.apps.is_empty());
        assert!(feed.catalogs.is_empty());
    }

    #[test]
    fn a_status_serializes_the_row_flat() {
        let s = CatalogStatus {
            catalog: cat("a", "First", 0, true),
            apps: Some(2),
            error: None,
            shadowed: vec![],
        };
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            json!({
                "id": "a", "name": "First", "url": "https://a.example/index.json",
                "enabled": true, "position": 0, "createdBy": null,
                "apps": 2, "error": null, "shadowed": [],
            })
        );
    }

    /// Two catalogs served from loopback, read by `fetch_one` the way
    /// `fetch_catalogs` reads each — through the SSRF guard, which admits
    /// loopback only when the operator allowlisted it.
    #[tokio::test]
    async fn two_loopback_catalogs_read_and_merge_through_the_guard() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn serve(body: String) -> u16 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                while let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 2048];
                    let _ = sock.read(&mut buf).await;
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(reply.as_bytes()).await;
                    let _ = sock.shutdown().await;
                }
            });
            port
        }
        let one = serve(json!({ "apps": [app("crm"), app("notes")] }).to_string()).await;
        let two = serve(json!({ "apps": [app("notes"), app("wiki")] }).to_string()).await;
        let url = |p: u16| format!("http://127.0.0.1:{p}/index.json");

        // Without the allowlist the guard refuses loopback: that is the
        // catalog's error, never an empty list.
        // SAFETY: this test is the only one in the crate that touches this
        // variable, and it sets it before any fetch it makes reads it.
        unsafe { std::env::remove_var("TALARIA_FETCH_ALLOW_HOSTS") };
        let refused = fetch_one(&url(one)).await.expect_err("loopback is refused");
        assert!(refused.contains("loopback"), "{refused}");

        unsafe { std::env::set_var("TALARIA_FETCH_ALLOW_HOSTS", "127.0.0.1") };
        let mut a = cat("a", "One", 0, true);
        a.url = url(one);
        let mut b = cat("b", "Two", 1, true);
        b.url = url(two);
        let reads = vec![
            (a.clone(), Some(fetch_one(&a.url).await)),
            (b.clone(), Some(fetch_one(&b.url).await)),
        ];
        unsafe { std::env::remove_var("TALARIA_FETCH_ALLOW_HOSTS") };
        let feed = merge_catalogs(reads);
        let shown: Vec<(&str, &str)> = feed
            .apps
            .iter()
            .map(|a| {
                (
                    a["slug"].as_str().unwrap(),
                    a["catalog"]["name"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(shown, [("crm", "One"), ("notes", "One"), ("wiki", "Two")]);
        assert_eq!(feed.catalogs[1].shadowed, ["notes"]);
    }

    /// The list against a migrated database: add, refuse a duplicate, edit,
    /// reorder, disable, remove — and an emptied list stays empty (no code
    /// path puts a default back). `#[ignore]`d: it needs a database, and
    /// CI's live-tests job runs it.
    ///
    ///   DATABASE_URL=postgres://… cargo test -p talaria-apps --lib -- --ignored catalog::tests::live
    #[tokio::test]
    #[ignore = "needs a live database (DATABASE_URL)"]
    async fn live_the_list_is_the_operators_and_never_reseeds() {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (a migrated database) to run the ignored live tests");
        let pg = PgPool::connect(&url).await.expect("connect");
        // The rows this test found, put back at the end whatever it did.
        let before: Vec<(String, String, String, bool, i32, Option<String>)> = sqlx::query_as(
            "select id::text, name, url, enabled, position, created_by from app_catalogs",
        )
        .fetch_all(&pg)
        .await
        .unwrap();
        sqlx::query("delete from app_catalogs")
            .execute(&pg)
            .await
            .unwrap();

        let a = create_catalog(&pg, " Ours ", "https://catalog.example/a.json", "admin@x")
            .await
            .expect("add");
        assert_eq!((a.name.as_str(), a.position, a.enabled), ("Ours", 0, true));
        let b = create_catalog(&pg, "Theirs", "https://catalog.example/b.json", "admin@x")
            .await
            .expect("add a second");
        assert_eq!(b.position, 1, "a new catalog goes to the end");
        assert!(matches!(
            create_catalog(&pg, "Again", "https://catalog.example/a.json", "admin@x").await,
            Err(CatalogError::Duplicate(_))
        ));
        assert!(matches!(
            create_catalog(&pg, "Plain", "http://catalog.example/c.json", "admin@x").await,
            Err(CatalogError::Invalid(_))
        ));

        let edited = update_catalog(
            &pg,
            &a.id,
            &CatalogPatch {
                name: Some("Ours, renamed".into()),
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .expect("edit");
        assert_eq!(
            (edited.name.as_str(), edited.enabled),
            ("Ours, renamed", false)
        );
        assert_eq!(edited.url, a.url, "an edit keeps what it does not name");
        assert!(matches!(
            update_catalog(
                &pg,
                &b.id,
                &CatalogPatch {
                    url: Some(a.url.clone()),
                    ..Default::default()
                }
            )
            .await,
            Err(CatalogError::Duplicate(_))
        ));
        assert!(matches!(
            update_catalog(
                &pg,
                "00000000-0000-0000-0000-000000000000",
                &CatalogPatch {
                    enabled: Some(true),
                    ..Default::default()
                }
            )
            .await,
            Err(CatalogError::NotFound)
        ));

        let order = reorder_catalogs(&pg, &[b.id.clone(), a.id.clone()])
            .await
            .expect("reorder");
        let names: Vec<&str> = order.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Theirs", "Ours, renamed"]);
        assert!(matches!(
            reorder_catalogs(&pg, std::slice::from_ref(&b.id)).await,
            Err(CatalogError::Invalid(_))
        ));
        assert!(matches!(
            reorder_catalogs(&pg, &[b.id.clone(), b.id.clone()]).await,
            Err(CatalogError::Invalid(_))
        ));

        // The disabled catalog is listed and not read: no fetch, no error.
        sqlx::query("delete from app_catalogs where id = $1::uuid")
            .bind(&b.id)
            .execute(&pg)
            .await
            .unwrap();
        let feed = fetch_catalogs(&pg).await.unwrap();
        assert_eq!(feed.catalogs.len(), 1);
        assert_eq!(
            (feed.catalogs[0].apps, feed.catalogs[0].error.as_deref()),
            (None, None)
        );

        let gone = delete_catalog(&pg, &a.id).await.expect("remove");
        assert_eq!(gone.id, a.id);
        assert!(matches!(
            delete_catalog(&pg, &a.id).await,
            Err(CatalogError::NotFound)
        ));
        assert!(
            list_catalogs(&pg).await.unwrap().is_empty(),
            "nothing re-seeds"
        );
        let feed = fetch_catalogs(&pg).await.unwrap();
        assert!(feed.apps.is_empty() && feed.catalogs.is_empty());

        sqlx::query("delete from app_catalogs")
            .execute(&pg)
            .await
            .unwrap();
        for (id, name, url, enabled, position, created_by) in before {
            sqlx::query(
                "insert into app_catalogs (id, name, url, enabled, position, created_by) \
                 values ($1::uuid, $2, $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(name)
            .bind(url)
            .bind(enabled)
            .bind(position)
            .bind(created_by)
            .execute(&pg)
            .await
            .unwrap();
        }
    }
}
