//! Linux: org.freedesktop.Notifications, over ONE session-bus connection.
//!
//! The process holds a single zbus connection (lazy, and reopened after an
//! I/O failure) with one subscription per signal it needs — `ActionInvoked`,
//! `NotificationClosed`, `ActivationToken` (spec 1.2) and the bus's own
//! `NameOwnerChanged` for the tray watcher — dispatched by id. No thread or
//! connection is held per banner: notify-rust's `wait_for_action` held one of
//! each until the server retired the banner, and 30 banners left open cost 30
//! bus names and 63 threads (D11).
//!
//! The same connection carries the launcher badge
//! (`com.canonical.Unity.LauncherEntry`) and owns `app.talaria.desktop.Shell`,
//! the single-instance name: a second launch calls its `Activate(token)` and
//! exits before it builds a window. Flatpak lets an app own names under its
//! own id with no extra permission.
//!
//! What a post carries (D10): app name "Talaria"; the icon and the
//! `desktop-entry` hint of the desktop file this install actually has
//! (detected, never assumed — deb/rpm ship `Talaria.desktop`, flatpak,
//! AppImage and pacman `app.talaria.desktop`, and renaming either would break
//! every pinned launcher); the instance's label in front of the body, escaped
//! when the server parses markup; a category and an urgency from the kind;
//! and the server's own id from the last post of the same (instance, tag) as
//! `replaces_id`, never an invented one — except for a re-alert, which closes
//! that banner and posts a new one, since a replace may be silent.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, atomic::AtomicBool, atomic::Ordering},
    time::Duration,
};

use futures_lite::StreamExt;
use tauri::AppHandle;
use zbus::{
    Connection, MatchRule, MessageStream,
    fdo::{RequestNameFlags, RequestNameReply},
    message::Type as MessageType,
    zvariant::Value,
};

use super::{OsStatus, Permission, Post, ServerInfo, Urgency};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// The single-instance name and its one method.
pub const SHELL_NAME: &str = "app.talaria.desktop.Shell";
const SHELL_PATH: &str = "/app/talaria/desktop/Shell";

/// Where the launcher-badge signal is emitted from. Docks match the
/// interface and the app URI, not the path.
const LAUNCHER_PATH: &str = "/app/talaria/desktop/LauncherEntry";

/// The tray host. Without an owner there is no tray to show (stock GNOME).
pub const WATCHER: &str = "org.kde.StatusNotifierWatcher";

/// A call the server has not answered in this long has failed.
const CALL_WAIT: Duration = Duration::from_secs(10);

/// The desktop file this install carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// Its name without `.desktop`: the `desktop-entry` hint, and the badge's
    /// `application://<id>.desktop`.
    pub id: String,
    /// Its `Icon=`, the banner's icon.
    pub icon: String,
}

/// The id every package but deb/rpm uses, and the fallback.
pub const APP_ID: &str = "app.talaria.desktop";
/// What Tauri's deb and rpm bundlers install.
const PRODUCT_DESKTOP: &str = "Talaria";

