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
//     and a notification post — no fs, nothing else.
//
// Every instance webview gets its own data directory, so cookie jars and
// localStorage never mix. A hidden webview stays loaded (sessions and SSE
// survive a switch) until it has been off screen for the suspend timeout;
// then, if its page agrees, it is closed, and showing it again recreates it at
// the exact page it was on (suspend.rs; the shell watches its notifications
// meanwhile, watch.rs).
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

mod beacon;
mod commands;
mod layout;
mod notify;
mod registry;
mod settings;
mod suspend;
mod watch;
use std::{
    collections::{BTreeMap, HashMap},
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

/// An instance whose page failed to load (WebKit's `load-failed`, Linux). The
/// launcher shows it with a retry until the next activation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadFailure {
    /// Increments per failure, so the launcher can tell a new one from one
    /// the person already dismissed.
    pub seq: u64,
    pub id: String,
    pub label: String,
    pub message: String,
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
    // Written by the WebKitGTK load-failed hook, which is Linux-only.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    failure_seq: std::sync::atomic::AtomicU64,
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
        let instances = self.instances.lock().unwrap();
        registry::save(
            &self.registry_path,
            &registry::Record {
                instances: &instances,
                last_view: active.as_deref(),
                removed: &removed,
                last_urls: &last_urls,
            },
        )
    }
}

/// Grant one instance origin the switcher, window-chrome, updater,
/// notification and suspend command set — the only IPC remote content ever
/// gets, and only for origins the user registered.
fn grant_switcher(app: &tauri::AppHandle, instance: &registry::Instance) -> Result<(), String> {
    let capability = tauri::ipc::CapabilityBuilder::new(format!("switcher-{}", instance.id))
        .webview(format!("instance-{}", instance.id))
        .remote(instance.url.clone())
        .permission("allow-list-instances")
        .permission("allow-activate-instance")
        .permission("allow-show-welcome")
        .permission("allow-get-desktop-settings")
        .permission("allow-set-titlebar-mode")
        .permission("allow-desktop-window")
        .permission("allow-check-for-update")
        .permission("allow-install-update")
        .permission("allow-desktop-notify")
        .permission("allow-set-suspend-after")
        .permission("allow-desktop-suspend-report");
    app.add_capability(capability)
        .map_err(|e| format!("granting the instance switcher access: {e}"))
}

/// An HTTP client for the shell's own requests. The platform verifier is
/// preferred (enterprise CAs), but it refuses to build when the system store
/// is empty — a Flatpak whose `SSL_CERT_FILE` points at a host path the
/// sandbox cannot see, or a runtime whose probe paths are empty. That error
/// aborts setup, and a `.desktop` launch (`Terminal=false`) shows nothing.
/// Mozilla's roots still verify.
///
/// Every client has the short connect bound, so a black-holed address fails
/// in seconds; `configure` adds the rest.
pub(crate) fn build_client(
    configure: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> reqwest::Client {
    let build = || configure(reqwest::Client::builder().connect_timeout(beacon::CONNECT_TIMEOUT));
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
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
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
                adding: Mutex::new(None),
                add_seq: Default::default(),
                last_urls: Mutex::new(last_urls),
                activity: Mutex::new(suspend::Activity::default()),
                suspended: Mutex::new(HashMap::new()),
                self_minimized: AtomicBool::new(false),
            });
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
            tauri::WindowEvent::CloseRequested { .. } => commands::remember_all(window.app_handle()),
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
            commands::remove_instance,
            commands::get_desktop_settings,
            commands::set_titlebar_mode,
            commands::set_suspend_after,
            commands::desktop_suspend_report,
            commands::desktop_window,
            commands::check_for_update,
            commands::install_update,
            commands::desktop_notify,
        ])
        .run(tauri::generate_context!())
        .expect("talaria desktop shell exited unexpectedly");
}
