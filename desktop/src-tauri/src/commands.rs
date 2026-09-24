//! The five shell commands. Thin by design: validation and persistence live
//! in beacon.rs / registry.rs, geometry in layout.rs — these wire them to the
//! window and its webviews.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Webview, WebviewBuilder, WebviewUrl, Window};
use tauri_plugin_updater::UpdaterExt;

use crate::{ShellState, View, beacon, layout, notify, registry, registry::Instance, settings};

fn main_window(app: &AppHandle) -> Result<Window, String> {
    app.get_window("main")
        .ok_or_else(|| "the main window is gone".to_string())
}

fn instance_label(id: &str) -> String {
    format!("instance-{id}")
}

/// Injected into every instance webview: Ctrl/Cmd+Shift+H returns to the
/// launcher. The in-UI switcher is the intended way back, but an instance
/// running an older Talaria (the switcher ships with the UI) would otherwise
/// be a one-way door — the shell itself provides the floor.
const ESCAPE_HATCH: &str = r#"
  if (window.__TAURI_INTERNALS__) {
    window.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'h') {
        e.preventDefault();
        window.__TAURI_INTERNALS__.invoke('show_welcome').catch(() => {});
      }
    });
  }
"#;

/// Change the view: geometry via relayout, plus show/hide so a zero-width
/// launcher webview can never flash. The active instance webview (hidden but
/// loaded in welcome) is shown again by activate_instance.
fn set_view(app: &AppHandle, view: View) -> Result<(), String> {
    let state = app.state::<ShellState>();
    let window = main_window(app)?;
    *state.view.lock().unwrap() = view;
    if let Some(launcher) = window.get_webview("main") {
        match view {
            View::Active => {
                let _ = launcher.hide();
            }
            View::Welcome => {
                let _ = launcher.show();
            }
        }
    }
    if view == View::Welcome {
        for webview in window.webviews() {
            if webview.label().starts_with("instance-") {
                let _ = webview.hide();
            }
        }
    }
    layout::relayout(&window);
    window
        .set_title("Talaria")
        .map_err(|e| format!("setting the window title: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn list_instances(app: AppHandle) -> Vec<Instance> {
    app.state::<ShellState>().instances.lock().unwrap().clone()
}

/// Validate a URL against the instance beacon, then register it. The beacon's
/// uuid is the dedupe key: the same deployment reachable two ways is one
/// instance, not two.
#[tauri::command]
pub async fn add_instance(app: AppHandle, url: String) -> Result<Instance, String> {
    let state = app.state::<ShellState>();
    let origin = beacon::normalize_origin(&url)?;
    let beacon = beacon::probe(&state.http, &beacon::origin_string(&origin)).await?;
    {
        let instances = state.instances.lock().unwrap();
        if let Some(existing) = instances.iter().find(|i| i.instance_id == beacon.instance) {
            return Err(format!(
                "that instance is already added as “{}”",
                existing.label
            ));
        }
    }
    let label = beacon
        .company_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match (origin.host_str(), origin.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            _ => origin.to_string(),
        });
    let instance = Instance {
        id: uuid::Uuid::new_v4().to_string(),
        instance_id: beacon.instance,
        label,
        url: beacon::origin_string(&origin),
        created_at: registry::now_ms(),
        last_opened_at: None,
    };
    let mut instances = state.instances.lock().unwrap();
    instances.push(instance.clone());
    registry::save(&state.registry_path, &instances)?;
    drop(instances);
    // The new origin can host the in-UI switcher from the moment it loads.
    crate::grant_switcher(&app, &instance)?;
    Ok(instance)
}

/// Show an instance: create its webview if this is the first open — born at
/// its final rect (WebviewBuilder cannot create hidden, and WebKitGTK
/// mis-places new children until a set_bounds, tauri#10420) — then hand the
/// whole window to it.
#[tauri::command]
pub fn activate_instance(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<ShellState>();
    let instance = state.find(&id)?;
    let window = main_window(&app)?;
    let label = instance_label(&instance.id);

    for other in window.webviews() {
        if other.label().starts_with("instance-") && other.label() != label {
            let _ = other.hide();
        }
    }
    *state.active.lock().unwrap() = Some(instance.id.clone());

    let webview = match window.get_webview(&label) {
        Some(webview) => webview,
        None => {
            // Per-instance data dir = per-instance cookie jar and storage.
            // Cookies ignore ports, so two dev instances on 127.0.0.1:530x
            // would share a session in one shared jar — directories don't.
            let dir = state.data_root.join(&instance.id).join("webview");
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("creating {}: {e}", dir.display()))?;
            let url = instance
                .url
                .parse()
                .map_err(|e| format!("stored URL no longer parses: {e}"))?;
            let rect = layout::creation_rect(&window);
            window
                .add_child(
                    WebviewBuilder::new(&label, WebviewUrl::External(url))
                        .data_directory(dir)
                        .initialization_script(ESCAPE_HATCH),
                    rect.position,
                    rect.size,
                )
                .map_err(|e| format!("creating the instance webview: {e}"))?
        }
    };

    {
        let mut instances = state.instances.lock().unwrap();
        if let Some(i) = instances.iter_mut().find(|i| i.id == instance.id) {
            i.last_opened_at = Some(registry::now_ms());
        }
        registry::save(&state.registry_path, &instances)?;
    }
    set_view(&app, View::Active)?;
    webview
        .show()
        .map_err(|e| format!("showing the instance webview: {e}"))?;
    window
        .set_title(&format!("Talaria — {}", instance.label))
        .map_err(|e| format!("setting the window title: {e}"))?;
    Ok(())
}