/// Which desktop file is installed, in order: a Flatpak (`$FLATPAK_ID`) or an
/// AppImage (`$APPIMAGE`) is `app.talaria.desktop`; otherwise the first
/// `applications/app.talaria.desktop.desktop`, then `applications/Talaria.desktop`,
/// across `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`; otherwise
/// `app.talaria.desktop`. The path, when a file was found.
pub fn detect_desktop_id(
    flatpak_id: Option<&str>,
    appimage: Option<&str>,
    data_dirs: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> (String, Option<PathBuf>) {
    if flatpak_id.is_some_and(|v| !v.is_empty()) || appimage.is_some_and(|v| !v.is_empty()) {
        return (APP_ID.to_string(), None);
    }
    for id in [APP_ID, PRODUCT_DESKTOP] {
        for dir in data_dirs {
            let path = dir.join("applications").join(format!("{id}.desktop"));
            if exists(&path) {
                return (id.to_string(), Some(path));
            }
        }
    }
    (APP_ID.to_string(), None)
}

/// `$XDG_DATA_HOME` then `$XDG_DATA_DIRS`, with the spec's defaults.
pub fn data_dirs(home: Option<&str>, data_home: Option<&str>, dirs: Option<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match data_home.filter(|v| !v.is_empty()) {
        Some(d) => out.push(PathBuf::from(d)),
        None => {
            if let Some(home) = home.filter(|v| !v.is_empty()) {
                out.push(Path::new(home).join(".local/share"));
            }
        }
    }
    let dirs = dirs
        .filter(|v| !v.is_empty())
        .unwrap_or("/usr/local/share:/usr/share");
    out.extend(dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    out
}

/// The `Icon=` of a desktop file's main group.
pub fn icon_of(contents: &str) -> Option<String> {
    let mut main = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            main = line == "[Desktop Entry]";
            continue;
        }
        if main
            && let Some(icon) = line.strip_prefix("Icon=")
            && !icon.trim().is_empty()
        {
            return Some(icon.trim().to_string());
        }
    }
    None
}

/// The detected desktop file, read once per process.
pub fn desktop_entry() -> &'static DesktopEntry {
    static ENTRY: OnceLock<DesktopEntry> = OnceLock::new();
    ENTRY.get_or_init(|| {
        let env = |k: &str| std::env::var(k).ok();
        let dirs = data_dirs(
            env("HOME").as_deref(),
            env("XDG_DATA_HOME").as_deref(),
            env("XDG_DATA_DIRS").as_deref(),
        );
        let (id, path) = detect_desktop_id(
            env("FLATPAK_ID").as_deref(),
            env("APPIMAGE").as_deref(),
            &dirs,
            |p| p.is_file(),
        );
        let icon = path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|c| icon_of(&c))
            .unwrap_or_else(|| APP_ID.to_string());
        eprintln!("talaria-desktop: desktop entry {id} (icon {icon})");
        DesktopEntry { id, icon }
    })
}

/// The banner body: the instance first, because one shell shows many. The
/// shell's own notices name no instance.
pub fn body_text(instance: &str, body: Option<&str>, markup: bool) -> String {
    let text = match body.filter(|b| !b.is_empty()) {
        Some(body) if instance.is_empty() => body.to_string(),
        Some(body) => format!("{instance}: {body}"),
        None => instance.to_string(),
    };
    if markup {
        super::escape_markup(&text)
    } else {
        text
    }
}

static CONN: Mutex<Option<Connection>> = Mutex::new(None);
static CAPS: Mutex<Option<Vec<String>>> = Mutex::new(None);
/// `ActivationToken` arrives just before its `ActionInvoked`; held until then.
static TOKENS: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());
/// The process has claimed the single-instance name once: a reconnect claims
/// it again.
static CLAIMED: AtomicBool = AtomicBool::new(false);

/// The process's one session-bus connection, opened on first use and again
/// after one breaks.
pub async fn connection() -> Result<Connection, String> {
    if let Some(conn) = CONN.lock().unwrap().clone() {
        return Ok(conn);
    }
    let conn = Connection::session()
        .await
        .map_err(|e| format!("connecting to the session bus: {e}"))?;
    {
        let mut slot = CONN.lock().unwrap();
        if let Some(existing) = slot.clone() {
            return Ok(existing);
        }
        *slot = Some(conn.clone());
    }
    *CAPS.lock().unwrap() = None;
    if let Err(e) = conn.object_server().at(SHELL_PATH, Shell).await {
        eprintln!("talaria-desktop: serving {SHELL_NAME}: {e}");
    }
    if CLAIMED.load(Ordering::Relaxed) {
        let _ = conn
            .request_name_with_flags(SHELL_NAME, RequestNameFlags::DoNotQueue.into())
            .await;
    }
    tauri::async_runtime::spawn(listen(conn.clone()));
    tauri::async_runtime::spawn(watch_tray(conn.clone()));
    Ok(conn)
}

