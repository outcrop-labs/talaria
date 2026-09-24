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
// localStorage never mix; hidden webviews stay loaded (sessions and SSE
// survive a switch).
//
// The shell owns all geometry: WebKitGTK mis-places new child webviews until
// their first set_bounds (tauri#10420) and auto_resize breaks across resize
// cycles (tauri#10131), so layout.rs applies logical-unit bounds at the end
// of setup and again on every resize, and never uses auto_resize. The setup
// pass is load-bearing: the initial Resized can arrive before ShellState
// exists, and without a later set_bounds the launcher stays 1×1 and Wayland
// never maps the undecorated window. Full story: docs/DESKTOP.md.

mod beacon;
mod commands;
mod layout;
mod notify;
mod registry;
mod settings;
use std::{fs, path::PathBuf, sync::Mutex, time::Duration};

use tauri::Manager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The launcher fills the window: brand, instance list, add.
    Welcome,
    /// The active instance fills the window; the launcher is hidden.
    Active,
}

pub struct ShellState {
    registry_path: PathBuf,
    settings_path: PathBuf,
    data_root: PathBuf,
    instances: Mutex<Vec<registry::Instance>>,
    settings: Mutex<settings::DesktopSettings>,
    active: Mutex<Option<String>>,
    view: Mutex<View>,
    http: reqwest::Client,
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
}

/// Grant one instance origin the switcher, window-chrome, updater, and
/// notification command set — the only IPC remote content ever gets, and only for origins
/// the user registered.
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
        .permission("allow-desktop-notify");
    app.add_capability(capability)
        .map_err(|e| format!("granting the instance switcher access: {e}"))
}

/// Beacon client. The platform verifier is preferred (enterprise CAs), but
/// it refuses to build when the system store is empty — a Flatpak whose
/// `SSL_CERT_FILE` points at a host path the sandbox cannot see, or a
/// runtime whose probe paths are empty. That error aborts setup, and a
/// `.desktop` launch (`Terminal=false`) shows nothing. Mozilla's roots
/// still verify the beacon.
fn http_client() -> reqwest::Client {
    let build = || {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::limited(3))
    };
    match build().build() {
        Ok(client) => client,
        Err(_) => {
            let certs = webpki_root_certs::TLS_SERVER_ROOT_CERTS
                .iter()
                .filter_map(|der| reqwest::Certificate::from_der(der.as_ref()).ok());
            build()
                .tls_certs_only(certs)
                .build()
                .expect("beacon client")
        }
    }
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
            let instances = registry::load(&registry_path)?;
            let loaded_settings = settings::load(&settings_path)?;
            for instance in &instances {
                grant_switcher(app.handle(), instance)?;
            }
            if let Some(window) = app.get_window("main") {
                settings::apply(&window, loaded_settings.titlebar)?;
            }
            app.manage(ShellState {
                registry_path,
                settings_path,
                data_root: dir.join("instances"),
                instances: Mutex::new(instances),
                settings: Mutex::new(loaded_settings),
                active: Mutex::new(None),
                view: Mutex::new(View::Welcome),
                http: http_client(),
            });
            // After state exists, and after decorations (they change the
            // client rect). The initial Resized often already happened.
            if let Some(window) = app.get_window("main") {
                layout::relayout(&window);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Resized(_)) {
                layout::relayout(window);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_instances,
            commands::add_instance,
            commands::activate_instance,
            commands::show_welcome,
            commands::remove_instance,
            commands::get_desktop_settings,
            commands::set_titlebar_mode,
            commands::desktop_window,
            commands::check_for_update,
            commands::install_update,
            commands::desktop_notify,
        ])
        .run(tauri::generate_context!())
        .expect("talaria desktop shell exited unexpectedly");
}
