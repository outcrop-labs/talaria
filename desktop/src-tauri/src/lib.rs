// Talaria Desktop — the multitenant shell around Talaria instances.
//
// Two views, two webview roles:
//
//   * welcome — the launcher (webview label "main", local content, the only
//     webview with full IPC: capabilities/launcher.json + the AppManifest
//     gate in build.rs) fills the window: brand, the instance list, add.
//   * active — the instance's webview IS the window; the launcher is hidden
//     but alive. Switching between instances happens INSIDE the instance UI
//     (the desktop switcher beside the logo in ui/), so each registered
//     instance origin is granted, at runtime, a minimal capability: the
//     switcher commands, window chrome (titlebar mode, min/max/close/drag),
//     the notification commands and the badge — no fs, no core:* (no event
//     channel: a remote origin that could listen could hear another
//     instance's events), nothing else.
//
// Every instance webview gets its own data directory, so cookie jars and
// localStorage never mix. A hidden webview stays loaded (sessions and SSE
// survive a switch) until it has been off screen for the suspend timeout;
// then, if its page agrees, it is closed, and showing it again recreates it at
// the exact page it was on (suspend.rs; the shell watches its notifications
// meanwhile, watch.rs). An instance nobody opens this session is loaded
// hidden at launch and handed to a watcher the same way, so every signed-in
// instance alerts (D8).
//
// Closing the window keeps the shell running where the desktop can show it
// (background.rs, tray.rs), so notifications still arrive; Quit is always
// one step away. On Linux one process serves every launch: a second one
// hands the window back to the first and exits (notify/linux.rs). The GTK
// application id is `app.talaria.desktop` (tauri.conf.json `enableGTKAppId`),
// so the dock matches the window to its desktop file by Wayland app_id, and
// the desktop's own Quit (GNOME's dash, Alt+Tab) finds an `app.quit` action
// that really quits (`install_quit_action`).
//
// The shell owns all geometry: WebKitGTK mis-places new child webviews until
// their first set_bounds (tauri#10420) and auto_resize breaks across resize
// cycles (tauri#10131), so layout.rs applies logical-unit bounds at the end
// of setup and again on every resize, and never uses auto_resize. The setup
// pass is load-bearing: the initial Resized can arrive before ShellState
// exists, and without a later set_bounds the launcher stays 1×1 and Wayland
// never maps the undecorated window. Full story: docs/DESKTOP.md.
//
// An unreadable registry or settings file, or a grant that fails, must not
// stop the window from opening: each becomes a notice on the launcher. An
// `Err` out of setup is a panic in Tauri and, from a .desktop launch, an app
// that silently never appears. The one failure setup still returns is an app
// data dir that cannot be found or created, where nothing could be saved.

/// Every `eprintln!` in this crate goes to stderr AND to the shell's log file
/// (logfile.rs) once setup has opened it. A release build on Windows has no
/// console, and a Finder or Start-menu launch discards stderr, so without
/// the file a report from a person came with no evidence at all. Defined
/// before the modules so it shadows std's macro in every one of them, with no
/// call site changed; `::std::eprintln!` is the plain one.
macro_rules! eprintln {
    () => {
        $crate::logfile::line(format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::logfile::line(format_args!($($arg)*))
    };
}

mod background;
mod beacon;
mod commands;
mod layout;
mod links;
mod loading;
mod logfile;
mod notify;
mod registry;
mod reload;
mod settings;
mod suspend;
mod tray;
mod watch;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::PathBuf,
    sync::{Mutex, atomic::AtomicBool},
};

use serde::Serialize;
use tauri::Manager;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum View {
    /// The launcher fills the window: brand, instance list, add.
    Welcome,
    /// The active instance fills the window; the launcher is hidden.
    Active,
}

/// An instance whose page did not come up (loading.rs): it failed (WebKit's
/// `load-failed`, WebView2's `NavigationCompleted`, an HTTP 5xx), or it has
/// not loaded in `loading::LOAD_WAIT`. The launcher shows it as a card until
/// the next activation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadFailure {
    /// Increments per failure, so the launcher can tell a new one from one
    /// the person already dismissed.
    pub seq: u64,
    pub id: String,
    pub label: String,
    pub message: String,
    pub kind: FailureKind,
}

