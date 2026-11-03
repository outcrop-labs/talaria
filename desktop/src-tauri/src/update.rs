//! The update chip: the shell looks for a new Talaria Desktop on its own and
//! offers it where the person is, with Update and Later.
//!
//! - **The check** is the launcher's bounded one (`check`, 10 s), on a tokio
//!   worker: `FIRST_CHECK` after launch, then every `CHECK_EVERY_MS`, or
//!   `RETRY_MS` after one that failed. A failed background check is a log
//!   line, never a chip. A dev build (no `custom-protocol`) never checks on
//!   its own: its version is the decorative one in tauri.conf.json.
//! - **The offer** is the shell's own, one for the whole window. The launcher
//!   renders it from the snapshot (`ShellSnapshot.update`), and every
//!   instance page gets the same chip from `chip_js`, a script the shell
//!   evaluates into the page. So it reaches an instance running any Talaria
//!   version, and a page that is half up, with no help from the instance's
//!   UI.
//! - **Later** puts that version off for `SNOOZE_MS`, in settings.json, so a
//!   relaunch keeps it put off. A newer version than the one put off is
//!   offered at once. Later from any view takes the chip down in every view.
//! - **Update** is `install`, where the updater can replace this copy:
//!   Windows' NSIS install, the macOS app, a Linux AppImage. A Linux package
//!   (deb, rpm, pacman, Flatpak) is the package manager's to update and an
//!   `.msi` install takes a new `.msi` (latest.json carries only the AppImage
//!   and the NSIS setup, so the updater would install the wrong thing there):
//!   the chip says how, and offers Later only.

use std::{
    path::Path,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, utils::config::BundleType};
use tauri_plugin_updater::UpdaterExt;

use crate::{ShellState, beacon, commands, registry, settings};

/// How long after launch the first check runs: after the launch-time loads
/// (commands::schedule_prewarm, 1 s then every 5 s, five at most).
const FIRST_CHECK: Duration = Duration::from_secs(30);
/// How often the shell looks again, by the wall clock, so a laptop that slept
/// through the interval checks when it wakes.
const CHECK_EVERY_MS: u64 = 6 * 60 * 60 * 1000;
/// After a check that failed (offline, GitHub down).
const RETRY_MS: u64 = 60 * 60 * 1000;
/// How often the loop wakes to see whether a check is due or a snooze has
/// run out. No network unless a check is due.
const TICK: Duration = Duration::from_secs(10 * 60);
/// How long Later puts a version off.
pub const SNOOZE_MS: u64 = 24 * 60 * 60 * 1000;
/// The update check's bound. GitHub answers in about a second and a half; the
/// plugin's default is no bound at all, which was 30 s of a disabled button
/// on a network that drops packets.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Update's own look at the feed, before it downloads. Longer than the
/// background check's: someone is waiting on the answer.
const INSTALL_CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// The download's bounds. The payload is the whole installer, so there is no
/// short total; a download that stops arriving for `DOWNLOAD_STALL` is a
/// failure, and so is one still going after `DOWNLOAD_CAP`. Installing the
/// bytes once they are here is local and never cut off.
pub const DOWNLOAD_STALL: Duration = Duration::from_secs(60);
pub const DOWNLOAD_CAP: Duration = Duration::from_secs(30 * 60);
/// How often the download's bounds are looked at.
const DOWNLOAD_LOOK: Duration = Duration::from_secs(5);

/// Where a person gets the installers.
const RELEASES: &str = "github.com/outcrop-labs/talaria/releases";

/// The wall clock, in ms since the epoch.
fn now_ms() -> u64 {
    u64::try_from(registry::now_ms()).unwrap_or(0)
}

/// A version Later put off, and until when (ms since the epoch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snooze {
    pub version: String,
    pub until_ms: u64,
}

impl Snooze {
    pub fn new(version: &str, now_ms: u64) -> Self {
        Self {
            version: version.to_string(),
            until_ms: now_ms.saturating_add(SNOOZE_MS),
        }
    }
}

/// Is `version` put off at `now_ms`? Not when nothing was put off, when the
/// snooze has run out, or when `version` is newer than the one put off. A
/// snooze that ends further out than one window can (the clock went back, a
/// hand-edited file) has run out too: no snooze outlives a day.
pub fn snoozed(version: &str, snooze: Option<&Snooze>, now_ms: u64) -> bool {
    let Some(snooze) = snooze else {
        return false;
    };
    if now_ms >= snooze.until_ms || snooze.until_ms - now_ms > SNOOZE_MS {
        return false;
    }
    !newer(version, &snooze.version)
}

/// `a` is a later version than `b`. Versions that do not parse are compared
/// as text: a different one counts as newer, so it is offered.
fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| semver::Version::parse(v.trim().trim_start_matches('v'));
    match (parse(a), parse(b)) {
        (Ok(a), Ok(b)) => a > b,
        _ => a.trim() != b.trim(),
    }
}