/// Forget a connection whose socket failed, so the next call opens a new
/// one. Only a transport error: an error the bus or the server answered
/// with, or a slow answer, leaves the connection (and its one set of
/// listeners) in place — a second connection would mean every click
/// dispatched twice.
fn reset_if_broken(e: &zbus::Error) {
    if matches!(e, zbus::Error::InputOutput(_)) {
        *CONN.lock().unwrap() = None;
        *CAPS.lock().unwrap() = None;
    }
}

async fn call<B>(conn: &Connection, method: &str, body: &B) -> Result<zbus::Message, zbus::Error>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    match tokio::time::timeout(
        CALL_WAIT,
        conn.call_method(Some(SERVICE), PATH, Some(SERVICE), method, body),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(zbus::Error::Failure(format!(
            "the notification service did not answer {method} in {} s",
            CALL_WAIT.as_secs()
        ))),
    }
}

/// What the server can render, read once per connection.
async fn capabilities(conn: &Connection) -> Result<Vec<String>, zbus::Error> {
    if let Some(caps) = CAPS.lock().unwrap().clone() {
        return Ok(caps);
    }
    let reply = call(conn, "GetCapabilities", &()).await?;
    let caps: Vec<String> = reply.body().deserialize()?;
    *CAPS.lock().unwrap() = Some(caps.clone());
    Ok(caps)
}

async fn server_info(conn: &Connection) -> Result<ServerInfo, zbus::Error> {
    let reply = call(conn, "GetServerInformation", &()).await?;
    let (name, vendor, version, spec): (String, String, String, String) =
        reply.body().deserialize()?;
    Ok(ServerInfo {
        name,
        vendor,
        version,
        spec,
    })
}

pub fn init(_app: &AppHandle) {
    // The connection usually exists already (the single-instance claim made
    // it before the builder). If the bus was not there then, try now, off the
    // main thread; every post tries again.
    tauri::async_runtime::spawn(async {
        if let Err(e) = connection().await {
            eprintln!("talaria-desktop: {e}");
        }
    });
}

/// Post, and answer the id the server issued.
pub async fn post(post: &Post, previous: Option<u32>) -> Result<u32, String> {
    let conn = connection().await?;
    let markup = match capabilities(&conn).await {
        Ok(caps) => caps.iter().any(|c| c == "body-markup"),
        Err(e) => {
            reset_if_broken(&e);
            return Err(format!("showing the notification: {e}"));
        }
    };
    // A re-alert is a new banner. The spec has a replaced notification update
    // "with no visual cues", so a server may well update one silently
    // (GNOME Shell 50 happens to re-show it; nothing obliges another server
    // to): close the old one — it leaves the list too — and post afresh.
    let (previous, closed) = match previous {
        Some(id) if post.renotify => {
            if let Err(e) = call(&conn, "CloseNotification", &(id,)).await {
                reset_if_broken(&e);
                eprintln!("talaria-desktop: closing notification {id} to re-alert: {e}");
            }
            (None, Some(id))
        }
        other => (other, None),
    };
    let entry = desktop_entry();
    let body = body_text(&post.instance, post.body.as_deref(), markup);
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", Value::from(entry.id.as_str()));
    let urgency = super::urgency_of(post.kind.as_deref());
    hints.insert("urgency", Value::U8(urgency as u8));
    if let Some(category) = super::category_of(post.kind.as_deref()) {
        hints.insert("category", Value::from(category));
    }
    let actions = vec!["default", "Open"];
    let args = (
        "Talaria",
        previous.unwrap_or(0),
        entry.icon.as_str(),
        post.title.as_str(),
        body.as_str(),
        actions,
        hints,
        -1i32,
    );
    match call(&conn, "Notify", &args).await {
        Ok(reply) => {
            let id: u32 = reply
                .body()
                .deserialize()
                .map_err(|e| format!("reading the notification id: {e}"))?;
            eprintln!(
                "talaria-desktop: {}: posted {} as {id}{}{}{}",
                post.label,
                post.tag,
                if urgency == Urgency::Critical {
                    " (critical)"
                } else {
                    ""
                },
                previous.map_or(String::new(), |p| format!(", replacing {p}")),
                closed.map_or(String::new(), |c| format!(", a re-alert after closing {c}"))
            );
            Ok(id)
        }
        Err(e) => {
            reset_if_broken(&e);
            Err(format!("showing the notification: {e}"))
        }
    }
}