/// What the card offers (loading.rs). Every card has Retry and Back to
/// instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// Nothing answered (the network): the page is closed.
    Failed,
    /// Still loading behind the card: Keep waiting.
    Stalled,
    /// Something answered that is not Talaria (an error page, a challenge, a
    /// sign-in gate), kept behind the card: Show the page.
    Blocked,
}

/// The add in flight: Cancel aborts its probe, so a cancelled add can never
/// land in the registry after the dialog closed.
pub struct AddAttempt {
    pub seq: u64,
    pub abort: tokio::task::AbortHandle,
}

/// An instance whose webview the shell closed (suspend.rs). Showing it again
/// recreates the webview at `page` and puts its session and scroll back.
pub struct Suspended {
    /// The last page, as recorded (`last_urls`) when it was suspended.
    pub page: Option<String>,
    pub scroll: Vec<suspend::Scroll>,
    pub session: Vec<(String, String)>,
    /// Its notifications, while the page is gone (watch.rs).
    pub watcher: Option<tauri::async_runtime::JoinHandle<()>>,
    /// What that watcher posted, merged back into the page's record when it
    /// is restored (watch::Handback).
    pub handback: Option<watch::Shared>,
}

/// Lock discipline: no code holds one of these mutexes while it takes another,
/// except `adding`, which an add holds through registration, and which nothing
/// takes while holding another. Async commands run on tokio workers while sync
/// ones run on the main thread, so two locks taken in opposite orders on the
/// two sides would freeze the GTK loop every webview paints from.
pub struct ShellState {
    registry_path: PathBuf,
    settings_path: PathBuf,
    data_root: PathBuf,
    instances: Mutex<Vec<registry::Instance>>,
    /// Removed instances whose data dir the next launch deletes again
    /// (registry::sweep_removed).
    removed: Mutex<Vec<String>>,
    settings: Mutex<settings::DesktopSettings>,
    active: Mutex<Option<String>>,
    view: Mutex<View>,
    http: reqwest::Client,
    /// What went wrong at startup, for the launcher to say.
    notices: Mutex<Vec<String>>,
    /// The instance to reopen at boot. Offered to the launcher until the
    /// first view change, so a launcher reload never reopens it twice.
    reopen: Mutex<Option<String>>,
    failure: Mutex<Option<LoadFailure>>,
    failure_seq: std::sync::atomic::AtomicU64,
    /// Every instance page's load, for the watchdog (loading.rs).
    loads: Mutex<loading::Loads>,
    adding: Mutex<Option<AddAttempt>>,
    add_seq: std::sync::atomic::AtomicU64,
    /// Local id → the last page it showed (registry `lastUrls`).
    last_urls: Mutex<BTreeMap<String, String>>,
    /// The suspend clock (suspend.rs).
    activity: Mutex<suspend::Activity>,
    suspended: Mutex<HashMap<String, Suspended>>,
    /// The shell minimized the window itself and it has not been focused
    /// since: GTK on Wayland reports no minimized state of its own.
    self_minimized: AtomicBool,
    /// Local ids muted in the launcher (registry `muted`).
    muted: Mutex<BTreeSet<String>>,
    /// Local ids whose webview is being created right now
    /// (commands::Creating).
    creating: Mutex<BTreeSet<String>>,
}

impl ShellState {
    fn find(&self, id: &str) -> Result<registry::Instance, String> {
        self.instances
            .lock()
            .unwrap()
            .iter()
            .find(|i| i.id == id)
            .cloned()
            .ok_or_else(|| format!("no such instance: {id}"))
    }

    /// Write the registry with the view the window shows now, so a relaunch
    /// lands there.
    fn persist(&self) -> Result<(), String> {
        // `active` and `removed` are read first, each guard dropping at its
        // own `;`, so `instances` is the only lock held when it is taken (see
        // the lock discipline above). It stays held through the write, so two
        // writes cannot land out of order.
        let active = self.active.lock().unwrap().clone();
        let removed = self.removed.lock().unwrap().clone();
        let last_urls = self.last_urls.lock().unwrap().clone();
        let muted: Vec<String> = self.muted.lock().unwrap().iter().cloned().collect();
        let instances = self.instances.lock().unwrap();
        registry::save(
            &self.registry_path,
            &registry::Record {
                instances: &instances,
                last_view: active.as_deref(),
                removed: &removed,
                last_urls: &last_urls,
                muted: &muted,
            },
        )
    }
}