/// How this copy of Talaria gets a new version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// The updater replaces it in place: the chip offers Update.
    Updater,
    /// Something else does, in these words: the chip offers Later only.
    Elsewhere(&'static str),
}

const LINUX_PACKAGE: &str = "This copy was installed from a .deb, .rpm or pacman package, so it \
     cannot update itself: install the new package from github.com/outcrop-labs/talaria/releases.";
const FLATPAK: &str = "This copy is a Flatpak, so it cannot update itself: install the new \
     .flatpak from github.com/outcrop-labs/talaria/releases.";
const MSI: &str = "This copy was installed from an .msi, so it cannot update itself: run the new \
     .msi from github.com/outcrop-labs/talaria/releases.";
const MAC_DISK_IMAGE: &str = "Talaria is running from its disk image or a quarantine copy, where \
     it cannot replace itself: drag it into Applications, open it from there, then update.";

/// What decides `Installed`, read from the running process (`installed`).
/// Linux: only an AppImage is the updater's to replace (`$APPIMAGE`, as the
/// updater itself reads it). A deb or rpm would download the AppImage the
/// feed carries and refuse it, and the pacman package and the Flatpak are
/// read-only. Windows: the NSIS setup is the feed's; an `.msi` install would
/// get a second, per-user NSIS copy beside it. macOS: the app replaces itself,
/// unless it runs from a read-only place.
pub fn classify(
    os: &str,
    appimage: bool,
    flatpak: bool,
    bundle: Option<BundleType>,
    exe: Option<&Path>,
) -> Installed {
    match os {
        "linux" if flatpak => Installed::Elsewhere(FLATPAK),
        "linux" if appimage => Installed::Updater,
        "linux" => Installed::Elsewhere(LINUX_PACKAGE),
        "windows" if bundle == Some(BundleType::Msi) => Installed::Elsewhere(MSI),
        "macos" if exe.is_some_and(read_only_mac_place) => Installed::Elsewhere(MAC_DISK_IMAGE),
        _ => Installed::Updater,
    }
}

/// A mounted disk image, or the read-only copy Gatekeeper runs a quarantined
/// app from (App Translocation): the updater cannot replace the app there.
fn read_only_mac_place(exe: &Path) -> bool {
    let path = exe.to_string_lossy();
    path.starts_with("/Volumes/") || path.contains("/AppTranslocation/")
}

/// This process's `Installed`.
pub fn installed(app: &AppHandle) -> Installed {
    #[cfg(target_os = "linux")]
    let appimage = app.env().appimage.is_some();
    #[cfg(not(target_os = "linux"))]
    let appimage = {
        let _ = app;
        false
    };
    let flatpak = std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists();
    let exe = tauri::utils::platform::current_exe().ok();
    classify(
        std::env::consts::OS,
        appimage,
        flatpak,
        tauri::utils::platform::bundle_type(),
        exe.as_deref(),
    )
}

/// What the chip shows, in every view. Serialized for the launcher (the
/// snapshot) and for the page script (`chip_js`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub version: String,
    /// `updater`: Update installs it here. `elsewhere`: `how` says what does.
    pub install: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
    /// An install is running.
    pub busy: bool,
    /// The last install failed, in a sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The offer, from its parts. None: nothing newer, or it is put off. An
/// install already running is never hidden by a snooze, and neither is the
/// error of one that failed: both answer a click.
pub fn offer(
    available: Option<&str>,
    snooze: Option<&Snooze>,
    now_ms: u64,
    installed: Installed,
    busy: bool,
    error: Option<&str>,
) -> Option<Offer> {
    let version = available?;
    if !busy && error.is_none() && snoozed(version, snooze, now_ms) {
        return None;
    }
    let (install, how) = match installed {
        Installed::Updater => ("updater", None),
        Installed::Elsewhere(how) => ("elsewhere", Some(how.to_string())),
    };
    Some(Offer {
        version: version.to_string(),
        install,
        how,
        busy,
        error: error.map(str::to_string),
    })
}

/// The shell's update state (managed beside `ShellState`). The snooze itself
/// lives in the settings, so it survives a relaunch.
#[derive(Default)]
pub struct Updates {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The version the last check found, when it is newer than this one.
    available: Option<String>,
    /// When the loop checks next (wall-clock ms); 0 is now.
    next_check_ms: u64,
    installing: bool,
    error: Option<String>,
    /// What the views were last given, so a tick that changes nothing pushes
    /// nothing.
    pushed: Option<Offer>,
}