/// The server, as it describes itself, or the bus's reason there is none.
pub async fn probe() -> OsStatus {
    let unavailable = |detail: String| OsStatus {
        backend: "fdo".into(),
        permission: Permission::Unavailable,
        server: None,
        capabilities: None,
        detail: Some(format!("unavailable: {detail}")),
    };
    let conn = match connection().await {
        Ok(conn) => conn,
        Err(e) => return unavailable(e),
    };
    let server = match server_info(&conn).await {
        Ok(server) => server,
        Err(e) => {
            reset_if_broken(&e);
            return unavailable(e.to_string());
        }
    };
    let capabilities = capabilities(&conn).await.ok();
    OsStatus {
        backend: "fdo".into(),
        permission: Permission::Granted,
        detail: Some(format!(
            "shown by {} {} (spec {})",
            server.name, server.version, server.spec
        )),
        server: Some(server),
        capabilities,
    }
}

/// Linux has no prompt: the status is the answer.
pub async fn request() -> OsStatus {
    probe().await
}

/// The launcher badge, for the docks that read it (Plasma, Ubuntu Dock,
/// Dash-to-Dock; stock GNOME shows none). Tauri's own `set_badge_count`
/// assumes `Talaria.desktop`, which is wrong for every package but deb/rpm.
pub fn set_badge(_app: &AppHandle, total: u32) {
    tauri::async_runtime::spawn(async move {
        let Ok(conn) = connection().await else {
            return;
        };
        let uri = format!("application://{}.desktop", desktop_entry().id);
        let mut props: HashMap<&str, Value<'_>> = HashMap::new();
        props.insert("count", Value::I64(i64::from(total)));
        props.insert("count-visible", Value::Bool(total > 0));
        if let Err(e) = conn
            .emit_signal(
                None::<&str>,
                LAUNCHER_PATH,
                "com.canonical.Unity.LauncherEntry",
                "Update",
                &(uri.as_str(), props),
            )
            .await
        {
            eprintln!("talaria-desktop: the launcher badge: {e}");
        }
    });
}

/// Is this signal the notification server's? The rule's sender makes the
/// bus deliver only the server's broadcasts, but a signal ADDRESSED to this
/// connection skips match rules, and zbus cannot check a well-known sender
/// itself. So the sender's unique name is compared with the server's: asked
/// once, and again whenever it differs (a restarted server has a new one).
async fn from_server(
    conn: &Connection,
    owner: &mut Option<String>,
    sender: Option<&zbus::names::UniqueName<'_>>,
) -> bool {
    let Some(sender) = sender else {
        return false;
    };
    if owner.as_deref() == Some(sender.as_str()) {
        return true;
    }
    *owner = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(SERVICE,),
        )
        .await
        .ok()
        .and_then(|r| r.body().deserialize::<String>().ok());
    owner.as_deref() == Some(sender.as_str())
}