/// The switcher, window-chrome, updater, notification, badge and suspend
/// command set: the only IPC remote content ever gets, and only for origins
/// the user registered. Every entry is one of the shell's own commands
/// (build.rs). Never a `core:` permission: `core:event` would let an
/// instance page listen, and an `Any`-target event matches every listener
/// (tauri's `EventTarget::Any` short-circuits the webview filter), so one
/// instance could hear another's.
pub const INSTANCE_PERMISSIONS: &[&str] = &[
    "allow-list-instances",
    "allow-activate-instance",
    "allow-show-welcome",
    "allow-get-desktop-settings",
    "allow-set-titlebar-mode",
    "allow-desktop-window",
    "allow-check-for-update",
    "allow-install-update",
    "allow-desktop-notify",
    "allow-set-suspend-after",
    "allow-desktop-suspend-report",
    "allow-desktop-notification-status",
    "allow-desktop-notification-request",
    "allow-desktop-notification-test",
    "allow-desktop-set-badge",
    "allow-desktop-quit",
    "allow-set-close-to-background",
    "allow-desktop-notify-close",
    "allow-desktop-os-idle",
    "allow-desktop-reload",
    "allow-desktop-open-link",
];

/// Grant one instance origin `INSTANCE_PERMISSIONS`, for its own webview,
/// and the way back (`grant_way_back`).
fn grant_switcher(app: &tauri::AppHandle, instance: &registry::Instance) -> Result<(), String> {
    let capability = INSTANCE_PERMISSIONS.iter().fold(
        tauri::ipc::CapabilityBuilder::new(format!("switcher-{}", instance.id))
            .webview(format!("instance-{}", instance.id))
            .remote(instance.url.clone()),
        |capability, permission| capability.permission(*permission),
    );
    app.add_capability(capability)
        .map_err(|e| format!("granting the instance switcher access: {e}"))?;
    grant_way_back(app, instance)
}

/// What an instance's webview may call on any origin: the way back alone,
/// so a page the instance sent the person to (a sign-in provider) still has
/// the pill and Ctrl/Cmd+Shift+H. Never Quit, never anything else.
///
/// Frames: on Linux and macOS Tauri puts its IPC into the main frame only.
/// WebView2 (Windows) puts it into every frame, so there a frame is held by
/// the origin ACL and the gate, not by frame scope: a sandboxed microsite
/// frame has an opaque origin and is refused before any command runs; a
/// cross-origin frame reaches this grant only, and `show_welcome` takes it
/// only for the page on screen, once a second (commands.rs); a same-origin
/// frame is the instance's own content.
pub const WAY_BACK_PERMISSIONS: &[&str] = &["allow-show-welcome"];
/// Any http(s) page, with or without a port.
pub const WAY_BACK_ORIGINS: &[&str] = &["https://*", "https://*:*", "http://*", "http://*:*"];

fn grant_way_back(app: &tauri::AppHandle, instance: &registry::Instance) -> Result<(), String> {
    let capability = WAY_BACK_ORIGINS.iter().fold(
        WAY_BACK_PERMISSIONS.iter().fold(
            tauri::ipc::CapabilityBuilder::new(format!("way-back-{}", instance.id))
                .webview(format!("instance-{}", instance.id)),
            |capability, permission| capability.permission(*permission),
        ),
        |capability, origin| capability.remote(origin.to_string()),
    );
    app.add_capability(capability)
        .map_err(|e| format!("granting the instance its way back: {e}"))
}

/// What the shell's own requests call themselves (the crate version is
/// decorative — the release tag stamps the real one — so none is claimed).
pub(crate) const USER_AGENT: &str = "Talaria-Desktop";

