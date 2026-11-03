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
/// The download's bounds, on the monotonic clock. The payload is the whole
/// installer — the Linux AppImage is about 83 MB, 43 minutes at 256 kbit/s —
/// so the bound is progress, not time: a download that has had nothing
/// arrive for `DOWNLOAD_STALL` has stopped. `DOWNLOAD_BACKSTOP` only ends a
/// trickle (a byte now and then, forever), and points to the releases page.
/// Installing the bytes once they are here is local and never cut off.
pub const DOWNLOAD_STALL: Duration = Duration::from_secs(60);
pub const DOWNLOAD_BACKSTOP: Duration = Duration::from_secs(4 * 60 * 60);
/// How often the download's bounds and progress are looked at.
const DOWNLOAD_LOOK: Duration = Duration::from_secs(1);

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
    /// How much of the download has arrived, in percent, while one runs and
    /// the server said how big it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
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
    progress: Option<u8>,
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
        progress: progress.filter(|_| busy),
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
    /// The running download's progress, in percent.
    progress: Option<u8>,
    error: Option<String>,
    /// What the views were last given, so a tick that changes nothing pushes
    /// nothing.
    pushed: Option<Offer>,
}

/// What a panic inside `install` says in the chip.
const INSTALL_PANICKED: &str =
    "The update stopped unexpectedly, so nothing was installed. Try Update again.";

impl Inner {
    /// `install` is over, however it ended. True when the views need telling:
    /// it was still marked running (a panic; every return marks it itself).
    fn end_install(&mut self, panicked: bool) -> bool {
        if !self.installing {
            return false;
        }
        self.installing = false;
        self.progress = None;
        if panicked {
            self.error = Some(INSTALL_PANICKED.into());
        }
        true
    }
}

impl Updates {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Held for the whole of `install`: dropped on any return or in a panic's
/// unwind, it ends the install, so the chip never stays on "Updating".
struct InstallRun<'a>(&'a AppHandle);

impl Drop for InstallRun<'_> {
    fn drop(&mut self) {
        let Some(updates) = self.0.try_state::<Updates>() else {
            return;
        };
        let tell = updates.lock().end_install(std::thread::panicking());
        if tell {
            push_soon(self.0);
        }
    }
}

