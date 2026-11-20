// The ClickUp v2 REST client the board sync reads through (talaria-board-sync,
// docs/BOARD-SYNC.md). Reads only, no database: the sync is one-way, ClickUp →
// Talaria, so nothing here writes to ClickUp.
//
// EVERY CALL GOES THROUGH safe_fetch. The api host is fixed, but attachment
// URLs come out of ClickUp's own responses, and the test override below points
// the base anywhere — both are URLs Talaria does not control. A loopback
// stand-in therefore needs TALARIA_FETCH_ALLOW_HOSTS as well as the override.
//
// THE TOKEN is a ClickUp personal token (`pk_…`), sent as the bare
// `Authorization` header — ClickUp wants no `Bearer`. It is never logged and
// never part of an error.
//
// ERRORS SAY WHAT KIND OF NO, never ClickUp's words (RUST-MIGRATION rule 8):
// a 401 is `Unauthorized` and nothing more. Only the caller knows whether a 401
// means the token is dead (the GET /user probe) or that one task is out of the
// token's reach (a per-task read) — so the client does not decide that.

pub mod types;

pub use types::{
    Account, Attachment, Comment, ListInfo, ListRef, ListStatus, Named, Person, Task, TasksPage,
};

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::Serialize;
use serde_json::Value;
use talaria_safe_fetch::{SafeError, SafeFetch, safe_fetch};

pub const DEFAULT_BASE: &str = "https://api.clickup.com/api/v2";

/// Test-only base override. It still has to pass safe_fetch, so a loopback
/// stand-in needs TALARIA_FETCH_ALLOW_HOSTS too.
pub const BASE_ENV: &str = "TALARIA_CLICKUP_API_BASE";

/// GET /list/{id}/task answers 100 tasks a page.
pub const PAGE_SIZE: usize = 100;

/// GET /task/{id}/comment answers 25 comments a page, newest first.
pub const COMMENT_PAGE: usize = 25;

const API_TIMEOUT_MS: u64 = 30_000;
const API_MAX_BYTES: u64 = 8 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_MS: u64 = 60_000;

pub fn api_base() -> String {
    std::env::var(BASE_ENV)
        .ok()
        .map(|b| b.trim().trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
}

/// The header value: the bare token. ClickUp refuses `Bearer pk_…`.
pub fn auth_header(token: &str) -> String {
    token.trim().to_string()
}

/// The last four characters, for "the token ending …abcd".
pub fn token_hint(token: &str) -> String {
    let t = token.trim();
    let tail: String = t
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tail
}

/// What ClickUp said about the token's budget on the last answer. ClickUp
/// limits per token per minute (100 on Free/Unlimited/Business, 1,000 on
/// Business Plus, 10,000 on Enterprise) and reports it on every answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimit {
    pub limit: Option<i64>,
    pub remaining: Option<i64>,
    /// When the window resets, epoch ms.
    pub reset_ms: Option<i64>,
}

fn header_i64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|f| f as i64)
}

/// X-RateLimit-Limit / -Remaining / -Reset. Reset is Unix SECONDS on the wire;
/// a value already in milliseconds (13 digits) is taken as it is.
pub fn parse_rate_limit(headers: &reqwest::header::HeaderMap) -> RateLimit {
    let reset = header_i64(headers, "x-ratelimit-reset")
        .map(|r| if r > 100_000_000_000 { r } else { r * 1000 });
    RateLimit {
        limit: header_i64(headers, "x-ratelimit-limit"),
        remaining: header_i64(headers, "x-ratelimit-remaining"),
        reset_ms: reset,
    }
}

/// What kind of no. The sentences are fixed: ClickUp's own text never crosses
/// into an error a person or a log reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClickUpError {
    /// 401. On GET /user the token is dead; on anything else the thing is out
    /// of the token's reach. `ecode` is ClickUp's code (OAUTH_…), kept for the
    /// caller's branch, never shown.
    Unauthorized {
        ecode: Option<String>,
    },
    Forbidden,
    NotFound,
    /// 429, or a budget the caller chose not to spend below.
    RateLimited {
        reset_ms: Option<i64>,
    },
    /// Bigger than the cap the caller set.
    TooLarge,
    /// Anything else: a network failure, a 5xx, an answer that did not parse.
    Transient(String),
}