/// An HTTP client for the shell's own requests. The platform verifier is
/// preferred (enterprise CAs), but it refuses to build when the system store
/// is empty — a Flatpak whose `SSL_CERT_FILE` points at a host path the
/// sandbox cannot see, or a runtime whose probe paths are empty. That error
/// aborts setup, and a `.desktop` launch (`Terminal=false`) shows nothing.
/// Mozilla's roots still verify.
///
/// Every client has the short connect bound, so a black-holed address fails
/// in seconds, and says what it is (reqwest sends no User-Agent of its own,
/// and a bot filter in front of an instance challenges a request with none);
/// `configure` adds the rest.
pub(crate) fn build_client(
    configure: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> reqwest::Client {
    let build = || {
        configure(
            reqwest::Client::builder()
                .connect_timeout(beacon::CONNECT_TIMEOUT)
                .user_agent(USER_AGENT),
        )
    };
    match build().build() {
        Ok(client) => client,
        Err(_) => {
            let certs = webpki_root_certs::TLS_SERVER_ROOT_CERTS
                .iter()
                .filter_map(|der| reqwest::Certificate::from_der(der.as_ref()).ok());
            build().tls_certs_only(certs).build().expect("http client")
        }
    }
}

/// Beacon client: two bounds, not one — the connect bound, and a longer total
/// so a slow-but-alive instance is not refused (beacon.rs has the numbers and
/// why).
fn http_client() -> reqwest::Client {
    build_client(|b| {
        b.timeout(beacon::TOTAL_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(3))
    })
}

pub fn run() {
    // A page's Quit waits out the first seconds of a launch (commands::desktop_quit).
    commands::launched();
    // A panic is the one thing `eprintln!` never says: the default hook writes
    // it to stderr, which a release build on Windows does not have. Into the
    // log file too, once setup has opened it.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        logfile::panic_line(format_args!("talaria-desktop: {info}"));
        default_hook(info);
    }));
    // `--notify-selftest[=<path>]`: the notification backend's report, with
    // no window (desktop-package.yml runs it on every platform's build).
    if let Some(path) = notify::selftest_arg(std::env::args().skip(1)) {
        std::process::exit(notify::selftest(path));
    }
    // One process per session: a second launch hands the window back to the
    // first (with its activation token) and leaves before it builds one.
    #[cfg(target_os = "linux")]
    if notify::linux::single_instance() {
        std::process::exit(0);
    }
    // The program name GTK gives X11's WM_CLASS, and the name a person reads
    // (`name_the_process`).
    #[cfg(target_os = "linux")]
    name_the_process();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // First, so everything setup says is in the file too. Windows:
            // %LOCALAPPDATA%\app.talaria.desktop\logs; macOS:
            // ~/Library/Logs/app.talaria.desktop; Linux:
            // ~/.local/share/app.talaria.desktop/logs. No dir: stderr only.
            match app.path().app_log_dir() {
                Ok(dir) => logfile::open(&dir),
                Err(e) => eprintln!("talaria-desktop: no log directory ({e}); logging to stderr only"),
            }
            eprintln!(
                "talaria-desktop: {} {} on {} {}",
                app.package_info().name,
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            // The identifier in tauri.conf.json decides this path on Linux
            // (~/.local/share/app.talaria.desktop) — changing it orphans the
            // registry and every instance's saved session.
            let dir = app.path().app_data_dir()?;
            fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
            let registry_path = dir.join("instances.json");
            let settings_path = dir.join("settings.json");
            let data_root = dir.join("instances");
            let mut notices = Vec::new();

            let mut loaded = registry::load(&registry_path);
            let last_urls = std::mem::take(&mut loaded.last_urls);
            let muted: BTreeSet<String> = std::mem::take(&mut loaded.muted).into_iter().collect();
            notices.extend(loaded.notice.clone());
            let (loaded_settings, settings_notice) = settings::load(&settings_path);
            notices.extend(settings_notice);

            for instance in &loaded.instances {
                if let Err(e) = grant_switcher(app.handle(), instance) {
                    eprintln!("talaria-desktop: {}: {e}", instance.url);
                    notices.push(format!(
                        "{} cannot use the in-app switcher this session ({e}); Ctrl+Shift+H still returns here",
                        instance.label
                    ));
                }
            }
            // The data dirs of instances the person removed, which a closing
            // WebKit process re-created. Nothing else: a dir no entry owns may
            // be the session of one a damaged registry lost.
            let reopen = loaded.resume.target(&loaded.instances);
            let swept = registry::sweep_removed(&data_root, &loaded.removed, &loaded.instances);
            for dir in &swept.deleted {
                eprintln!("talaria-desktop: deleted removed {}", dir.display());
            }
            if swept.pending != loaded.removed {
                // The view to resume is written as it was read: the state
                // below has no active instance yet.
                if let Err(e) = registry::save(
                    &registry_path,
                    &registry::Record {
                        instances: &loaded.instances,
                        last_view: reopen.as_deref(),
                        removed: &swept.pending,
                        last_urls: &last_urls,
                        muted: &muted.iter().cloned().collect::<Vec<_>>(),
                    },
                ) {
                    eprintln!("talaria-desktop: {e}");
                }
            }
            if let Some(window) = app.get_window("main") {
                // tauri.conf.json's size and minimum, shrunk to the monitor
                // when it is smaller.
                if let Some(config) = app.config().app.windows.first() {
                    let want = (config.width, config.height);
                    let min = (
                        config.min_width.unwrap_or(0.0),
                        config.min_height.unwrap_or(0.0),
                    );
                    layout::fit_to_monitor(&window, want, min);
                }
                if let Err(e) = settings::apply(&window, loaded_settings.titlebar) {
                    eprintln!("talaria-desktop: {e}");
                }
            }
            app.manage(ShellState {
                registry_path,
                settings_path,
                data_root,
                instances: Mutex::new(loaded.instances),
                removed: Mutex::new(swept.pending),
                settings: Mutex::new(loaded_settings),
                active: Mutex::new(None),
                view: Mutex::new(View::Welcome),
                http: http_client(),
                notices: Mutex::new(notices),
                reopen: Mutex::new(reopen),
                failure: Mutex::new(None),
                failure_seq: Default::default(),
                loads: Mutex::new(loading::Loads::default()),
                adding: Mutex::new(None),
                add_seq: Default::default(),
                last_urls: Mutex::new(last_urls),
                activity: Mutex::new(suspend::Activity::default()),
                suspended: Mutex::new(HashMap::new()),
                self_minimized: AtomicBool::new(false),
                muted: Mutex::new(muted),
                creating: Mutex::new(BTreeSet::new()),
            });
            app.manage(tray::Tray::default());
            notify::init(app.handle());
            background::init(app.handle());
            #[cfg(target_os = "linux")]
            install_quit_action(app.handle());
            // An in-app popup's window goes when its page closes (links.rs).
            #[cfg(target_os = "linux")]
            links::close_emptied_popups(app.handle());
            // View → Reload Page, for a page whose own scripts never ran.
            #[cfg(target_os = "macos")]
            reload::install_menu(app.handle());
            // D8: the signed-in instances nobody opens this session.
            commands::schedule_prewarm(app.handle());
            // The suspend clock. Each tick is a few lock reads on the main
            // thread; a page is only asked once it is due.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut every = tokio::time::interval(suspend::TICK);
                every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    every.tick().await;
                    let app = handle.clone();
                    if handle
                        .run_on_main_thread(move || commands::suspend_tick(&app))
                        .is_err()
                    {
                        break;
                    }
                }
            });
            // After state exists, and after decorations (they change the
            // client rect). The initial Resized often already happened.
            if let Some(window) = app.get_window("main") {
                layout::relayout(&window);
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Resized(_) => {
                layout::relayout(window);
                commands::window_returned(window.app_handle(), false);
            }
            tauri::WindowEvent::Focused(true) => {
                commands::window_returned(window.app_handle(), true);
            }
            tauri::WindowEvent::CloseRequested { api, .. } => {
                background::close_requested(window, api);
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_instances,
            commands::get_shell_state,
            commands::add_instance,
            commands::cancel_add_instance,
            commands::probe_instance,
            commands::activate_instance,
            commands::show_welcome,
            commands::close_instance_page,
            commands::show_instance_page,
            commands::remove_instance,
            commands::get_desktop_settings,
            commands::set_titlebar_mode,
            commands::set_suspend_after,
            commands::desktop_suspend_report,
            commands::desktop_window,
            commands::check_for_update,
            commands::install_update,
            commands::desktop_notify,
            commands::desktop_notification_status,
            commands::desktop_notification_request,
            commands::desktop_notification_test,
            commands::desktop_set_badge,
            commands::desktop_quit,
            commands::set_close_to_background,
            commands::desktop_notify_close,
            commands::desktop_os_idle,
            commands::set_instance_muted,
            reload::desktop_reload,
            links::desktop_open_link,
        ])
        .build(tauri::generate_context!())
        .expect("talaria desktop shell could not start");
    app.run(|handle, event| match event {
        // The dock icon, clicked with the window closed to the background.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => background::present_main(handle, None),
        // Cmd+Q, the app menu's Quit, or `app.exit`: remember every page.
        tauri::RunEvent::ExitRequested { .. } => commands::remember_all(handle),
        // The last event before the process goes (and before an updater
        // relaunch spawns its successor): the single-instance name is free
        // for the next launch at once.
        tauri::RunEvent::Exit => {
            #[cfg(target_os = "linux")]
            notify::linux::release_single_instance();
        }
        _ => {}
    });
}