impl Updates {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The offer as it stands now. One lock at a time (lib.rs's discipline).
pub fn current(app: &AppHandle) -> Option<Offer> {
    let updates = app.try_state::<Updates>()?;
    let (available, busy, error) = {
        let inner = updates.lock();
        (
            inner.available.clone(),
            inner.installing,
            inner.error.clone(),
        )
    };
    let snooze = app
        .try_state::<ShellState>()?
        .settings
        .lock()
        .unwrap()
        .update_snooze
        .clone();
    offer(
        available.as_deref(),
        snooze.as_ref(),
        now_ms(),
        installed(app),
        busy,
        error.as_deref(),
    )
}

/// Give every view the offer as it stands, when it changed: the launcher by
/// the snapshot, each instance page by `chip_js`. Main thread, like every
/// other `eval`.
pub fn push(app: &AppHandle) {
    let offer = current(app);
    {
        let Some(updates) = app.try_state::<Updates>() else {
            return;
        };
        let mut inner = updates.lock();
        if inner.pushed == offer {
            return;
        }
        inner.pushed = offer.clone();
    }
    match &offer {
        Some(o) => eprintln!(
            "talaria-desktop: update chip: {} ({}{}{})",
            o.version,
            o.install,
            if o.busy { ", installing" } else { "" },
            if o.error.is_some() { ", failed" } else { "" }
        ),
        None => eprintln!("talaria-desktop: update chip: none"),
    }
    commands::emit_state(app);
    let js = chip_js(offer.as_ref());
    for webview in commands::instance_pages(app) {
        let _ = webview.eval(&js);
    }
}

/// `push`, from wherever: a tokio worker or a command. Queued, so it never
/// runs inside a WebView2 handler.
fn push_soon(app: &AppHandle) {
    commands::queue_main_turn(app, push);
}

/// A page finished loading: it has no chip yet, whatever the others show.
pub fn page_loaded(app: &AppHandle, webview: &tauri::Webview) {
    if let Some(offer) = current(app) {
        let _ = webview.eval(chip_js(Some(&offer)));
    }
}

/// The automatic check: `FIRST_CHECK` after launch, then on the schedule.
pub fn schedule(app: &AppHandle) {
    if tauri::is_dev() {
        eprintln!("talaria-desktop: a dev build; no automatic update check");
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let due = {
                let Some(updates) = app.try_state::<Updates>() else {
                    return;
                };
                now_ms() >= updates.lock().next_check_ms
            };
            if due {
                let found = check(&app).await;
                let now = now_ms();
                let updates = app.state::<Updates>();
                let mut inner = updates.lock();
                match found {
                    Ok(found) => {
                        eprintln!(
                            "talaria-desktop: update check: {}",
                            found
                                .as_ref()
                                .map_or("nothing newer", |u| u.version.as_str())
                        );
                        inner.available = found.map(|u| u.version);
                        inner.next_check_ms = now.saturating_add(CHECK_EVERY_MS);
                    }
                    Err(e) => {
                        eprintln!("talaria-desktop: update check: {e}; again in an hour");
                        inner.next_check_ms = now.saturating_add(RETRY_MS);
                    }
                }
            }
            // A snooze that ran out shows the chip again on this tick.
            let handle = app.clone();
            if app.run_on_main_thread(move || push(&handle)).is_err() {
                return;
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
    /// `updater`, or `elsewhere` with `how` (Offer.install).
    pub install: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
}

/// Compare this build to the feed's `latest.json`. None: nothing newer.
pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| format!("Could not check for updates: {e}"))?;
    let found = updater.check().await.map_err(check_failure)?;
    let (install, how) = match installed(app) {
        Installed::Updater => ("updater", None),
        Installed::Elsewhere(how) => ("elsewhere", Some(how.to_string())),
    };
    Ok(found.map(|update| UpdateInfo {
        version: update.version,
        notes: update.body,
        install,
        how,
    }))
}

/// The launcher's and Settings → Profile's "Check for updates": the same
/// check, and what it found becomes the offer too.
pub async fn check_now(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let found = check(app).await?;
    {
        let updates = app.state::<Updates>();
        let mut inner = updates.lock();
        inner.available = found.as_ref().map(|u| u.version.clone());
        inner.next_check_ms = now_ms().saturating_add(CHECK_EVERY_MS);
    }
    push_soon(app);
    Ok(found)
}

/// Later: put the version on offer off for a day, everywhere. Nothing on
/// offer, nothing to do. The page never says which version: it is the one
/// the shell found.
pub fn snooze(app: &AppHandle) -> Result<(), String> {
    let Some(version) = app.state::<Updates>().lock().available.clone() else {
        return Ok(());
    };
    let state = app.state::<ShellState>();
    {
        let mut current = state.settings.lock().unwrap();
        current.update_snooze = Some(Snooze::new(&version, now_ms()));
        settings::save(&state.settings_path, &current)?;
    }
    app.state::<Updates>().lock().error = None;
    eprintln!("talaria-desktop: update {version} put off for a day");
    push_soon(app);
    Ok(())
}

/// Update: download the signed payload, apply it, relaunch. The updater
/// verifies the minisign signature against the pubkey in tauri.conf.json
/// before it touches the install. No short bound here: the download is the
/// whole installer, and the plugin applies one timeout to check and download.
/// On Windows the NSIS installer takes over and the plugin exits the app;
/// it relaunches it when it is done (`installMode: passive`).
pub async fn install(app: &AppHandle) -> Result<(), String> {
    if let Installed::Elsewhere(how) = installed(app) {
        return Err(how.to_string());
    }
    {
        let updates = app.state::<Updates>();
        let mut inner = updates.lock();
        if inner.installing {
            return Err("Talaria is already installing the update.".into());
        }
        inner.installing = true;
        inner.error = None;
    }
    push_soon(app);
    let outcome: Result<Option<String>, String> = async {
        let updater = app
            .updater()
            .map_err(|e| format!("Could not start the update: {e}."))?;
        let Some(update) = tokio::time::timeout(INSTALL_CHECK_TIMEOUT, updater.check())
            .await
            .map_err(|_| {
                format!(
                    "Could not check for updates: the update server did not answer within {} seconds.",
                    INSTALL_CHECK_TIMEOUT.as_secs()
                )
            })?
            .map_err(check_failure)?
        else {
            return Ok(None);
        };
        let version = update.version.clone();
        eprintln!("talaria-desktop: downloading update {version}");
        let bytes = download_watched(&update).await?;
        eprintln!(
            "talaria-desktop: downloaded update {version} ({} bytes); installing",
            bytes.len()
        );
        update.install(bytes).map_err(install_failure)?;
        Ok(Some(version))
    }
    .await;
    let finish = |error: Option<String>, available: Option<Option<String>>| {
        let updates = app.state::<Updates>();
        let mut inner = updates.lock();
        inner.installing = false;
        inner.error = error;
        if let Some(available) = available {
            inner.available = available;
        }
    };
    match outcome {
        Ok(Some(version)) => {
            eprintln!("talaria-desktop: installed update {version}; restarting");
            // From a tokio worker: through ExitRequested and Exit, so every
            // page is remembered and the single-instance name is free first.
            app.restart();
        }
        Ok(None) => {
            finish(None, Some(None));
            push_soon(app);
            Err("Talaria is already on the latest version.".into())
        }
        Err(e) => {
            eprintln!("talaria-desktop: update: {e}");
            finish(Some(e.clone()), None);
            push_soon(app);
            Err(e)
        }
    }
}

/// The update's download, bounded by `download_stalled`: a download that is
/// dropped stops, and the chip says so and offers Update again.
async fn download_watched(update: &tauri_plugin_updater::Update) -> Result<Vec<u8>, String> {
    let last = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_ms()));
    let arrived = last.clone();
    let started = std::time::Instant::now();
    let download = update.download(
        move |_, _| arrived.store(now_ms(), std::sync::atomic::Ordering::Relaxed),
        || {},
    );
    let mut download = std::pin::pin!(download);
    loop {
        match tokio::time::timeout(DOWNLOAD_LOOK, &mut download).await {
            Ok(done) => return done.map_err(install_failure),
            Err(_) => {
                let idle = now_ms().saturating_sub(last.load(std::sync::atomic::Ordering::Relaxed));
                if let Some(why) = download_stalled(idle, started.elapsed()) {
                    eprintln!("talaria-desktop: update download: {why}");
                    return Err(why);
                }
            }
        }
    }
}