impl std::fmt::Display for ClickUpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClickUpError::Unauthorized { .. } => f.write_str("ClickUp refused the credential"),
            ClickUpError::Forbidden => f.write_str("ClickUp says the credential may not see this"),
            ClickUpError::NotFound => f.write_str("ClickUp has no such item"),
            ClickUpError::RateLimited { .. } => f.write_str("ClickUp's rate limit was reached"),
            ClickUpError::TooLarge => f.write_str("the file is over the size limit"),
            ClickUpError::Transient(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ClickUpError {}

/// ClickUp's ECODE from an error body, if it parses.
fn ecode_of(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("ECODE")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The status code (and the ECODE, which names a missing item even on a 400)
/// as one of the error classes. `Ok(())` for a success.
pub fn classify(status: u16, body: &[u8], rate: &RateLimit) -> Result<(), ClickUpError> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let ecode = ecode_of(body);
    match status {
        429 => Err(ClickUpError::RateLimited {
            reset_ms: rate.reset_ms,
        }),
        401 => Err(ClickUpError::Unauthorized { ecode }),
        403 => Err(ClickUpError::Forbidden),
        404 => Err(ClickUpError::NotFound),
        _ if ecode.as_deref().is_some_and(|c| c.starts_with("ITEM_")) => {
            Err(ClickUpError::NotFound)
        }
        s if s >= 500 => Err(ClickUpError::Transient(format!(
            "ClickUp answered {s} — it may be having trouble; the next sync retries"
        ))),
        s => Err(ClickUpError::Transient(format!(
            "ClickUp refused the request ({s})"
        ))),
    }
}

/// The request URL: the base, path segments (each escaped), the query.
pub fn build_url(base: &str, segments: &[&str], query: &[(&str, String)]) -> String {
    let joined = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        segments
            .iter()
            .map(|s| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>())
            .collect::<Vec<_>>()
            .join("/")
    );
    if query.is_empty() {
        return joined;
    }
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in query {
        ser.append_pair(k, v);
    }
    format!("{joined}?{}", ser.finish())
}

/// One page of a list's tasks. Every field of a sync's walk is here so the
/// query a pass sends is a value a test can read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskQuery {
    pub page: u32,
    pub archived: bool,
    /// Only tasks updated after this (epoch ms).
    pub updated_after_ms: Option<i64>,
}

impl TaskQuery {
    /// Closed tasks and sub-tasks included, markdown descriptions, tasks that
    /// live in several lists, oldest first: a page boundary stays put while
    /// new tasks are created, because new ones land on the last page.
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut q = vec![
            ("page", self.page.to_string()),
            ("archived", self.archived.to_string()),
            ("include_closed", "true".into()),
            ("subtasks", "true".into()),
            ("include_markdown_description", "true".into()),
            ("include_timl", "true".into()),
            ("order_by", "created".into()),
            ("reverse", "true".into()),
        ];
        if let Some(after) = self.updated_after_ms {
            q.push(("date_updated_gt", after.to_string()));
        }
        q
    }
}

/// Where the next comments page starts: the oldest comment seen so far.
/// ClickUp pages comments backwards in time by `start` (its date) and
/// `start_id`.
pub fn comment_pairs(before: Option<(i64, &str)>) -> Vec<(&'static str, String)> {
    match before {
        Some((date, id)) => vec![("start", date.to_string()), ("start_id", id.to_string())],
        None => Vec::new(),
    }
}

/// One credential's client. Cheap: a token, a base and two counters.
pub struct Client {
    token: String,
    base: String,
    rate: Mutex<RateLimit>,
    requests: AtomicU32,
}

impl Client {
    pub fn new(token: &str) -> Self {
        Self::with_base(token, &api_base())
    }

    pub fn with_base(token: &str, base: &str) -> Self {
        Client {
            token: token.trim().to_string(),
            base: base.trim_end_matches('/').to_string(),
            rate: Mutex::new(RateLimit::default()),
            requests: AtomicU32::new(0),
        }
    }

