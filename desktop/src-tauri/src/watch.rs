//! A suspended instance's notifications.
//!
//! An open page posts its own: ui's `NotificationToasts` watches
//! `GET /api/notifications` (made live by `/api/me/events`, with a 30 s poll
//! floor) and, for every row that lands while Talaria is not what the person
//! is looking at, asks the shell to post it (`desktop_notify`). A suspended
//! page posts nothing, so the shell does the same job for it, natively: the
//! same two routes, the same arrival rule (the first read is the baseline,
//! then every new row, oldest first), the same banner and the same click.
//!
//! It runs only when the page said it would post (`notify` in its report: ui's
//! `browserNotifyEnabled`), with that instance's own cookies, read from its
//! webview just before it closed, and only ever to that instance's origin:
//! redirects are not followed. Its first read (`baseline`) happens while the
//! page is still open; if the shell cannot get that read the way the page
//! does, the page is not suspended. A 401 or 403 (signed out, session
//! expired) ends it; a restore ends it too. Its cost is one HTTP connection.

use std::{collections::HashSet, time::Duration};

use reqwest::{
    StatusCode,
    header::{ACCEPT, COOKIE},
};
use serde::Deserialize;
use tauri::AppHandle;

/// The poll floor: the page's own `refetchInterval`.
const POLL: Duration = Duration::from_secs(30);

/// No bytes for this long and the stream is presumed dead. The server pings
/// every 25 s.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// The most row ids remembered, as the page's ArrivalTracker.
const MAX_SEEN: usize = 1_000;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Row {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub href: String,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    notifications: Vec<Row>,
}

/// ui's `ArrivalTracker`: which rows have been surfaced, so an arrival posts
/// exactly once. The first page is the baseline, empty or not.
#[derive(Debug, Default)]
pub struct Arrivals {
    seen: HashSet<String>,
    primed: bool,
}

impl Arrivals {
    /// Rows never seen before, oldest first. `rows` is newest first, as
    /// `/api/notifications` returns them.
    pub fn arrive(&mut self, rows: &[Row]) -> Vec<Row> {
        if !self.primed {
            self.primed = true;
            self.seen.extend(rows.iter().map(|r| r.id.clone()));
            return Vec::new();
        }
        let mut fresh: Vec<Row> = rows
            .iter()
            .filter(|r| self.seen.insert(r.id.clone()))
            .cloned()
            .collect();
        if self.seen.len() > MAX_SEEN {
            self.seen = rows.iter().map(|r| r.id.clone()).collect();
        }
        fresh.reverse();
        fresh
    }
}

/// An SSE body, fed as it arrives: complete events come out, a partial one
/// waits for the rest.
#[derive(Debug, Default)]
pub struct Frames {
    buf: String,
}

impl Frames {
    /// The `data` of every complete event in what has arrived so far.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(&chunk.replace("\r\n", "\n"));
        let mut out = Vec::new();
        while let Some(end) = self.buf.find("\n\n") {
            let event: String = self.buf.drain(..end + 2).collect();
            let data: Vec<&str> = event
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if !data.is_empty() {
                out.push(data.join("\n"));
            }
        }
        out
    }
}

/// True for an `/api/me/events` frame that says a notification landed.
pub fn is_notification(data: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(data)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str().map(str::to_string)))
        .as_deref()
        == Some("notification")
}

/// The `Cookie` header for a request to the instance: every cookie its
/// webview holds for that URL, as the webview would send it.
pub fn cookie_header<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<String> {
    let header = pairs
        .into_iter()
        .filter(|(name, _)| !name.is_empty())
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");
    (!header.is_empty()).then_some(header)
}

enum Fetch {
    Rows(Vec<Row>),
    SignedOut,
    Failed,
}

async fn fetch(client: &reqwest::Client, origin: &str, cookie: &str) -> Fetch {
    let response = client
        .get(format!("{origin}/api/notifications"))
        .header(COOKIE, cookie)
        .header(ACCEPT, "application/json")
        .send()
        .await;
    match response {
        Ok(r) if matches!(r.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) => {
            Fetch::SignedOut
        }
        Ok(r) if r.status().is_success() => match r.json::<Payload>().await {
            Ok(p) => Fetch::Rows(p.notifications),
            Err(_) => Fetch::Failed,
        },
        _ => Fetch::Failed,
    }
}

fn client() -> reqwest::Client {
    crate::build_client(|b| {
        b.read_timeout(READ_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
    })
}

/// What the shell's first read of an instance's notifications found, taken
/// while its page is still open.
pub enum Baseline {
    /// The rows the page has already surfaced: the watcher starts from them.
    Ready(Arrivals),
    /// 401 or 403: the page is signed out and is not fetching either.
    SignedOut,
    /// Anything else: a proxy or a certificate only the webview knows, the
    /// instance down. The shell cannot do the page's job, so the page stays.
    Unreachable,
}

/// The longest the first read may take. The page waits open for it, and a
/// read that has not finished by then counts as one the shell cannot make.
pub const BASELINE_WAIT: Duration = Duration::from_secs(30);

/// One read of `/api/notifications` as the page would make it, bounded by
/// `BASELINE_WAIT`.
pub async fn baseline(origin: &str, cookie: &str) -> Baseline {
    let read = tokio::time::timeout(BASELINE_WAIT, fetch(&client(), origin, cookie)).await;
    match read.unwrap_or(Fetch::Failed) {
        Fetch::Rows(rows) => {
            let mut arrivals = Arrivals::default();
            arrivals.arrive(&rows);
            Baseline::Ready(arrivals)
        }
        Fetch::SignedOut => Baseline::SignedOut,
        Fetch::Failed => Baseline::Unreachable,
    }
}

/// Start watching for `label`'s instance at `origin`, from `arrivals` (its
/// baseline). The handle aborts it.
pub fn spawn(
    app: AppHandle,
    label: String,
    origin: String,
    cookie: String,
    arrivals: Arrivals,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move { run(app, label, origin, cookie, arrivals).await })
}