/// Back to the launcher: every instance webview hidden (they stay loaded —
/// sessions and SSE survive), the welcome screen full-window. Reachable from
/// the in-UI switcher ("manage instances") as well as the shell itself.
#[tauri::command]
pub fn show_welcome(app: AppHandle) -> Result<(), String> {
    *app.state::<ShellState>().active.lock().unwrap() = None;
    set_view(&app, View::Welcome)
}

/// Remove an instance: close its webview, delete its data dir (the saved
/// session dies with it), forget it from the registry.
#[tauri::command]
pub fn remove_instance(app: AppHandle, id: String) -> Result<Vec<Instance>, String> {
    let state = app.state::<ShellState>();
    let window = main_window(&app)?;
    let label = instance_label(&id);
    if let Some(webview) = window.get_webview(&label) {
        webview
            .close()
            .map_err(|e| format!("closing the instance webview: {e}"))?;
    }
    let was_active = state.active.lock().unwrap().as_deref() == Some(id.as_str());
    {
        let mut instances = state.instances.lock().unwrap();
        instances.retain(|i| i.id != id);
        registry::save(&state.registry_path, &instances)?;
    }
    let dir = state.data_root.join(&id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("deleting {}: {e}", dir.display()))?;
    }
    if was_active {
        *state.active.lock().unwrap() = None;
        set_view(&app, View::Welcome)?;
    }
    Ok(state.instances.lock().unwrap().clone())
}

#[tauri::command]
pub fn get_desktop_settings(app: AppHandle) -> settings::DesktopSettings {
    app.state::<ShellState>().settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_titlebar_mode(
    app: AppHandle,
    mode: settings::TitlebarMode,
) -> Result<settings::DesktopSettings, String> {
    let state = app.state::<ShellState>();
    {
        let mut current = state.settings.lock().unwrap();
        current.titlebar = mode;
        settings::save(&state.settings_path, &current)?;
    }
    let window = main_window(&app)?;
    settings::apply(&window, mode)?;
    Ok(state.settings.lock().unwrap().clone())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowAction {
    Minimize,
    ToggleMaximize,
    Close,
    StartDragging,
}

#[tauri::command]
pub fn desktop_window(app: AppHandle, action: WindowAction) -> Result<(), String> {
    let window = main_window(&app)?;
    match action {
        WindowAction::Minimize => window.minimize().map_err(|e| format!("minimizing: {e}"))?,
        WindowAction::ToggleMaximize => {
            if window
                .is_maximized()
                .map_err(|e| format!("reading maximize: {e}"))?
            {
                window
                    .unmaximize()
                    .map_err(|e| format!("unmaximizing: {e}"))?;
            } else {
                window.maximize().map_err(|e| format!("maximizing: {e}"))?;
            }
        }
        WindowAction::Close => window.close().map_err(|e| format!("closing: {e}"))?,
        WindowAction::StartDragging => window
            .start_dragging()
            .map_err(|e| format!("starting a drag: {e}"))?,
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
}

/// Compare this build to GitHub's latest stable `latest.json`. None means
/// this version is current (or the endpoint had nothing newer).
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|e| format!("updater: {e}"))?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(UpdateInfo {
            version: update.version,
            notes: update.body,
        })),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("checking for an update: {e}")),
    }
}

/// Download the latest signed payload, apply it, and relaunch. The updater
/// verifies the minisign signature against the pubkey in tauri.conf.json
/// before touching the install.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| format!("updater: {e}"))?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("checking for an update: {e}"))?
    else {
        return Err("already on the latest version".into());
    };
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("installing the update: {e}"))?;
    app.restart();
}

/// Post an OS notification. The webview's Notification API reads denied inside
/// the shell and has no site-settings page; this is the path that actually
/// reaches the OS. A click focuses the window, brings the calling instance
/// forward, and opens `href` when it is a same-origin path.
#[tauri::command]
pub fn desktop_notify(
    app: AppHandle,
    webview: Webview,
    title: String,
    body: Option<String>,
    tag: Option<String>,
    href: Option<String>,
) -> Result<(), String> {
    let title = notify::clamped_title(&title);
    if title.is_empty() {
        return Err("a notification needs a title".into());
    }
    let body = body
        .as_deref()
        .map(notify::clamped_body)
        .filter(|b| !b.is_empty());
    let tag = tag
        .as_deref()
        .map(notify::clamped_tag)
        .filter(|t| !t.is_empty());
    let label = webview.label().to_string();
    let href = href
        .as_deref()
        .and_then(notify::openable_href)
        .map(str::to_string);
    notify::post(&app, &title, body.as_deref(), tag.as_deref(), {
        let app = app.clone();
        move || reveal_notice(&app, &label, href.as_deref())
    })
}

fn reveal_notice(app: &AppHandle, label: &str, href: Option<&str>) {
    let label = label.to_string();
    let href = href.map(str::to_string);
    let app = app.clone();
    // WebKitGTK eval and window ops belong on the main thread. The click
    // waiter is a blocking thread; this only queues the reveal.
    let _ = app.clone().run_on_main_thread(move || {
        if let Some(id) = label.strip_prefix("instance-") {
            let _ = activate_instance(app.clone(), id.to_string());
        }
        if let Some(window) = app.get_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
        if let Some(href) = href
            && let Some(js) = notify::assign_js(&href)
            && let Some(webview) = app.get_webview(&label)
        {
            let _ = webview.eval(js);
        }
    });
}