/// The desktop's own Quit (GNOME's dash and Alt+Tab menu, a dock's): GNOME
/// Shell's `shell_app_request_quit` activates the application's `app.quit`
/// action when it has one, and only otherwise deletes each window — the
/// same close request as ×, which keeps Talaria running. With the GTK
/// application registered under `app.talaria.desktop`, this is that action:
/// remember every page, then exit 0, as Ctrl+Q does.
#[cfg(target_os = "linux")]
fn install_quit_action(app: &tauri::AppHandle) {
    use gtk::prelude::*;
    let Some(window) = app.get_window("main") else {
        return;
    };
    let application = match window.gtk_window() {
        Ok(w) => w.application(),
        Err(e) => {
            eprintln!("talaria-desktop: the GTK window: {e}");
            return;
        }
    };
    let Some(application) = application else {
        eprintln!("talaria-desktop: no GTK application; the desktop's Quit closes the window");
        return;
    };
    let quit = gtk::gio::SimpleAction::new("quit", None);
    let handle = app.clone();
    quit.connect_activate(move |_, _| {
        eprintln!("talaria-desktop: the desktop asked Talaria to quit (app.quit)");
        background::quit(&handle);
    });
    application.add_action(&quit);
    // The registered application also answers D-Bus `Activate`
    // (`gapplication launch app.talaria.desktop`, a dock or launcher that
    // activates by GApplication). Each one reaches tao's `activate` handler,
    // which hands tauri a fresh start, and tauri runs its setup again and
    // panics on the `main` window that already exists. Tao's one start has
    // happened — setup, which is running this, came from it — so every
    // handler connected so far is blocked, and an activate shows the window
    // instead, as a second launch does.
    let blocked = block_activate_handlers(&application);
    let handle = app.clone();
    application.connect_activate(move |_| {
        eprintln!(
            "talaria-desktop: the desktop activated Talaria (GApplication); showing the window"
        );
        background::present_main(&handle, None);
    });
    eprintln!(
        "talaria-desktop: GTK application {} (prgname {}): app.quit exported; activate shows the window ({blocked} start handler(s) retired)",
        application
            .application_id()
            .map_or("(none)".to_string(), |id| id.to_string()),
        gtk::glib::prgname().map_or("(none)".to_string(), |p| p.to_string())
    );
}

