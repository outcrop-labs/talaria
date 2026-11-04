//! Windows: toasts under the app's own AUMID. Wave 1 is attribution only.
//!
//! notify-rust with no `app_id` posts as `Toast::POWERSHELL_APP_ID`, so every
//! Talaria toast said "Windows PowerShell" (D6). At start the shell picks the
//! attribution it can stand behind, in order:
//!
//!   1. `shortcut` — a Start-menu shortcut already carries AUMID
//!      `app.talaria.desktop` (the MSI's does; the shortcut's property store
//!      holds the id as UTF-16, which is what is looked for);
//!   2. `registry` — otherwise `HKCU\Software\Classes\AppUserModelId\
//!      app.talaria.desktop` is written with `DisplayName=Talaria` and an
//!      `IconUri`, which Windows accepts for an unpackaged app;
//!   3. `powershell` — if that write fails, today's visible-but-misattributed
//!      path.
//!
//! The process takes the same id (`SetCurrentProcessExplicitAppUserModelID`)
//! unless it fell back to PowerShell's. The body click, late clicks after a
//! restart, the tray, close-to-background and the single-instance guard are
//! the next round (W2-D): Windows keeps quit-on-close, because a hidden
//! window with no single-instance guard would let a Start-menu relaunch open
//! a second process on the same WebView2 profile.

use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use tauri::{AppHandle, Manager};

use super::{OsStatus, Permission, Post};

pub const AUMID: &str = "app.talaria.desktop";

/// No console window for the `reg` child.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribution {
    Shortcut,
    Registry,
    Powershell,
}

impl Attribution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shortcut => "shortcut",
            Self::Registry => "registry",
            Self::Powershell => "powershell",
        }
    }
}

static ATTRIBUTION: OnceLock<Attribution> = OnceLock::new();

#[link(name = "shell32")]
unsafe extern "system" {
    fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> i32;
}

fn utf16z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// True when `bytes` (a `.lnk`) holds the AUMID as UTF-16LE.
pub fn holds_aumid(bytes: &[u8]) -> bool {
    let needle: Vec<u8> = AUMID.encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes.windows(needle.len()).any(|w| w == needle.as_slice())
}

/// Every Start-menu shortcut named for Talaria, user's and machine's, one
/// folder deep.
fn shortcuts() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        roots.push(Path::new(&appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(data) = std::env::var_os("ProgramData") {
        roots.push(Path::new(&data).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    let mut out = Vec::new();
    for root in roots {
        let mut dirs = vec![root.clone()];
        if let Ok(entries) = std::fs::read_dir(&root) {
            dirs.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            out.extend(entries.flatten().map(|e| e.path()).filter(|p| {
                p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"))
                    && p.file_stem()
                        .is_some_and(|s| s.to_string_lossy().to_lowercase().contains("talaria"))
            }));
        }
    }
    out
}

fn reg_add(value: &str, data: &str) -> Result<(), String> {
    let key = format!(r"HKCU\Software\Classes\AppUserModelId\{AUMID}");
    let status = Command::new("reg")
        .args(["add", &key, "/v", value, "/t", "REG_SZ", "/d", data, "/f"])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("running reg: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("reg add {value} exited {status}"))
    }
}

/// The AUMID's registry entry, with the app's icon written next to its data.
fn register(app: &AppHandle) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("the app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let icon = dir.join("notification-icon.png");
    std::fs::write(&icon, include_bytes!("../../icons/128x128.png"))
        .map_err(|e| format!("writing {}: {e}", icon.display()))?;
    reg_add("DisplayName", "Talaria")?;
    reg_add("IconUri", &icon.to_string_lossy())
}

fn choose(app: &AppHandle) -> Attribution {
    if shortcuts()
        .iter()
        .any(|p| std::fs::read(p).is_ok_and(|bytes| holds_aumid(&bytes)))
    {
        return Attribution::Shortcut;
    }
    match register(app) {
        Ok(()) => Attribution::Registry,
        Err(e) => {
            eprintln!("talaria-desktop: registering the toast identity: {e}");
            Attribution::Powershell
        }
    }
}

fn attribution() -> Attribution {
    ATTRIBUTION
        .get()
        .copied()
        .unwrap_or(Attribution::Powershell)
}

/// Pick the attribution and give the process the id. In setup, once.
pub fn init(app: &AppHandle) {
    let chosen = *ATTRIBUTION.get_or_init(|| choose(app));
    if chosen != Attribution::Powershell {
        let id = utf16z(AUMID);
        // SAFETY: a NUL-terminated UTF-16 string that outlives the call.
        let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
        if hr != 0 {
            eprintln!("talaria-desktop: SetCurrentProcessExplicitAppUserModelID: 0x{hr:08x}");
        }
    }
    eprintln!("talaria-desktop: toasts attributed by {}", chosen.as_str());
}

fn backend() -> &'static str {
    match attribution() {
        Attribution::Powershell => "winrt-powershell",
        _ => "winrt",
    }
}

/// The toast, built. Its handle's type is private to notify-rust on Windows,
/// so callers `show()` it and never name the result.
fn notification(post: &Post) -> notify_rust::Notification {
    let mut notification = notify_rust::Notification::new();
    notification.summary(&post.title);
    if !post.instance.is_empty() {
        notification.subtitle(&post.instance);
    }
    if let Some(body) = &post.body {
        notification.body(body);
    }
    if attribution() != Attribution::Powershell {
        notification.app_id(AUMID);
    }
    notification.action("default", "Open");
    notification
}

pub async fn post(post: &Post, _previous: Option<u32>) -> Result<u32, String> {
    let post = post.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let handle = notification(&post)
            .show()
            .map_err(|e| format!("showing the notification: {e}"))?;
        // The waiter for the "Open" button, as before: the body click and
        // late clicks are W2-D.
        let (label, tag) = (post.label.clone(), post.tag.clone());
        std::thread::spawn(move || {
            handle.wait_for_action(|action| {
                if action == "default" {
                    super::clicked_tag(&label, &tag);
                }
            });
        });
        Ok(0)
    })
    .await
    .map_err(|e| format!("posting the notification: {e}"))?
}