async fn run(
    app: AppHandle,
    label: String,
    origin: String,
    cookie: String,
    mut arrivals: Arrivals,
) {
    let client = client();
    eprintln!("talaria-desktop: {label}: watching notifications while suspended");
    let mut next_poll = tokio::time::Instant::now() + POLL;
    loop {
        let stream = client
            .get(format!("{origin}/api/me/events"))
            .header(COOKIE, &cookie)
            .header(ACCEPT, "text/event-stream")
            .send()
            .await;
        let mut response = match stream {
            Ok(r) if matches!(r.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) => {
                eprintln!("talaria-desktop: {label}: signed out; no notifications while suspended");
                return;
            }
            Ok(r) if r.status().is_success() => Some(r),
            _ => None,
        };
        let mut frames = Frames::default();
        loop {
            let poll_at = next_poll;
            let check = match response.as_mut() {
                Some(r) => match tokio::time::timeout_at(poll_at, r.chunk()).await {
                    Ok(Ok(Some(bytes))) => frames
                        .push(&String::from_utf8_lossy(&bytes))
                        .iter()
                        .any(|d| is_notification(d)),
                    // The stream ended or broke: reconnect after a pause. The
                    // poll floor covers the gap.
                    Ok(_) => break,
                    // The poll floor.
                    Err(_) => true,
                },
                None => {
                    tokio::time::sleep_until(poll_at).await;
                    true
                }
            };
            if !check {
                continue;
            }
            if tokio::time::Instant::now() >= next_poll {
                next_poll = tokio::time::Instant::now() + POLL;
            }
            match fetch(&client, &origin, &cookie).await {
                Fetch::Rows(rows) => {
                    for row in arrivals.arrive(&rows) {
                        post(&app, &label, row);
                    }
                }
                Fetch::SignedOut => {
                    eprintln!(
                        "talaria-desktop: {label}: signed out; no notifications while suspended"
                    );
                    return;
                }
                Fetch::Failed => {}
            }
            if response.is_none() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Post one arrival exactly as `desktop_notify` would for the page: clamped,
/// tagged with the row id, and a click that restores the instance at `href`.
fn post(app: &AppHandle, label: &str, row: Row) {
    let title = crate::notify::clamped_title(&row.title);
    if title.is_empty() {
        return;
    }
    let body = Some(crate::notify::clamped_body(&row.body)).filter(|b| !b.is_empty());
    let tag = Some(crate::notify::clamped_tag(&row.id)).filter(|t| !t.is_empty());
    let href = crate::notify::openable_href(&row.href).map(str::to_string);
    let app = app.clone();
    let label = label.to_string();
    eprintln!(
        "talaria-desktop: {label}: posting {} while suspended",
        row.id
    );
    tauri::async_runtime::spawn_blocking(move || {
        let on_open = {
            let app = app.clone();
            let label = label.clone();
            move || crate::commands::reveal_notice(&app, &label, href.as_deref())
        };
        if let Err(e) = crate::notify::post(&app, &title, body.as_deref(), tag.as_deref(), on_open)
        {
            eprintln!("talaria-desktop: {label}: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str) -> Row {
        Row {
            id: id.into(),
            title: format!("t-{id}"),
            body: String::new(),
            href: String::new(),
        }
    }

    #[test]
    fn the_first_read_is_the_baseline_then_each_arrival_once_oldest_first() {
        let mut a = Arrivals::default();
        assert!(a.arrive(&[row("2"), row("1")]).is_empty());
        assert_eq!(
            a.arrive(&[row("4"), row("3"), row("2"), row("1")]),
            vec![row("3"), row("4")]
        );
        assert!(a.arrive(&[row("4"), row("3"), row("2")]).is_empty());
        // An empty baseline still primes: the first row ever is an arrival.
        let mut b = Arrivals::default();
        assert!(b.arrive(&[]).is_empty());
        assert_eq!(b.arrive(&[row("1")]), vec![row("1")]);
    }

    #[test]
    fn the_remembered_set_stays_bounded() {
        let mut a = Arrivals::default();
        a.arrive(&[]);
        for i in 0..(MAX_SEEN + 10) {
            a.arrive(&[row(&i.to_string())]);
        }
        assert!(a.seen.len() <= MAX_SEEN);
    }

    #[test]
    fn frames_split_events_across_chunks_and_skip_comments() {
        let mut f = Frames::default();
        assert!(f.push(": connected\n\n").is_empty());
        assert!(f.push("data: {\"type\":\"notif").is_empty());
        assert_eq!(
            f.push(
                "ication\",\"notificationId\":\"n1\"}\n\ndata: {\"type\":\"run\"}\r\n\r\n: ping\n\n"
            ),
            vec![
                r#"{"type":"notification","notificationId":"n1"}"#.to_string(),
                r#"{"type":"run"}"#.to_string()
            ]
        );
        assert!(is_notification(
            r#"{"type":"notification","notificationId":"n1"}"#
        ));
        assert!(!is_notification(r#"{"type":"run","runId":"r"}"#));
        assert!(!is_notification("not json"));
    }

    #[test]
    fn the_cookie_header_joins_every_pair() {
        assert_eq!(
            cookie_header([("talaria_session", "abc%2F"), ("other", "1")]),
            Some("talaria_session=abc%2F; other=1".into())
        );
        assert_eq!(cookie_header([]), None);
        assert_eq!(cookie_header([("", "x")]), None);
    }
}