/// The process's two GLib names, set before GTK starts.
///
/// The program name is X11's `WM_CLASS`: the app id, as the Wayland `app_id`
/// is (`enableGTKAppId`), so a dock on either matches the window to
/// `app.talaria.desktop.desktop`, or to `Talaria.desktop` by its
/// `StartupWMClass` (packaging/linux).
///
/// The application name is the one a person reads. Unset, GLib answers the
/// program name for it, and libayatana-appindicator gives that as the tray
/// item's StatusNotifierItem `Title` (the entry's name in Plasma's tray and
/// its settings): `app.talaria.desktop`.
#[cfg(target_os = "linux")]
fn name_the_process() {
    gtk::glib::set_prgname(Some(notify::linux::APP_ID));
    gtk::glib::set_application_name(notify::linux::APP_NAME);
}

/// Block every handler of the application's `activate` signal connected so
/// far (tao's start handler); the number blocked.
#[cfg(target_os = "linux")]
fn block_activate_handlers(application: &gtk::Application) -> u32 {
    use gtk::glib::{gobject_ffi, object::ObjectType};
    // SAFETY: `application` is a live GApplication for the whole call; the
    // signal id is looked up on GApplication's own type with a NUL-terminated
    // name; the match is by signal id alone, so the closure, func and data
    // filters are unused and null.
    unsafe {
        let signal = gobject_ffi::g_signal_lookup(
            c"activate".as_ptr(),
            gtk::gio::ffi::g_application_get_type(),
        );
        if signal == 0 {
            return 0;
        }
        gobject_ffi::g_signal_handlers_block_matched(
            application.as_ptr().cast(),
            gobject_ffi::G_SIGNAL_MATCH_ID,
            signal,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_remote_origin_gets_a_core_permission() {
        assert!(
            INSTANCE_PERMISSIONS.iter().all(|p| !p.starts_with("core:")),
            "{INSTANCE_PERMISSIONS:?}"
        );
        // Each is one of the shell's own generated command permissions.
        assert!(INSTANCE_PERMISSIONS.iter().all(|p| p.starts_with("allow-")));
        for needed in [
            "allow-desktop-notify",
            "allow-desktop-notification-status",
            "allow-desktop-set-badge",
            "allow-desktop-quit",
            "allow-desktop-notify-close",
            "allow-desktop-os-idle",
            // The key script reloads through it (reload.rs).
            "allow-desktop-reload",
            // A clicked mailto: reaches the OS through it (links.rs).
            "allow-desktop-open-link",
        ] {
            assert!(INSTANCE_PERMISSIONS.contains(&needed), "{needed}");
        }
    }

    // B3: what any origin may call in an instance webview: the way back,
    // nothing more, and every origin's patterns parse as Tauri reads them.
    #[test]
    fn any_origin_gets_the_way_back_and_nothing_else() {
        assert_eq!(WAY_BACK_PERMISSIONS, &["allow-show-welcome"]);
        assert!(!WAY_BACK_PERMISSIONS.contains(&"allow-desktop-quit"));
        for origin in WAY_BACK_ORIGINS {
            assert!(
                origin.starts_with("http://") || origin.starts_with("https://"),
                "{origin}"
            );
            let pattern: tauri::utils::acl::RemoteUrlPattern = origin.parse().unwrap();
            assert!(
                !pattern.test(&"tauri://localhost/".parse().unwrap()),
                "{origin}"
            );
        }
        let matches = |url: &str| {
            let url: tauri::Url = url.parse().unwrap();
            WAY_BACK_ORIGINS.iter().any(|o| {
                o.parse::<tauri::utils::acl::RemoteUrlPattern>()
                    .unwrap()
                    .test(&url)
            })
        };
        assert!(matches("https://accounts.google.com/signin?x=1"));
        assert!(matches("http://127.0.0.1:47394/provider"));
        // Not the custom scheme the launcher is served on (macOS, Linux).
        assert!(!matches("tauri://localhost/index.html"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_process_has_a_name_for_docks_and_one_for_people() {
        name_the_process();
        assert_eq!(gtk::glib::prgname().as_deref(), Some("app.talaria.desktop"));
        // What the tray's StatusNotifierItem Title reads (the program name
        // when this is unset).
        assert_eq!(gtk::glib::application_name().as_deref(), Some("Talaria"));
    }

    #[test]
    fn every_instance_permission_has_a_generated_command() {
        let build = include_str!("../build.rs");
        for permission in INSTANCE_PERMISSIONS {
            let command = permission.trim_start_matches("allow-").replace('-', "_");
            assert!(
                build.contains(&format!("\"{command}\"")),
                "{command} is not in build.rs"
            );
        }
    }
}