pub async fn probe() -> OsStatus {
    OsStatus {
        backend: backend().into(),
        permission: Permission::Granted,
        server: None,
        capabilities: None,
        detail: Some(format!(
            "attributed by {} ({AUMID})",
            attribution().as_str()
        )),
    }
}

pub async fn request() -> OsStatus {
    probe().await
}

/// Windows toasts are not closed by a read elsewhere yet: notify-rust has no
/// remove, and the toast history is the cut Windows round (docs/DESKTOP.md).
pub async fn close(_label: &str, _tags: &[String], _entries: &[super::Entry]) -> u32 {
    0
}

/// `LASTINPUTINFO`.
#[repr(C)]
struct LastInputInfo {
    size: u32,
    time: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetLastInputInfo(info: *mut LastInputInfo) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount() -> u32;
}

/// Milliseconds since the last input in this session: the tick count now
/// minus the last input's (both wrap at 49.7 days, so the difference wraps).
pub async fn os_idle() -> Option<u64> {
    let mut info = LastInputInfo {
        size: std::mem::size_of::<LastInputInfo>() as u32,
        time: 0,
    };
    // SAFETY: a properly sized LASTINPUTINFO that outlives the call.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // SAFETY: no arguments.
    let now = unsafe { GetTickCount() };
    Some(u64::from(now.wrapping_sub(info.time)))
}

/// No badge on Windows in wave 1 (the taskbar overlay is W2-D).
pub fn set_badge(_app: &AppHandle, _total: u32) {}

/// The self-test (desktop-package.yml): which attribution this install got,
/// and one real toast. It runs before Tauri, so it picks the attribution
/// itself, without the app data dir (the registry path writes no icon then).
pub fn selftest() -> serde_json::Value {
    let chosen = *ATTRIBUTION.get_or_init(|| {
        if shortcuts()
            .iter()
            .any(|p| std::fs::read(p).is_ok_and(|bytes| holds_aumid(&bytes)))
        {
            Attribution::Shortcut
        } else if reg_add("DisplayName", "Talaria").is_ok() {
            Attribution::Registry
        } else {
            Attribution::Powershell
        }
    });
    if chosen != Attribution::Powershell {
        let id = utf16z(AUMID);
        // SAFETY: a NUL-terminated UTF-16 string that outlives the call.
        let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
    }
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
    let posted = match notification(&post).show() {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("error: {e}"),
    };
    serde_json::json!({
        "backend": backend(),
        "attribution": chosen.as_str(),
        "aumid": if chosen == Attribution::Powershell { "powershell" } else { AUMID },
        "bundleId": AUMID,
        "authorization": "n/a",
        "post": posted,
        "close": "skipped: Windows toasts are not closed by a read yet",
        "osIdle": tauri::async_runtime::block_on(os_idle()),
        "background": "none",
    })
}