/// Every signal the shell needs from the notification server, on one stream.
/// Only the server's own count: any session-bus peer can emit an
/// `ActionInvoked` with one of our ids.
async fn listen(conn: Connection) {
    let rule = match MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(SERVICE)
        .and_then(|b| b.interface(SERVICE))
        .map(|b| b.build())
    {
        Ok(rule) => rule,
        Err(e) => {
            eprintln!("talaria-desktop: the notification signals: {e}");
            return;
        }
    };
    let mut stream = match MessageStream::for_match_rule(rule, &conn, Some(256)).await {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("talaria-desktop: subscribing to notification signals: {e}");
            return;
        }
    };
    let mut owner = None;
    while let Some(message) = stream.next().await {
        let Ok(message) = message else {
            continue;
        };
        let header = message.header();
        let Some(member) = header.member() else {
            continue;
        };
        if !from_server(&conn, &mut owner, header.sender()).await {
            eprintln!(
                "talaria-desktop: ignored {member} from {}, which is not the notification server",
                header.sender().map_or("nobody", |s| s.as_str())
            );
            continue;
        }
        let body = message.body();
        match member.as_str() {
            "ActivationToken" => {
                if let Ok((id, token)) = body.deserialize::<(u32, String)>() {
                    let mut tokens = TOKENS.lock().unwrap();
                    tokens.retain(|(i, _)| *i != id);
                    tokens.push((id, token));
                    if tokens.len() > 32 {
                        tokens.remove(0);
                    }
                }
            }
            "ActionInvoked" => {
                if let Ok((id, key)) = body.deserialize::<(u32, String)>()
                    && key == "default"
                {
                    let token = {
                        let mut tokens = TOKENS.lock().unwrap();
                        let at = tokens.iter().position(|(i, _)| *i == id);
                        at.map(|at| tokens.remove(at).1)
                    };
                    super::clicked_server_id(id, token);
                }
            }
            "NotificationClosed" => {
                if let Ok((id, _reason)) = body.deserialize::<(u32, u32)>() {
                    TOKENS.lock().unwrap().retain(|(i, _)| *i != id);
                    super::closed_server_id(id);
                }
            }
            _ => {}
        }
    }
    // The stream ends with the connection: the next post opens a new one.
    let mut slot = CONN.lock().unwrap();
    if slot
        .as_ref()
        .is_some_and(|c| c.unique_name() == conn.unique_name())
    {
        *slot = None;
    }
}

/// Whether a tray host is on the bus now, and every change after: enabling
/// the GNOME AppIndicator extension later takes effect without a restart.
async fn watch_tray(conn: Connection) {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender("org.freedesktop.DBus")
        .and_then(|b| b.interface("org.freedesktop.DBus"))
        .and_then(|b| b.member("NameOwnerChanged"))
        .and_then(|b| b.arg(0, WATCHER))
        .map(|b| b.build());
    let mut stream = match rule {
        Ok(rule) => match MessageStream::for_match_rule(rule, &conn, Some(16)).await {
            Ok(stream) => Some(stream),
            Err(e) => {
                eprintln!("talaria-desktop: watching for a tray host: {e}");
                None
            }
        },
        Err(e) => {
            eprintln!("talaria-desktop: watching for a tray host: {e}");
            None
        }
    };
    let owned = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(WATCHER,),
        )
        .await
        .ok()
        .and_then(|r| r.body().deserialize::<bool>().ok())
        .unwrap_or(false);
    crate::background::tray_host_changed(owned);
    let Some(stream) = stream.as_mut() else {
        return;
    };
    while let Some(message) = stream.next().await {
        let Ok(message) = message else {
            continue;
        };
        if let Ok((_name, _old, new)) = message.body().deserialize::<(String, String, String)>() {
            crate::background::tray_host_changed(!new.is_empty());
        }
    }
}

/// `app.talaria.desktop.Shell`: what a second launch calls.
struct Shell;