/// Is a download that has run for `elapsed`, with nothing arriving for the
/// last `idle_ms`, past its bounds? The sentence for the chip if so.
pub fn download_stalled(idle_ms: u64, elapsed: Duration) -> Option<String> {
    if idle_ms >= DOWNLOAD_STALL.as_millis() as u64 {
        return Some(format!(
            "The download stopped: nothing arrived for {} seconds, so nothing was installed. Try Update again.",
            DOWNLOAD_STALL.as_secs()
        ));
    }
    if elapsed >= DOWNLOAD_CAP {
        return Some(format!(
            "The download took more than {} minutes, so it was stopped and nothing was installed. Try Update again.",
            DOWNLOAD_CAP.as_secs() / 60
        ));
    }
    None
}

/// A failed check in a sentence: a timeout says so, a transport error says
/// which (beacon::classify), and the rest keep the plugin's own text.
pub fn check_failure(e: tauri_plugin_updater::Error) -> String {
    match e {
        tauri_plugin_updater::Error::Reqwest(e) => {
            let host = e
                .url()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| "the update server".into());
            if e.is_timeout() {
                format!(
                    "Could not check for updates: {host} did not answer within {} seconds.",
                    CHECK_TIMEOUT.as_secs()
                )
            } else {
                let url = e
                    .url()
                    .cloned()
                    .unwrap_or_else(|| "https://github.com".parse().unwrap());
                format!(
                    "Could not check for updates: {}.",
                    beacon::classify(&e).reason(&url)
                )
            }
        }
        // The feed answered, but not with a release: an error page, a 5xx, a
        // file that does not parse. The plugin's own words say "JSON".
        tauri_plugin_updater::Error::ReleaseNotFound => {
            "Could not check for updates: the update server did not answer with a release.".into()
        }
        other => sentence("Could not check for updates", &other.to_string()),
    }
}

