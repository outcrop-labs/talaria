// Talaria Desktop — the multitenant shell around Talaria instances.
//
// One window, webviews in two roles:
//
//   * the launcher (label "main") — local content, the only webview with IPC
//     (capabilities/launcher.json + the AppManifest gate in build.rs). It is
//     the full-window welcome screen until an instance activates, then a
//     240px sidebar beside it.
//   * one webview per registered instance (label "instance-<id>") — remote
//     content at that instance's own origin, each with its own data directory
//     so cookie jars and localStorage never mix. No capability, no IPC.
//
// The shell owns all geometry: WebKitGTK mis-places new child webviews until
// their first set_bounds (tauri#10420) and auto_resize breaks across resize
// cycles (tauri#10131), so layout.rs recomputes logical-unit bounds on every
// resize and never uses auto_resize. Full story: docs/DESKTOP.md.

mod beacon;
mod commands;
mod layout;
mod registry;

use std::{fs, path::PathBuf, sync::Mutex, time::Duration};

use tauri::Manager;

pub struct ShellState {
    registry_path: PathBuf,
    data_root: PathBuf,
    instances: Mutex<Vec<registry::Instance>>,
    active: Mutex<Option<String>>,
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

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // The identifier in tauri.conf.json decides this path on Linux
            // (~/.local/share/app.talaria.desktop) — changing it orphans the
            // registry and every instance's saved session.
            let dir = app.path().app_data_dir()?;
            fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
            let registry_path = dir.join("instances.json");
            let instances = registry::load(&registry_path)?;
            app.manage(ShellState {
                registry_path,
                data_root: dir.join("instances"),
                instances: Mutex::new(instances),
                active: Mutex::new(None),
                http: reqwest::Client::builder()
                    .timeout(Duration::from_secs(8))
                    .redirect(reqwest::redirect::Policy::limited(3))
                    .build()?,
            });
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
        ])
        .run(tauri::generate_context!())
        .expect("talaria desktop shell exited unexpectedly");
}