    /// The budget as of the last answer.
    pub fn rate(&self) -> RateLimit {
        *self.rate.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Requests this client has sent.
    pub fn requests(&self) -> u32 {
        self.requests.load(Ordering::Relaxed)
    }

    async fn get_json(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<Value, ClickUpError> {
        let url = build_url(&self.base, segments, query);
        let auth = auth_header(&self.token);
        self.requests.fetch_add(1, Ordering::Relaxed);
        let res = safe_fetch(
            &url,
            SafeFetch {
                method: Some("GET"),
                headers: vec![("authorization", &auth), ("accept", "application/json")],
                timeout_ms: Some(API_TIMEOUT_MS),
                max_bytes: Some(API_MAX_BYTES),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| match e {
            SafeError::Timeout => ClickUpError::Transient("ClickUp did not answer in time".into()),
            SafeError::Blocked(_) => ClickUpError::Transient(
                "the ClickUp address is not reachable from this instance".into(),
            ),
            SafeError::Fetch(_) => ClickUpError::Transient("ClickUp could not be reached".into()),
        })?;
        let rate = parse_rate_limit(&res.headers);
        if rate != RateLimit::default() {
            *self.rate.lock().unwrap_or_else(|p| p.into_inner()) = rate;
        }
        classify(res.status, &res.body, &rate)?;
        serde_json::from_slice(&res.body).map_err(|_| {
            ClickUpError::Transient("ClickUp sent an answer that did not parse".into())
        })
    }

    /// GET /user — the identity the token acts as. The probe every run starts
    /// with: its 401 is the only one that means "the token is dead".
    pub async fn user(&self) -> Result<Account, ClickUpError> {
        let body = self.get_json(&["user"], &[]).await?;
        Account::parse(&body).ok_or_else(|| ClickUpError::Transient("ClickUp sent no user".into()))
    }

    /// GET /team — the workspaces the token can see.
    pub async fn workspaces(&self) -> Result<Vec<Named>, ClickUpError> {
        let body = self.get_json(&["team"], &[]).await?;
        Ok(Named::parse_all(&body, "teams"))
    }

    pub async fn spaces(&self, team_id: &str) -> Result<Vec<Named>, ClickUpError> {
        let body = self
            .get_json(&["team", team_id, "space"], &[("archived", "false".into())])
            .await?;
        Ok(Named::parse_all(&body, "spaces"))
    }

    pub async fn folders(&self, space_id: &str) -> Result<Vec<Named>, ClickUpError> {
        let body = self
            .get_json(
                &["space", space_id, "folder"],
                &[("archived", "false".into())],
            )
            .await?;
        Ok(Named::parse_all(&body, "folders"))
    }

    pub async fn folder_lists(&self, folder_id: &str) -> Result<Vec<ListRef>, ClickUpError> {
        let body = self
            .get_json(
                &["folder", folder_id, "list"],
                &[("archived", "false".into())],
            )
            .await?;
        Ok(ListRef::parse_all(&body))
    }

    pub async fn folderless_lists(&self, space_id: &str) -> Result<Vec<ListRef>, ClickUpError> {
        let body = self
            .get_json(
                &["space", space_id, "list"],
                &[("archived", "false".into())],
            )
            .await?;
        Ok(ListRef::parse_all(&body))
    }

    /// GET /list/{id} — its path and effective statuses.
    pub async fn list(&self, list_id: &str) -> Result<ListInfo, ClickUpError> {
        let body = self.get_json(&["list", list_id], &[]).await?;
        ListInfo::parse(&body).ok_or_else(|| ClickUpError::Transient("ClickUp sent no list".into()))
    }

    pub async fn tasks_page(
        &self,
        list_id: &str,
        q: &TaskQuery,
    ) -> Result<TasksPage, ClickUpError> {
        let body = self
            .get_json(&["list", list_id, "task"], &q.pairs())
            .await?;
        Ok(TasksPage::parse(&body))
    }

    /// GET /task/{id} — the one read that carries attachments.
    pub async fn task(&self, task_id: &str) -> Result<Task, ClickUpError> {
        let body = self
            .get_json(
                &["task", task_id],
                &[("include_markdown_description", "true".into())],
            )
            .await?;
        Task::parse(&body).ok_or_else(|| ClickUpError::Transient("ClickUp sent no task".into()))
    }

    /// One page of a task's comments, newest first, older than `before`.
    pub async fn comments(
        &self,
        task_id: &str,
        before: Option<(i64, &str)>,
    ) -> Result<Vec<Comment>, ClickUpError> {
        let body = self
            .get_json(&["task", task_id, "comment"], &comment_pairs(before))
            .await?;
        Ok(Comment::parse_all(&body))
    }

    /// A comment's threaded replies.
    pub async fn replies(&self, comment_id: &str) -> Result<Vec<Comment>, ClickUpError> {
        let body = self
            .get_json(&["comment", comment_id, "reply"], &[])
            .await?;
        Ok(Comment::parse_all(&body))
    }

    /// An attachment's bytes, refused past `max_bytes` before they are all
    /// read. The URL came from ClickUp's own answer; no credential rides on
    /// it (safe_fetch would strip one at a cross-origin hop anyway).
    pub async fn download(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, ClickUpError> {
        let res = safe_fetch(
            url,
            SafeFetch {
                method: Some("GET"),
                timeout_ms: Some(DOWNLOAD_TIMEOUT_MS),
                max_bytes: Some(max_bytes as u64),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| match e {
            SafeError::Blocked(b) if b.0.contains("cap") => ClickUpError::TooLarge,
            SafeError::Blocked(_) => ClickUpError::Transient(
                "the file's address is not reachable from this instance".into(),
            ),
            SafeError::Timeout => ClickUpError::Transient("the download timed out".into()),
            SafeError::Fetch(_) => ClickUpError::Transient("the file could not be fetched".into()),
        })?;
        classify(res.status, &res.body, &RateLimit::default())?;
        Ok(res.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn the_auth_header_is_the_bare_token() {
        assert_eq!(auth_header(" pk_123_ABC "), "pk_123_ABC");
        assert!(!auth_header("pk_1").starts_with("Bearer"));
        assert_eq!(token_hint("pk_123_ABCD"), "ABCD");
        assert_eq!(token_hint("ab"), "ab");
    }

    #[test]
    fn a_tasks_page_parses_and_the_pager_stops_on_last_page() {
        let page = TasksPage::parse(&fixture("tasks_page.json"));
        assert_eq!(page.tasks.len(), 3);
        assert!(page.last_page);
        let t = &page.tasks[0];
        assert_eq!(t.id, "86a1b2c3d");
        assert_eq!(t.custom_id.as_deref(), Some("PROJ-7"));
        assert_eq!(t.status, "in progress");
        assert_eq!(t.status_type.as_deref(), Some("custom"));
        assert_eq!(t.priority, Some(2));
        assert_eq!(t.tags, vec!["backend".to_string(), "q4".to_string()]);
        assert_eq!(t.due_ms, Some(1_767_225_600_000));
        assert_eq!(t.assignees[0].email.as_deref(), Some("dana@example.com"));
        assert_eq!(t.description.as_deref(), Some("**Bold** plan"));
        assert!(t.attachments.is_none(), "the list endpoint carries none");
        // A sub-task names its parent and its top-level parent.
        let sub = &page.tasks[2];
        assert_eq!(sub.parent.as_deref(), Some("86a1b2c3e"));
        assert_eq!(sub.top_level_parent.as_deref(), Some("86a1b2c3d"));
        assert!(page.tasks[1].closed());
        assert_eq!(page.tasks[1].priority, None);
        // A full page with no flag is not the last.
        let mut many = serde_json::json!({"tasks": []});
        for i in 0..PAGE_SIZE {
            many["tasks"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"id": format!("t{i}"), "name": "x"}));
        }
        assert!(!TasksPage::parse(&many).last_page);
    }

    #[test]
    fn task_detail_parses_attachments_and_skips_deleted_ones() {
        let t = Task::parse(&fixture("task_detail.json")).unwrap();
        let atts = t.attachments.unwrap();
        assert_eq!(atts.len(), 2);
        assert_eq!(atts[0].title, "plan.pdf");
        assert_eq!(atts[0].size, Some(48_213));
        assert_eq!(atts[0].mime.as_deref(), Some("application/pdf"));
        assert!(atts[0].url.as_deref().unwrap().starts_with("https://"));
        assert_eq!(atts[1].size, Some(222_298_112));
        assert!(t.in_list("901100"));
        assert!(t.in_list("901999"), "a second list counts");
        assert!(!t.in_list("123"));
    }

    #[test]
    fn comments_and_replies_parse_and_page_backwards() {
        let cs = Comment::parse_all(&fixture("comments.json"));
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].id, "90120001");
        assert_eq!(cs[0].reply_count, 1, "a string count reads");
        assert_eq!(cs[0].user.as_ref().unwrap().display(), "Dana Reyes");
        assert_eq!(cs[1].text, "first line", "the structured form flattens");
        assert_eq!(cs[1].date_ms, Some(1_767_000_000_000));
        assert!(comment_pairs(None).is_empty());
        assert_eq!(
            comment_pairs(Some((1_767_000_000_000, "90120002"))),
            vec![
                ("start", "1767000000000".to_string()),
                ("start_id", "90120002".to_string())
            ]
        );
    }

    #[test]
    fn list_statuses_parse_in_order_and_the_path_skips_a_hidden_folder() {
        let l = ListInfo::parse(&fixture("list.json")).unwrap();
        let names: Vec<&str> = l.statuses.iter().map(|s| s.status.as_str()).collect();
        assert_eq!(names, ["to do", "in progress", "qa", "complete"]);
        assert_eq!(l.statuses[3].kind.as_deref(), Some("closed"));
        assert_eq!(l.path(), "Engineering / Sprint board");
        assert_eq!(l.task_count, Some(214));
    }

    #[test]
    fn the_task_query_names_every_walk_parameter() {
        let q = TaskQuery {
            page: 3,
            archived: false,
            updated_after_ms: Some(1_700_000_000_000),
        };
        let url = build_url(
            "https://api.clickup.com/api/v2",
            &["list", "901100", "task"],
            &q.pairs(),
        );
        assert!(
            url.starts_with(
                "https://api.clickup.com/api/v2/list/901100/task?page=3&archived=false"
            )
        );
        for p in [
            "include_closed=true",
            "subtasks=true",
            "include_markdown_description=true",
            "order_by=created",
            "date_updated_gt=1700000000000",
        ] {
            assert!(url.contains(p), "{p} missing from {url}");
        }
        let first = TaskQuery::default();
        assert!(
            !build_url("b", &["list", "1", "task"], &first.pairs()).contains("date_updated_gt")
        );
        // A segment is escaped, never spliced.
        assert_eq!(build_url("b", &["task", "a/b"], &[]), "b/task/a%2Fb");
    }

    #[test]
    fn rate_limit_headers_parse_and_429_carries_the_reset() {
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-limit", HeaderValue::from_static("100"));
        h.insert("x-ratelimit-remaining", HeaderValue::from_static("7"));
        h.insert("x-ratelimit-reset", HeaderValue::from_static("1767225660"));
        let r = parse_rate_limit(&h);
        assert_eq!(r.limit, Some(100));
        assert_eq!(r.remaining, Some(7));
        assert_eq!(r.reset_ms, Some(1_767_225_660_000));
        assert_eq!(
            classify(429, b"{}", &r),
            Err(ClickUpError::RateLimited {
                reset_ms: Some(1_767_225_660_000)
            })
        );
    }

    #[test]
    fn errors_classify_by_status_and_never_carry_upstream_prose() {
        let none = RateLimit::default();
        let body = br#"{"err":"Token invalid secret-ish words","ECODE":"OAUTH_025"}"#;
        let e = classify(401, body, &none).unwrap_err();
        assert_eq!(
            e,
            ClickUpError::Unauthorized {
                ecode: Some("OAUTH_025".into())
            }
        );
        assert!(!e.to_string().contains("secret-ish"));
        assert_eq!(classify(403, b"", &none), Err(ClickUpError::Forbidden));
        assert_eq!(classify(404, b"", &none), Err(ClickUpError::NotFound));
        assert_eq!(
            classify(
                400,
                br#"{"err":"Task not found","ECODE":"ITEM_013"}"#,
                &none
            ),
            Err(ClickUpError::NotFound)
        );
        let t = classify(502, br#"{"err":"upstream words"}"#, &none).unwrap_err();
        assert!(matches!(t, ClickUpError::Transient(_)));
        assert!(!t.to_string().contains("upstream words"));
        assert!(classify(200, b"{}", &none).is_ok());
    }

    #[test]
    fn a_person_without_a_username_is_named_from_the_email() {
        let p = Person {
            id: None,
            username: None,
            email: Some("lee@example.com".into()),
        };
        assert_eq!(p.display(), "lee");
        let nobody = Person {
            id: None,
            username: None,
            email: None,
        };
        assert_eq!(nobody.display(), "a ClickUp member");
    }

    #[test]
    fn the_account_parses_from_the_user_envelope() {
        let a = Account::parse(
            &serde_json::json!({"user": {"id": 183, "username": "Dana", "email": "d@x.io"}}),
        )
        .unwrap();
        assert_eq!(a.id, "183");
        assert_eq!(a.email.as_deref(), Some("d@x.io"));
    }
}