/// A failed download or install in a sentence a person can act on.
pub fn install_failure(e: tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error as E;
    match e {
        E::Minisign(_) | E::Base64(_) | E::SignatureUtf8(_) => {
            "The download did not match Talaria's signature, so nothing was installed.".into()
        }
        E::Reqwest(e) => {
            let url = e
                .url()
                .cloned()
                .unwrap_or_else(|| "https://github.com".parse().unwrap());
            format!(
                "The update could not be downloaded: {}.",
                beacon::classify(&e).reason(&url)
            )
        }
        E::Network(detail) => sentence(
            "The update could not be downloaded",
            detail.trim_matches('`'),
        ),
        E::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied => format!(
            "Talaria cannot replace itself where it is installed (permission denied): install the new version from {RELEASES}."
        ),
        E::TempDirNotOnSameMountPoint => format!(
            "The AppImage cannot be replaced where it is (its temporary folder is on another drive): download the new AppImage from {RELEASES}."
        ),
        E::InvalidUpdaterFormat | E::BinaryNotFoundInArchive => {
            "The download is not an installer for this copy of Talaria, so nothing was installed."
                .into()
        }
        E::AuthenticationFailed => {
            "The update was cancelled at the password prompt, so nothing was installed.".into()
        }
        other => sentence("The update could not be installed", &other.to_string()),
    }
}

/// `lead: detail.`, with the detail's own full stop if it has one.
fn sentence(lead: &str, detail: &str) -> String {
    let detail = detail.trim().trim_end_matches('.');
    if detail.is_empty() {
        format!("{lead}.")
    } else {
        format!("{lead}: {detail}.")
    }
}

/// The chip in an instance page: `offer`, or none. Evaluated by the shell
/// (`push`, `page_loaded`), so it needs nothing from the page and works on any
/// Talaria version. It lives in a shadow root on `<html>`, never takes the
/// focus, and stays small: one row (the version, Later, Update), plus a line
/// only while an install runs, after one failed, or when "How?" is opened on
/// a copy the updater cannot replace. It sits `CHIP_LIFT` above the bottom
/// right corner, clear of a composer's send button, and Later is always on
/// it. Styles come from a constructed sheet, which a page's CSP does not
/// govern; if one did not apply, the same styles are set through CSSOM
/// (`el.style`), which no CSP refuses either. Update and Later call the shell
/// (`install_update`, `snooze_update`), and the shell pushes what follows to
/// every view; a call the shell refuses says why in the chip.
pub fn chip_js(offer: Option<&Offer>) -> String {
    let data = serde_json::to_string(&offer).unwrap_or_else(|_| "null".into());
    CHIP_JS
        .replace("__OFFER__", &data)
        .replace("__LIFT__", &CHIP_LIFT.to_string())
}

/// How far above the window's bottom edge the page chip sits, in CSS px:
/// above a chat composer's row, so its send button stays reachable.
pub const CHIP_LIFT: u32 = 76;