#[zbus::interface(name = "app.talaria.desktop.Shell")]
impl Shell {
    /// Bring the running instance forward, with the launcher's activation
    /// token when it passed one (xdg-activation: without it, a Wayland
    /// compositor leaves the window behind).
    async fn activate(&self, token: String) {
        eprintln!(
            "talaria-desktop: a second launch asked for the window{}",
            if token.is_empty() {
                ""
            } else {
                " (with an activation token)"
            }
        );
        if let Some(app) = super::app() {
            let token = (!token.is_empty()).then_some(token);
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || crate::background::present_main(&handle, token));
        }
    }
}

/// Claim the single-instance name. True when another Talaria already holds
/// it and has been asked to come forward: this process exits 0 before it
/// builds a window. A bus that is not there, or an owner that does not
/// answer, lets this one run.
pub fn single_instance() -> bool {
    tauri::async_runtime::block_on(async {
        let conn = match connection().await {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("talaria-desktop: {e}; no single-instance check this run");
                return false;
            }
        };
        match conn
            .request_name_with_flags(SHELL_NAME, RequestNameFlags::DoNotQueue.into())
            .await
        {
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
                CLAIMED.store(true, Ordering::Relaxed);
                false
            }
            // zbus answers a DoNotQueue request for an owned name with
            // NameTaken; a bus that queued it anyway says InQueue/Exists.
            Ok(_) | Err(zbus::Error::NameTaken) => {
                let token = std::env::var("XDG_ACTIVATION_TOKEN")
                    .or_else(|_| std::env::var("DESKTOP_STARTUP_ID"))
                    .unwrap_or_default();
                let asked = tokio::time::timeout(
                    Duration::from_secs(2),
                    conn.call_method(
                        Some(SHELL_NAME),
                        SHELL_PATH,
                        Some(SHELL_NAME),
                        "Activate",
                        &(token.as_str(),),
                    ),
                )
                .await;
                match asked {
                    Ok(Ok(_)) => {
                        eprintln!(
                            "talaria-desktop: Talaria is already running; brought it forward{}",
                            if cfg!(debug_assertions) {
                                " (this is a dev build: quit the running Talaria first — Ctrl+Q, or its tray or switcher Quit — to run this one)"
                            } else {
                                ""
                            }
                        );
                        true
                    }
                    Ok(Err(e)) => {
                        eprintln!(
                            "talaria-desktop: the running Talaria did not answer ({e}); starting anyway"
                        );
                        false
                    }
                    Err(_) => {
                        eprintln!(
                            "talaria-desktop: the running Talaria did not answer in 2 s; starting anyway"
                        );
                        false
                    }
                }
            }
            Err(e) => {
                eprintln!("talaria-desktop: claiming {SHELL_NAME}: {e}");
                false
            }
        }
    })
}

/// Give up the single-instance name before the process goes (`RunEvent::Exit`).
/// The updater's relaunch spawns the new process before this one exits: a new
/// process that still found the name held would hand over to this dying one
/// and exit too, leaving nothing running. Bounded; the bus frees the name at
/// exit anyway.
pub fn release_single_instance() {
    if !CLAIMED.swap(false, Ordering::Relaxed) {
        return;
    }
    let Some(conn) = CONN.lock().unwrap().clone() else {
        return;
    };
    tauri::async_runtime::block_on(async move {
        match tokio::time::timeout(Duration::from_secs(1), conn.release_name(SHELL_NAME)).await {
            Ok(Ok(_)) => eprintln!("talaria-desktop: released {SHELL_NAME}"),
            Ok(Err(e)) => eprintln!("talaria-desktop: releasing {SHELL_NAME}: {e}"),
            Err(_) => eprintln!("talaria-desktop: releasing {SHELL_NAME}: no answer in 1 s"),
        }
    });
}