/// The offer as it stands now. One lock at a time (lib.rs's discipline).
pub fn current(app: &AppHandle) -> Option<Offer> {
    let updates = app.try_state::<Updates>()?;
    let (available, busy, progress, error) = {
        let inner = updates.lock();
        (
            inner.available.clone(),
            inner.installing,
            inner.progress,
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
        progress,
        error.as_deref(),
    )
}

/// Give every view the offer as it stands, when it changed: the launcher by
/// the snapshot, each instance page by `chip_js`. Main thread, like every
/// other `eval`.
pub fn push(app: &AppHandle) {
    let offer = current(app);
    let said = {
        let Some(updates) = app.try_state::<Updates>() else {
            return;
        };
        let mut inner = updates.lock();
        if inner.pushed == offer {
            return;
        }
        let said = worth_a_line(&inner.pushed, &offer);
        inner.pushed = offer.clone();
        said
    };
    if said {
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
    }
    commands::emit_state(app);
    let js = chip_js(offer.as_ref(), titlebar(app));
    for webview in commands::instance_pages(app) {
        let _ = webview.eval(&js);
    }
}

/// The log says when the chip's state changes (a version, how it installs,
/// installing or not, failed or not), not each percent of a download.
fn worth_a_line(before: &Option<Offer>, now: &Option<Offer>) -> bool {
    let kind = |o: &Option<Offer>| {
        o.as_ref()
            .map(|o| (o.version.clone(), o.install, o.busy, o.error.is_some()))
    };
    kind(before) != kind(now)
}

/// The window's titlebar mode: where the chip goes in a page.
fn titlebar(app: &AppHandle) -> settings::TitlebarMode {
    app.try_state::<ShellState>()
        .map(|s| s.settings.lock().unwrap().titlebar)
        .unwrap_or_default()
}

/// The titlebar mode changed: every page's chip again, in its new place,
/// whether or not the offer changed. Queued, like `push_soon`.
pub fn repaint(app: &AppHandle) {
    commands::queue_main_turn(app, |app| {
        let js = chip_js(current(app).as_ref(), titlebar(app));
        for webview in commands::instance_pages(app) {
            let _ = webview.eval(&js);
        }
    });
}

/// `push`, from wherever: a tokio worker or a command. Queued, so it never
/// runs inside a WebView2 handler.
fn push_soon(app: &AppHandle) {
    commands::queue_main_turn(app, push);
}

/// A page finished loading: it has no chip yet, whatever the others show.
pub fn page_loaded(app: &AppHandle, webview: &tauri::Webview) {
    if let Some(offer) = current(app) {
        let _ = webview.eval(chip_js(Some(&offer), titlebar(app)));
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

/// Update: look at the feed (bounded at `INSTALL_CHECK_TIMEOUT`), download
/// the signed payload (bounded by its progress, `download_watched`), apply
/// it, relaunch. The plugin verifies the minisign signature against the
/// pubkey in tauri.conf.json inside `download`, before any bytes reach
/// `install`. On Windows the NSIS installer takes over and the plugin exits
/// the app; it relaunches it when it is done (`installMode: passive`).
/// However it ends, even in a panic, the chip stops saying "Updating"
/// (`InstallRun`).
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
        inner.progress = None;
        inner.error = None;
    }
    let _run = InstallRun(app);
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
        let bytes = download_watched(app, &update).await?;
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
        inner.progress = None;
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

/// The update's download, bounded by `download_stalled` on the monotonic
/// clock: a download that is dropped stops, and the chip says so and offers
/// Update again. Its progress goes to every view as it arrives, a percent at
/// a time.
async fn download_watched(
    app: &AppHandle,
    update: &tauri_plugin_updater::Update,
) -> Result<Vec<u8>, String> {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    };
    let started = std::time::Instant::now();
    // Milliseconds after `started` when the last bytes arrived, and how many.
    let last = Arc::new(AtomicU64::new(0));
    let received = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let (at, got, of) = (last.clone(), received.clone(), total.clone());
    let download = update.download(
        move |chunk, length| {
            at.store(started.elapsed().as_millis() as u64, Relaxed);
            got.fetch_add(chunk as u64, Relaxed);
            if let Some(length) = length {
                of.store(length, Relaxed);
            }
        },
        || {},
    );
    let mut download = std::pin::pin!(download);
    loop {
        match tokio::time::timeout(DOWNLOAD_LOOK, &mut download).await {
            Ok(done) => return done.map_err(install_failure),
            Err(_) => {
                let now = started.elapsed();
                let idle = (now.as_millis() as u64).saturating_sub(last.load(Relaxed));
                if let Some(why) = download_stalled(idle, now) {
                    eprintln!("talaria-desktop: update download: {why}");
                    return Err(why);
                }
                let progress = percent(received.load(Relaxed), total.load(Relaxed));
                let changed = {
                    let updates = app.state::<Updates>();
                    let mut inner = updates.lock();
                    let changed = inner.progress != progress;
                    inner.progress = progress;
                    changed
                };
                if changed {
                    push_soon(app);
                }
            }
        }
    }
}

/// `received` of `total` bytes, in whole percent; None when the size is not
/// known. Never 100 before the download has returned.
pub fn percent(received: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    Some(((received.min(total) * 100) / total).min(99) as u8)
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
    if elapsed >= DOWNLOAD_BACKSTOP {
        return Some(format!(
            "The download has run for more than {} hours, so it was stopped and nothing was installed: download the new version from {RELEASES}.",
            DOWNLOAD_BACKSTOP.as_secs() / 3600
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
/// (`push`, `page_loaded`, `repaint`), so it needs nothing from the page and
/// works on any Talaria version. It lives in a shadow root on `<html>` and
/// never takes the focus.
///
/// Where depends on the window's titlebar mode and on the page, which the
/// chip measures, since instances run different Talaria versions:
/// - **Themed** (the default on every OS), on a page whose themed band's
///   middle takes no click — an older Talaria's 36 px band, a sign-in page:
///   one 28 px row centred in it, `CHIP_TOP` from the top. The band has the
///   window controls at one end and empty drag space everywhere else, so
///   the row covers nothing a person types into or reads. A second line (an
///   install's error, the "How?" sentence) drops below the band, over the
///   page's nav, only once the person asked for it.
/// - **Themed, on a UI whose tab bar is the titlebar** (its tabs are in that
///   middle), **and the OS titlebar or none:** the chip starts as the small
///   pill, just below the bar across the top of the page (`CHIP_TOP_BARE`
///   when there is none to measure), over the middle of the page's header
///   line, and stays folded unless the person opens it.
///
/// The page is measured again a moment after the chip arrives (the UI may
/// mount after it), on resize, and every 2 s, but never while the pointer is
/// on the chip or a press is in progress (it is measured again once the
/// pointer leaves), and not at all once the chip is gone: its interval and
/// resize listener stop with it.
///
/// The chip folds to the pill ("–") at any time, an install included: the
/// pill keeps its progress, and folding cancels nothing. An install that
/// failed folds it to "↑ Update failed" until the person opens it for the
/// sentence, so no error line sits over the page by itself. A window too narrow for the row (a small window, a page zoomed in)
/// starts folded too. The page's choice is kept for the tab's session. A
/// progress push changes the text in place, so a click or the focus is never
/// lost to a re-render.
///
/// Styles come from a constructed sheet, which a page's CSP does not govern;
/// if one did not apply, the same rules are set through CSSOM (`el.style`),
/// which no CSP refuses either. Update and Later call the shell
/// (`install_update`, `snooze_update`), and the shell pushes what follows to
/// every view; a call the shell refuses says why in the chip.
pub fn chip_js(offer: Option<&Offer>, titlebar: settings::TitlebarMode) -> String {
    let data = serde_json::to_string(&offer).unwrap_or_else(|_| "null".into());
    let bar = titlebar == settings::TitlebarMode::Themed;
    CHIP_JS
        .replace("__OFFER__", &data)
        .replace("__BAR__", if bar { "true" } else { "false" })
        .replace("__TOP__", &CHIP_TOP.to_string())
        .replace("__BARE_TOP__", &CHIP_TOP_BARE.to_string())
        .replace("__NARROW__", &CHIP_NARROW.to_string())
}

/// How far from the top of the page the chip's row sits, in CSS px, in the
/// themed mode: inside the 36 px titlebar, 4 px clear of its edges.
pub const CHIP_TOP: u32 = 4;

/// Below the bar across the top of the page, when the chip cannot measure
/// it: a nav row's 56 px, and 6 px clear.
pub const CHIP_TOP_BARE: u32 = 62;

/// Below this window width, in CSS px, the chip starts folded: the row and
/// its buttons need about 300 px between the window controls.
pub const CHIP_NARROW: u32 = 560;

const CHIP_JS: &str = r#"(() => {
  const offer = __OFFER__;
  const BAR = __BAR__;
  const BAND = __TOP__;
  const BELOW = __BARE_TOP__;
  const ID = 'talaria-desktop-update';
  const KEY = 'talaria-desktop-update-fold';
  let host = document.getElementById(ID);
  // The page is measured only while a chip is up: when it goes (Later, a
  // snooze, no offer), its interval and its resize listener go with it.
  const stopFit = () => {
    const f = window.__talariaChipFits;
    if (f) { clearInterval(f.timer); removeEventListener('resize', f.fit); window.__talariaChipFits = null; }
  };
  if (!offer) { stopFit(); if (host) host.remove(); return; }
  const root = document.documentElement;
  if (!root) return;
  if (!host) {
    host = document.createElement('div');
    host.id = ID;
    host.attachShadow({ mode: 'open' });
    root.appendChild(host);
  }
  // Where: in the themed band's middle when nothing there takes a click (an
  // older Talaria's 36 px band, a sign-in page); otherwise just below the bar
  // across the top of the page (a UI whose tab bar is the titlebar, the OS
  // titlebar's nav row), folded.
  const CONTROLS = 'a, button, input, select, textarea, [role="tab"], [role="button"], [contenteditable="true"]';
  // Nothing that takes a click in the box `half` either side of `x`, from
  // `y` down `h` px (the chip's own host aside).
  const clear = (x, half, y, h) => {
    for (const dy of [2, h / 2, h - 2]) {
      for (const dx of [-half, -half / 2, 0, half / 2, half]) {
        for (const e of document.elementsFromPoint(x + dx, y + dy)) {
          if (e !== host && e.closest(CONTROLS)) return false;
        }
      }
    }
    return true;
  };
  const bandFree = () => clear(innerWidth / 2, Math.max(0, Math.min(180, innerWidth / 2 - 130)), BAND, 28);
  // The bottom of the bar across the top of the page.
  const barBottom = () => {
    for (const e of document.elementsFromPoint(innerWidth / 2, 18)) {
      if (e === host || e === document.documentElement || e === document.body) continue;
      const r = e.getBoundingClientRect();
      if (r.top <= 1 && r.width >= innerWidth * 0.9 && r.height >= 24 && r.height <= 120) return Math.round(r.bottom);
    }
    return BELOW - 6;
  };
  const place = () => (BAR && bandFree() ? BAND : barBottom() + 6);
  // The folded pill below the bar goes where nothing under it takes a click:
  // just below the bar, in the middle, else a quarter or an eighth in from
  // either side; else across the bar's lower edge, where a bar's tabs and a
  // header's controls leave a strip (a page zoomed in on a small window,
  // whose sidebar search and header buttons fill the row below).
  const PILL_H = 22;
  const spot = () => {
    const below = top, edge = top - 6 - 10;
    for (const y of [below, edge]) {
      for (const f of [0.5, 0.75, 0.25, 0.875, 0.125]) {
        const x = Math.max(62, Math.min(innerWidth - 62, innerWidth * f));
        if (clear(x, 60, y, PILL_H)) return [x, y];
      }
    }
    return [innerWidth / 2, below];
  };
  const setX = () => {
    const [x, y] = top !== BAND && collapsed ? spot() : [innerWidth / 2, top];
    host.style.left = x + 'px';
    host.style.top = y + 'px';
  };
  let top = place();
  host.style.cssText = 'all:initial;position:fixed;top:' + top + 'px;left:50%;transform:translateX(-50%);z-index:2147483647;display:block;';
  const shadow = host.shadowRoot;
  // Each rule is also what CSSOM sets when the sheet did not apply.
  const RULES = {
    chip: 'box-sizing:border-box;max-width:calc(100vw - 260px);overflow:hidden;border:1px solid #302d29;border-radius:6px;background:#141312;color:#e7e2db;font-family:"IBM Plex Sans",ui-sans-serif,system-ui,"Segoe UI",Helvetica,Arial,sans-serif;font-size:12px;line-height:1.35;box-shadow:0 4px 16px rgba(0,0,0,0.45);text-align:left;',
    row: 'box-sizing:border-box;display:flex;align-items:center;gap:6px;height:26px;padding:0 3px 0 10px;',
    title: 'flex:1 1 auto;min-width:0;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;',
    line: 'max-width:420px;padding:0 10px 6px 10px;color:#8e877e;user-select:text;-webkit-user-select:text;overflow-wrap:anywhere;',
    error: 'color:#ee6018;',
    button: 'flex:0 0 auto;font-family:inherit;font-weight:600;font-size:11px;line-height:1.4;padding:1px 8px;border-radius:4px;border:1px solid #4a4640;background:transparent;color:#e7e2db;cursor:pointer;',
    update: 'background:#c8b46c;border-color:#c8b46c;color:#090a09;',
    quiet: 'border-color:transparent;color:#8e877e;',
    pill: 'display:block;max-width:100%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-family:inherit;font-weight:600;font-size:11px;line-height:1.4;padding:2px 10px;border:0;border-radius:6px;background:transparent;color:#c8b46c;cursor:pointer;',
    failed: 'color:#ee6018;',
  };
  const CSS = `.chip{${RULES.chip}} .row{${RULES.row}} .title{${RULES.title}} .line{${RULES.line}}
    .error{${RULES.error}} button{${RULES.button}} button.update{${RULES.update}} button.quiet{${RULES.quiet}}
    button.pill{${RULES.pill}} button.pill.failed{${RULES.failed}}
    button:hover{filter:brightness(1.15);}
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
  // The person's choice for this version: folded ('small:<v>') or opened
  // ('open:<v>'), on the host and for the tab's session.
  const stored = () => { try { return sessionStorage.getItem(KEY); } catch (e) { return null; } };
  const choose = (what) => {
    const v = what + ':' + offer.version;
    host.dataset.fold = v;
    try { sessionStorage.setItem(KEY, v); } catch (e) {}
  };
  const chose = (what) => (host.dataset.fold || stored()) === what + ':' + offer.version;
  // A failed install the shell pushed, that the person has not opened yet:
  // the pill says so, and its sentence waits for them (it would sit over the
  // page's nav).
  const unseen = () => !!offer.error && host.dataset.seen !== offer.error;
  const folded = () => chose('small') || unseen() || (!chose('open') && (top !== BAND || innerWidth < __NARROW__));
  let collapsed = folded();
  setX();
  let how = host.dataset.how === '1';
  const pillText = (o, pct) => o.error ? '↑ Update failed' : o.busy ? '↑ Updating' + pct : '↑ ' + o.version;
  const titleText = (o, pct) => o.busy
    ? 'Updating Talaria Desktop ' + o.version + (pct ? ' ·' + pct : '')
    : 'Talaria Desktop ' + o.version + ' is available';
  const render = (o) => {
    const pct = o.busy && o.progress != null ? ' ' + o.progress + '%' : '';
    // Only the progress changed: the text, in place.
    const sig = JSON.stringify([o.version, o.install, !!o.busy, o.error || null, collapsed, how, BAR]);
    const live = shadow.querySelector('.chip');
    if (live && host.dataset.sig === sig) {
      const t = live.querySelector(collapsed ? '.pill' : '.title');
      if (t) t.textContent = collapsed ? pillText(o, pct) : titleText(o, pct);
      return;
    }
    host.dataset.sig = sig;
    const box = el('div', 'chip');
    box.setAttribute('role', 'status');
    box.setAttribute('aria-live', 'polite');
    box.dataset.version = o.version;
    box.dataset.state = collapsed ? 'collapsed' : o.busy ? 'busy' : o.error ? 'error' : 'offer';
    if (collapsed) {
      const pill = el('button', o.error ? 'pill failed' : 'pill', pillText(o, pct));
      pill.type = 'button';
      pill.title = o.error
        ? o.error + ' Show'
        : 'Talaria Desktop ' + o.version + (o.busy ? ': updating' : ' is available') + '. Show';
      pill.addEventListener('click', (e) => {
        e.stopPropagation();
        if (o.error) host.dataset.seen = o.error;
        collapsed = false; choose('open'); setX();
        render(o);
      });
      box.append(pill);
    } else {
      const row = el('div', 'row');
      const title = el('div', 'title', titleText(o, pct));
      title.title = o.busy ? 'Talaria restarts when it is done.' : title.textContent;
      row.append(title);
      const button = (cls, text, onclick) => {
        const b = el('button', cls, text);
        b.type = 'button';
        b.addEventListener('click', (e) => { e.stopPropagation(); onclick(); });
        row.append(b);
        return b;
      };
      if (!o.busy) {
        if (o.install !== 'updater') {
          const h = button('quiet', how ? 'Hide' : 'How?', () => {
            how = !how;
            if (how) host.dataset.how = '1'; else delete host.dataset.how;
            render(o);
          });
          h.setAttribute('aria-expanded', String(how));
        }
        button('later', 'Later', () => {
          host.remove();
          stopFit();
          invoke('snooze_update').catch(() => {});
        });
        if (o.install === 'updater') {
          const u = button('update', 'Update', () => {
            render(Object.assign({}, o, { busy: true, progress: null, error: null }));
            invoke('install_update').catch((err) =>
              render(Object.assign({}, o, { busy: false, error: said(err) })));
          });
          u.title = 'Downloads it and restarts Talaria';
        }
      }
      const hide = button('quiet hide', '–', () => {
        collapsed = true; choose('small'); setX();
        render(o);
      });
      hide.title = 'Make this small';
      hide.setAttribute('aria-label', 'Make this small');
      box.append(row);
      const line = o.error || (how && !o.busy ? o.how : null);
      if (line) box.append(el('div', o.error ? 'line error' : 'line', line));
    }
    if (style) shadow.replaceChildren(style, box);
    else shadow.replaceChildren(box);
    // The sheet did not take (a page that refuses it, an engine without
    // constructed sheets): the same rules, through CSSOM.
    if (getComputedStyle(box).borderTopLeftRadius !== '6px') {
      const set = (e, ...names) => { e.style.cssText = names.map((n) => RULES[n]).join(''); };
      set(box, 'chip');
      box.querySelectorAll('.row').forEach((r) => set(r, 'row'));
      box.querySelectorAll('.title').forEach((t) => set(t, 'title'));
      box.querySelectorAll('button').forEach((b) => {
        const c = b.classList;
        set(b, ...(c.contains('pill') ? ['pill', ...(c.contains('failed') ? ['failed'] : [])] : ['button', ...(c.contains('update') ? ['update'] : c.contains('quiet') ? ['quiet'] : [])]));
      });
      box.querySelectorAll('.line').forEach((l) => set(l, 'line', ...(l.classList.contains('error') ? ['error'] : [])));
      host.dataset.styled = 'cssom';
    } else {
      host.dataset.styled = style ? 'style' : 'sheet';
    }
  };
  // The page changes under the chip: its UI mounts after the chip arrives, a
  // window narrows or widens, a page is zoomed. Placed and folded again, by
  // the latest offer's render: on resize, a moment after it is added, and
  // every 2 s.
  host.__talariaFit = () => {
    // Never moved under the person's pointer, nor mid-press: measured again
    // once the pointer leaves.
    if (host.dataset.over || host.dataset.press) return;
    const was = collapsed;
    top = place();
    collapsed = folded();
    if (collapsed !== was) render(offer);
    setX();
  };
  const fit = () => { const h = document.getElementById(ID); if (h && h.__talariaFit) h.__talariaFit(); };
  if (!window.__talariaChipFits) {
    window.__talariaChipFits = { timer: setInterval(fit, 2000), fit };
    addEventListener('resize', fit);
  }
  if (!host.dataset.watched) {
    host.dataset.watched = '1';
    const again = () => setTimeout(fit, 300);
    host.addEventListener('pointerenter', () => { host.dataset.over = '1'; });
    host.addEventListener('pointerleave', () => { delete host.dataset.over; if (!host.dataset.press) again(); });
    host.addEventListener('pointerdown', () => { host.dataset.press = '1'; });
    addEventListener('pointerup', () => { if (host.dataset.press) { delete host.dataset.press; if (!host.dataset.over) again(); } }, true);
    addEventListener('pointercancel', () => { delete host.dataset.press; }, true);
  }
  setTimeout(fit, 300);
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
            offer(None, None, NOW, Installed::Updater, false, None, None),
            None
        );
        let shown = offer(
            Some("0.13.0"),
            None,
            NOW,
            Installed::Updater,
            false,
            None,
            None,
        )
        .unwrap();
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
                None,
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
                None,
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
                None,
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
            None,
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
            None,
        )
        .unwrap();
        assert_eq!(elsewhere.install, "elsewhere");
        assert!(elsewhere.how.unwrap().contains(".msi"));
        let json = serde_json::to_value(
            offer(
                Some("0.13.0"),
                None,
                NOW,
                Installed::Updater,
                false,
                None,
                None,
            )
            .unwrap(),
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
            download_stalled(stall - 1, DOWNLOAD_BACKSTOP - Duration::from_secs(1)),
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
        let said = download_stalled(0, DOWNLOAD_BACKSTOP).unwrap();
        assert!(said.contains("more than 4 hours"), "{said}");
        assert!(said.contains(RELEASES), "{said}");
        assert!(DOWNLOAD_STALL < DOWNLOAD_BACKSTOP);
        // The AppImage (about 83 MB) at 256 kbit/s takes about 43 minutes:
        // well inside the backstop, and never "stopped" while bytes arrive.
        let appimage_at_256k = Duration::from_secs(83_000_000 / 32_000);
        assert!(appimage_at_256k < DOWNLOAD_BACKSTOP);
        assert_eq!(download_stalled(1_000, appimage_at_256k), None);
    }

    #[test]
    fn progress_is_whole_percent_and_never_done_before_the_download_is() {
        assert_eq!(percent(0, 0), None);
        assert_eq!(percent(500, 0), None);
        assert_eq!(percent(0, 1000), Some(0));
        assert_eq!(percent(425, 1000), Some(42));
        assert_eq!(percent(1000, 1000), Some(99));
        assert_eq!(percent(2000, 1000), Some(99));
        // 83 MB against u64: no overflow.
        assert_eq!(percent(41_500_000, 83_000_000), Some(50));
        // Only an install that runs carries it.
        let o = offer(
            Some("0.13.0"),
            None,
            NOW,
            Installed::Updater,
            false,
            Some(42),
            None,
        )
        .unwrap();
        assert_eq!(o.progress, None);
        let o = offer(
            Some("0.13.0"),
            None,
            NOW,
            Installed::Updater,
            true,
            Some(42),
            None,
        )
        .unwrap();
        assert_eq!(o.progress, Some(42));
    }

    #[test]
    fn an_install_that_panics_does_not_leave_the_chip_on_updating() {
        let mut inner = Inner {
            installing: true,
            progress: Some(30),
            ..Inner::default()
        };
        assert!(inner.end_install(true));
        assert!(!inner.installing);
        assert_eq!(inner.progress, None);
        assert_eq!(inner.error.as_deref(), Some(INSTALL_PANICKED));
        // An install that ended by returning has marked itself: nothing more.
        let mut inner = Inner {
            error: Some(
                "The download did not match Talaria's signature, so nothing was installed.".into(),
            ),
            ..Inner::default()
        };
        assert!(!inner.end_install(false));
        assert!(inner.error.unwrap().contains("signature"));
    }

    #[test]
    fn the_log_says_when_the_chip_changes_not_each_percent() {
        let base = Offer {
            version: "0.13.0".into(),
            install: "updater",
            busy: true,
            progress: Some(4),
            how: None,
            error: None,
        };
        let at = |p| {
            Some(Offer {
                progress: Some(p),
                ..base.clone()
            })
        };
        // A download's progress: one line for the whole run.
        assert!(!worth_a_line(&at(4), &at(14)));
        assert!(!worth_a_line(&at(14), &at(99)));
        // What changes the chip: each a line.
        assert!(worth_a_line(&None, &at(4)));
        assert!(worth_a_line(&at(99), &None));
        let failed = Some(Offer {
            busy: false,
            error: Some("The download stopped.".into()),
            ..base.clone()
        });
        assert!(worth_a_line(&at(40), &failed));
        let newer = Some(Offer {
            version: "0.13.1".into(),
            ..base.clone()
        });
        assert!(worth_a_line(&at(40), &newer));
        let idle = Some(Offer {
            busy: false,
            progress: None,
            ..base.clone()
        });
        assert!(worth_a_line(&idle, &at(0)));
    }

    #[test]
    fn the_page_chip_carries_the_offer_and_nothing_else() {
        use settings::TitlebarMode::{Os, Themed};
        let gone = chip_js(None, Themed);
        assert!(gone.contains("const offer = null;"));
        let o = offer(
            Some("0.13.0"),
            None,
            NOW,
            Installed::Updater,
            false,
            None,
            None,
        )
        .unwrap();
        let js = chip_js(Some(&o), Themed);
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
        // In the themed band's middle if nothing there takes a click, open
        // unless the window is narrow; else below the bar, folded.
        assert!(js.contains(&format!("const BAND = {CHIP_TOP};")));
        assert!(js.contains(&format!("const BELOW = {CHIP_TOP_BARE};")));
        assert!(js.contains("const place = () => (BAR && bandFree() ? BAND : barBottom() + 6);"));
        // The pill goes where nothing under it takes a click, below the bar or
        // across its lower edge.
        assert!(js.contains("for (const y of [below, edge])"));
        // It never moves under the pointer, and the measuring stops when the
        // chip goes: no offer, or Later.
        assert!(js.contains("if (host.dataset.over || host.dataset.press) return;"));
        assert!(js.contains("if (!offer) { stopFit(); if (host) host.remove(); return; }"));
        assert!(js.contains("host.remove();\n          stopFit();"));
        assert!(js.contains("clearInterval(f.timer); removeEventListener('resize', f.fit);"));
        assert!(js.contains("const BAR = true;"));
        assert!(js.contains(&format!("innerWidth < {CHIP_NARROW}")));
        for hole in [
            "__TOP__",
            "__BARE_TOP__",
            "__BAR__",
            "__NARROW__",
            "__OFFER__",
        ] {
            assert!(!js.contains(hole), "{hole}");
        }
        // With the OS titlebar, or none, the page's nav is the first row:
        // below it, and folded to the pill until the person opens it.
        let bare = chip_js(Some(&o), Os);
        assert!(bare.contains("const BAR = false;"));
        assert!(bare.contains("(top !== BAND || innerWidth <"));
        // A failed install says so in the pill, folded until it is opened.
        assert!(js.contains("o.error ? '↑ Update failed'"));
        assert!(js.contains("chose('small') || unseen() ||"));
        // A progress push changes the text in place, so no click is lost.
        assert!(js.contains("host.dataset.sig === sig"));
        assert!(js.contains("t.textContent = collapsed ? pillText(o, pct) : titleText(o, pct)"));
        // The row never spills over the window controls.
        assert!(js.contains("max-width:calc(100vw - 260px);overflow:hidden;"));
        // Collapsing keeps the progress and is there while an install runs:
        // the hide button is outside the `!o.busy` block.
        assert!(js.contains("'↑ Updating' + pct"));
        assert!(js.contains("button('quiet hide'"));
        // The buttons' type is set longhand: `font:600 11px/1.4 inherit` is
        // not valid CSS and was dropped whole.
        assert!(!js.contains("font:600"));
        assert!(js.contains("font-size:11px"));
        // No button is ever disabled (busy hides them), so nothing styles one.
        assert!(!js.contains("disabled"));
        assert!(js.contains("invoke('snooze_update')"));
        assert!(js.contains("invoke('install_update')"));
        // A version that is not a plain one still cannot break out of the
        // literal: it is JSON.
        let odd = Offer {
            version: "1.0.0\"});alert(1);({\"a\":\"".into(),
            ..o
        };
        let js = chip_js(Some(&odd), Themed);
        assert!(js.contains(r#""version":"1.0.0\"});alert(1);({\"a\":\"""#));
    }
}