const CHIP_JS: &str = r#"(() => {
  const offer = __OFFER__;
  const ID = 'talaria-desktop-update';
  let host = document.getElementById(ID);
  if (!offer) { if (host) host.remove(); return; }
  const root = document.documentElement;
  if (!root) return;
  if (!host) {
    host = document.createElement('div');
    host.id = ID;
    host.attachShadow({ mode: 'open' });
    root.appendChild(host);
  }
  host.style.cssText = 'all:initial;position:fixed;right:16px;bottom:__LIFT__px;z-index:2147483647;display:block;';
  const shadow = host.shadowRoot;
  // Each rule is also what CSSOM sets when the sheet did not apply.
  const RULES = {
    chip: 'box-sizing:border-box;max-width:min(340px,calc(100vw - 32px));padding:6px 6px 6px 10px;border:1px solid #302d29;border-radius:6px;background:#141312;color:#e7e2db;font:12px/1.35 "IBM Plex Sans",ui-sans-serif,system-ui,"Segoe UI",Helvetica,Arial,sans-serif;box-shadow:0 4px 16px rgba(0,0,0,0.45);text-align:left;',
    row: 'display:flex;align-items:center;gap:6px;',
    title: 'flex:1 1 auto;min-width:0;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;',
    line: 'margin:4px 4px 2px 0;color:#8e877e;user-select:text;-webkit-user-select:text;overflow-wrap:anywhere;',
    error: 'color:#ee6018;',
    button: 'flex:0 0 auto;font:600 11px/1.4 inherit;font-family:inherit;padding:2px 8px;border-radius:4px;border:1px solid #4a4640;background:transparent;color:#e7e2db;cursor:pointer;',
    update: 'background:#c8b46c;border-color:#c8b46c;color:#090a09;',
    how: 'border-color:transparent;color:#8e877e;',
  };
  const CSS = `.chip{${RULES.chip}} .row{${RULES.row}} .title{${RULES.title}} .line{${RULES.line}}
    .error{${RULES.error}} button{${RULES.button}} button.update{${RULES.update}} button.how{${RULES.how}}
    button:hover:not(:disabled){filter:brightness(1.15);} button:disabled{opacity:0.5;cursor:default;}
    button:focus-visible{outline:2px solid #c8b46c;outline-offset:2px;}`;
  let style = null;
  try {
    if (!shadow.adoptedStyleSheets.length) {
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(CSS);
      shadow.adoptedStyleSheets = [sheet];
    }
  } catch (e) {
    style = document.createElement('style');
    style.textContent = CSS;
  }
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  const invoke = (cmd) => window.__TAURI_INTERNALS__
    ? window.__TAURI_INTERNALS__.invoke(cmd, {})
    : Promise.reject(new Error('This page cannot reach Talaria Desktop.'));
  const said = (e) => String(e && e.message || e).replace(/^Error:\s*/, '');
  let open = !!host.dataset.how;
  const render = (o) => {
    const box = el('div', 'chip');
    box.setAttribute('role', 'status');
    box.setAttribute('aria-live', 'polite');
    box.dataset.version = o.version;
    const row = el('div', 'row');
    const title = el('div', 'title', 'Talaria Desktop ' + o.version + ' is available');
    title.title = title.textContent;
    row.append(title);
    const button = (cls, text, onclick) => {
      const b = el('button', cls, text);
      b.type = 'button';
      b.disabled = !!o.busy;
      b.addEventListener('click', (e) => { e.stopPropagation(); onclick(); });
      row.append(b);
      return b;
    };
    if (o.install !== 'updater') {
      const how = button('how', open ? 'Hide' : 'How?', () => {
        open = !open;
        if (open) host.dataset.how = '1'; else delete host.dataset.how;
        render(o);
      });
      how.disabled = false;
      how.setAttribute('aria-expanded', String(open));
    }
    button('later', 'Later', () => {
      host.remove();
      invoke('snooze_update').catch(() => {});
    });
    if (o.install === 'updater') {
      const update = button('update', o.busy ? 'Updating' : 'Update', () => {
        render(Object.assign({}, o, { busy: true, error: null }));
        invoke('install_update').catch((err) =>
          render(Object.assign({}, o, { busy: false, error: said(err) })));
      });
      update.title = 'Downloads it and restarts Talaria';
    }
    box.append(row);
    const line = o.error
      || (o.busy ? 'Updating. Talaria restarts when it is done.' : null)
      || (open ? o.how : null);
    if (line) box.append(el('div', o.error ? 'line error' : 'line', line));
    if (style) shadow.replaceChildren(style, box);
    else shadow.replaceChildren(box);
    // The sheet did not take (a page that refuses it, an engine without
    // constructed sheets): the same rules, through CSSOM.
    if (getComputedStyle(box).borderTopLeftRadius !== '6px') {
      const set = (e, ...names) => { e.style.cssText = names.map((n) => RULES[n]).join(''); };
      set(box, 'chip');
      set(row, 'row');
      set(title, 'title');
      box.querySelectorAll('button').forEach((b) =>
        set(b, 'button', ...(b.classList.contains('update') ? ['update'] : b.classList.contains('how') ? ['how'] : [])));
      box.querySelectorAll('.line').forEach((l) => set(l, 'line', ...(l.classList.contains('error') ? ['error'] : [])));
      host.dataset.styled = 'cssom';
    } else {
      host.dataset.styled = style ? 'style' : 'sheet';
    }
  };
  render(offer);
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    #[test]
    fn nothing_put_off_is_offered() {
        assert!(!snoozed("0.13.0", None, NOW));
    }

    #[test]
    fn later_puts_that_version_off_for_a_day() {
        let snooze = Snooze::new("0.13.0", NOW);
        assert_eq!(snooze.until_ms, NOW + SNOOZE_MS);
        assert!(snoozed("0.13.0", Some(&snooze), NOW));
        assert!(snoozed("0.13.0", Some(&snooze), NOW + SNOOZE_MS - 1));
        // The day is over: offered again.
        assert!(!snoozed("0.13.0", Some(&snooze), NOW + SNOOZE_MS));
        assert!(!snoozed("0.13.0", Some(&snooze), NOW + 2 * SNOOZE_MS));
    }

    #[test]
    fn a_newer_version_than_the_one_put_off_is_offered_at_once() {
        let snooze = Snooze::new("0.13.0", NOW);
        assert!(!snoozed("0.13.1", Some(&snooze), NOW + 1));
        assert!(!snoozed("0.14.0", Some(&snooze), NOW + 1));
        assert!(!snoozed("1.0.0", Some(&snooze), NOW + 1));
        // A pre-release of the next one is newer too; an older one is not.
        assert!(!snoozed("0.13.1-rc.1", Some(&snooze), NOW + 1));
        assert!(snoozed("0.12.9", Some(&snooze), NOW + 1));
        assert!(snoozed("0.13.0-rc.2", Some(&snooze), NOW + 1));
        // The feed's `v` prefix and stray spaces change nothing.
        assert!(snoozed("v0.13.0", Some(&snooze), NOW + 1));
        assert!(snoozed(" 0.13.0 ", Some(&snooze), NOW + 1));
    }

    #[test]
    fn a_version_that_does_not_parse_is_offered_unless_it_is_the_one_put_off() {
        let snooze = Snooze::new("nightly", NOW);
        assert!(snoozed("nightly", Some(&snooze), NOW + 1));
        assert!(!snoozed("nightly-2", Some(&snooze), NOW + 1));
        let snooze = Snooze::new("0.13.0", NOW);
        assert!(!snoozed("garbage", Some(&snooze), NOW + 1));
    }

    #[test]
    fn no_snooze_outlives_a_day() {
        // The clock went back a week, or the file was edited: offered.
        let snooze = Snooze {
            version: "0.13.0".into(),
            until_ms: NOW + 7 * SNOOZE_MS,
        };
        assert!(!snoozed("0.13.0", Some(&snooze), NOW));
        let edge = Snooze {
            version: "0.13.0".into(),
            until_ms: NOW + SNOOZE_MS,
        };
        assert!(snoozed("0.13.0", Some(&edge), NOW));
        // A file whose snooze is in the far future, or zero, never panics.
        let max = Snooze {
            version: "0.13.0".into(),
            until_ms: u64::MAX,
        };
        assert!(!snoozed("0.13.0", Some(&max), NOW));
        assert_eq!(Snooze::new("0.13.0", u64::MAX).until_ms, u64::MAX);
        let zero = Snooze {
            version: "0.13.0".into(),
            until_ms: 0,
        };
        assert!(!snoozed("0.13.0", Some(&zero), NOW));
    }

    #[test]
    fn the_offer_follows_the_snooze_and_the_install() {
        let snooze = Snooze::new("0.13.0", NOW);
        // Nothing found: no chip.
        assert_eq!(
            offer(None, None, NOW, Installed::Updater, false, None),
            None
        );
        let shown = offer(Some("0.13.0"), None, NOW, Installed::Updater, false, None).unwrap();
        assert_eq!(shown.version, "0.13.0");
        assert_eq!(shown.install, "updater");
        assert_eq!(shown.how, None);
        // Put off: no chip, until a newer one or the day is over.
        assert_eq!(
            offer(
                Some("0.13.0"),
                Some(&snooze),
                NOW + 1,
                Installed::Updater,
                false,
                None
            ),
            None
        );
        assert!(
            offer(
                Some("0.13.1"),
                Some(&snooze),
                NOW + 1,
                Installed::Updater,
                false,
                None
            )
            .is_some()
        );
        assert!(
            offer(
                Some("0.13.0"),
                Some(&snooze),
                NOW + SNOOZE_MS,
                Installed::Updater,
                false,
                None
            )
            .is_some()
        );
        // An install that is running, or that failed, stays on screen.
        let busy = offer(
            Some("0.13.0"),
            Some(&snooze),
            NOW + 1,
            Installed::Updater,
            true,
            None,
        )
        .unwrap();
        assert!(busy.busy);
        let failed = offer(
            Some("0.13.0"),
            Some(&snooze),
            NOW + 1,
            Installed::Updater,
            false,
            Some("The download did not match Talaria's signature, so nothing was installed."),
        )
        .unwrap();
        assert!(failed.error.unwrap().ends_with("installed."));
    }

    #[test]
    fn a_copy_the_updater_cannot_replace_is_told_how_instead() {
        let elsewhere = offer(
            Some("0.13.0"),
            None,
            NOW,
            Installed::Elsewhere(MSI),
            false,
            None,
        )
        .unwrap();
        assert_eq!(elsewhere.install, "elsewhere");
        assert!(elsewhere.how.unwrap().contains(".msi"));
        let json = serde_json::to_value(
            offer(Some("0.13.0"), None, NOW, Installed::Updater, false, None).unwrap(),
        )
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "version": "0.13.0", "install": "updater", "busy": false })
        );
    }

    #[test]
    fn only_a_copy_the_updater_can_replace_offers_update() {
        let exe = |p: &str| Some(std::path::PathBuf::from(p));
        let c = |os, appimage, flatpak, bundle, e: Option<std::path::PathBuf>| {
            classify(os, appimage, flatpak, bundle, e.as_deref())
        };
        // Linux: the AppImage only.
        assert_eq!(
            c(
                "linux",
                true,
                false,
                Some(BundleType::AppImage),
                exe("/tmp/.mount_x/usr/bin/t")
            ),
            Installed::Updater
        );
        for bundle in [
            Some(BundleType::Deb),
            Some(BundleType::Rpm),
            Some(BundleType::AppImage),
            None,
        ] {
            assert_eq!(
                c(
                    "linux",
                    false,
                    false,
                    bundle.clone(),
                    exe("/usr/bin/talaria-desktop")
                ),
                Installed::Elsewhere(LINUX_PACKAGE),
                "{bundle:?}"
            );
        }
        assert_eq!(
            c("linux", false, true, None, exe("/app/bin/talaria-desktop")),
            Installed::Elsewhere(FLATPAK)
        );
        // Windows: the NSIS install (and an unbundled exe); not the .msi.
        assert_eq!(
            c("windows", false, false, Some(BundleType::Nsis), None),
            Installed::Updater
        );
        assert_eq!(c("windows", false, false, None, None), Installed::Updater);
        assert_eq!(
            c("windows", false, false, Some(BundleType::Msi), None),
            Installed::Elsewhere(MSI)
        );
        // macOS: the app, unless it runs from a read-only place.
        assert_eq!(
            c(
                "macos",
                false,
                false,
                Some(BundleType::App),
                exe("/Applications/Talaria.app/Contents/MacOS/talaria-desktop")
            ),
            Installed::Updater
        );
        for ro in [
            "/Volumes/Talaria/Talaria.app/Contents/MacOS/talaria-desktop",
            "/private/var/folders/x/T/AppTranslocation/1234/d/Talaria.app/Contents/MacOS/talaria-desktop",
        ] {
            assert_eq!(
                c("macos", false, false, Some(BundleType::App), exe(ro)),
                Installed::Elsewhere(MAC_DISK_IMAGE),
                "{ro}"
            );
        }
        // Every "elsewhere" sentence names where to get the new one.
        for how in [LINUX_PACKAGE, FLATPAK, MSI] {
            assert!(how.contains(RELEASES), "{how}");
            assert!(how.ends_with('.'), "{how}");
        }
    }

    #[test]
    fn failures_read_as_sentences() {
        use tauri_plugin_updater::Error as E;
        let said = install_failure(E::InvalidUpdaterFormat);
        assert!(
            said.starts_with("The download is not an installer"),
            "{said}"
        );
        let said = install_failure(E::Network(
            "Download request failed with status: 404 Not Found".into(),
        ));
        assert_eq!(
            said,
            "The update could not be downloaded: Download request failed with status: 404 Not Found."
        );
        let said = install_failure(E::Io(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        assert!(
            said.contains("permission denied") && said.ends_with('.'),
            "{said}"
        );
        let said = install_failure(E::TempDirNotOnSameMountPoint);
        assert!(said.contains("AppImage") && said.ends_with('.'), "{said}");
        let said = check_failure(E::ReleaseNotFound);
        assert_eq!(
            said,
            "Could not check for updates: the update server did not answer with a release."
        );
        let said = check_failure(E::TargetsNotFound(vec!["linux-x86_64".into()]));
        assert!(
            said.starts_with("Could not check for updates: ") && said.ends_with('.'),
            "{said}"
        );
        assert_eq!(sentence("Lead", "detail."), "Lead: detail.");
        assert_eq!(sentence("Lead", "  "), "Lead.");
    }

    #[test]
    fn a_download_that_stops_or_runs_too_long_is_given_up_with_a_sentence() {
        let stall = DOWNLOAD_STALL.as_millis() as u64;
        // Bytes arriving: never cut off before the cap, however slow.
        assert_eq!(download_stalled(0, Duration::from_secs(5)), None);
        assert_eq!(
            download_stalled(stall - 1, DOWNLOAD_CAP - Duration::from_secs(1)),
            None
        );
        // Nothing for a minute: stopped.
        let said = download_stalled(stall, Duration::from_secs(61)).unwrap();
        assert!(
            said.starts_with("The download stopped: nothing arrived for 60 seconds"),
            "{said}"
        );
        assert!(said.ends_with("Try Update again."), "{said}");
        // Still going after the cap: stopped too.
        let said = download_stalled(0, DOWNLOAD_CAP).unwrap();
        assert!(said.contains("more than 30 minutes"), "{said}");
        assert!(DOWNLOAD_STALL < DOWNLOAD_CAP);
    }

    #[test]
    fn the_page_chip_carries_the_offer_and_nothing_else() {
        let gone = chip_js(None);
        assert!(gone.contains("const offer = null;"));
        let o = offer(Some("0.13.0"), None, NOW, Installed::Updater, false, None).unwrap();
        let js = chip_js(Some(&o));
        assert!(
            js.contains(r#"const offer = {"version":"0.13.0","install":"updater","busy":false};"#)
        );
        assert!(!js.contains("__OFFER__"));
        // Text goes in as text, never as markup, and the styles never need an
        // inline <style> a page CSP could refuse (only the fallback makes one).
        assert!(!js.contains("innerHTML"));
        assert!(js.contains("adoptedStyleSheets"));
        // And when no sheet applies, CSSOM sets the same rules.
        assert!(js.contains("e.style.cssText = names.map"));
        assert!(js.contains(&format!("bottom:{CHIP_LIFT}px")));
        assert!(!js.contains("__LIFT__"));
        assert!(js.contains("invoke('snooze_update')"));
        assert!(js.contains("invoke('install_update')"));
        // A version that is not a plain one still cannot break out of the
        // literal: it is JSON.
        let odd = Offer {
            version: "1.0.0\"});alert(1);({\"a\":\"".into(),
            ..o
        };
        let js = chip_js(Some(&odd));
        assert!(js.contains(r#""version":"1.0.0\"});alert(1);({\"a\":\"""#));
    }
}