/// The self-test: the server and one real post. Run it on a test bus only.
pub fn selftest() -> serde_json::Value {
    tauri::async_runtime::block_on(async {
        let status = probe().await;
        let post = Post {
            label: "main".into(),
            instance: "Talaria".into(),
            title: super::TEST_TITLE.into(),
            body: Some("the self-test".into()),
            tag: "selftest".into(),
            href: None,
            kind: None,
            renotify: false,
            source: super::Source::Page,
        };
        let posted = match self::post(&post, None).await {
            Ok(id) => format!("ok (id {id})"),
            Err(e) => format!("error: {e}"),
        };
        serde_json::json!({
            "backend": status.backend,
            "bundleId": desktop_entry().id,
            "authorization": status.permission,
            "detail": status.detail,
            "post": posted,
            "background": if crate::background::tray_library() { "tray-capable" } else { "minimize" },
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/home/u/.local/share"),
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]
    }

    #[test]
    fn a_sandboxed_or_appimage_run_is_the_app_id() {
        let none = |_: &Path| false;
        assert_eq!(
            detect_desktop_id(Some("app.talaria.desktop"), None, &dirs(), none).0,
            APP_ID
        );
        assert_eq!(
            detect_desktop_id(None, Some("/opt/Talaria.AppImage"), &dirs(), |_| true).0,
            APP_ID
        );
    }

    #[test]
    fn the_installed_file_wins_app_id_first_then_the_product_name() {
        let only = |want: &'static str| move |p: &Path| p == Path::new(want);
        assert_eq!(
            detect_desktop_id(
                None,
                None,
                &dirs(),
                only("/usr/share/applications/Talaria.desktop")
            ),
            (
                "Talaria".to_string(),
                Some(PathBuf::from("/usr/share/applications/Talaria.desktop"))
            )
        );
        let both = |p: &Path| {
            p == Path::new("/usr/share/applications/Talaria.desktop")
                || p == Path::new("/usr/share/applications/app.talaria.desktop.desktop")
        };
        assert_eq!(detect_desktop_id(None, None, &dirs(), both).0, APP_ID);
        // The user's own data dir counts first.
        assert_eq!(
            detect_desktop_id(
                None,
                None,
                &dirs(),
                only("/home/u/.local/share/applications/app.talaria.desktop.desktop")
            )
            .1,
            Some(PathBuf::from(
                "/home/u/.local/share/applications/app.talaria.desktop.desktop"
            ))
        );
        assert_eq!(
            detect_desktop_id(None, None, &dirs(), |_| false),
            (APP_ID.to_string(), None)
        );
        assert_eq!(
            detect_desktop_id(Some(""), Some(""), &dirs(), |_| false).0,
            APP_ID
        );
    }

    #[test]
    fn the_data_dirs_follow_the_spec_defaults() {
        assert_eq!(data_dirs(Some("/home/u"), None, None), dirs());
        assert_eq!(
            data_dirs(Some("/home/u"), Some("/x"), Some("/a:/b:")),
            vec![
                PathBuf::from("/x"),
                PathBuf::from("/a"),
                PathBuf::from("/b")
            ]
        );
    }

    #[test]
    fn the_icon_comes_from_the_main_group() {
        let file =
            "[Desktop Entry]\nName=Talaria\nIcon=talaria-desktop\n[Desktop Action x]\nIcon=other\n";
        assert_eq!(icon_of(file).as_deref(), Some("talaria-desktop"));
        assert_eq!(icon_of("[Other]\nIcon=x\n"), None);
        assert_eq!(icon_of("[Desktop Entry]\nIcon=\n"), None);
    }

    #[test]
    fn the_body_names_the_instance_and_is_escaped_for_markup() {
        assert_eq!(body_text("Acme", Some("hi"), false), "Acme: hi");
        assert_eq!(body_text("Acme", None, false), "Acme");
        assert_eq!(body_text("Acme", Some(""), true), "Acme");
        assert_eq!(
            body_text("A&B", Some("<b>&"), true),
            "A&amp;B: &lt;b&gt;&amp;"
        );
        assert_eq!(body_text("A&B", Some("<b>&"), false), "A&B: <b>&");
        assert_eq!(
            body_text("", Some("the shell's own"), false),
            "the shell's own"
        );
    }
}
