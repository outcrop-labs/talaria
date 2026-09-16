//! The five shell commands. Thin by design: validation and persistence live
//! in beacon.rs / registry.rs, geometry in layout.rs — these wire them to the
//! window and its webviews.

use tauri::{AppHandle, Manager, WebviewBuilder, WebviewUrl, Window};

use crate::{ShellState, beacon, layout, registry, registry::Instance};

fn main_window(app: &AppHandle) -> Result<Window, String> {
    app.get_window("main")
        .ok_or_else(|| "the main window is gone".to_string())
}

fn instance_label(id: &str) -> String {
    format!("instance-{id}")
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
    Ok(instance)
}

/// Show an instance: create its webview if this is the first open — born at
/// its final rect (WebviewBuilder cannot create hidden, and WebKitGTK
/// mis-places new children until a set_bounds, tauri#10420) — then position
/// everything and reveal.
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
                    WebviewBuilder::new(&label, WebviewUrl::External(url)).data_directory(dir),
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
    layout::relayout(&window);
    webview
        .show()
        .map_err(|e| format!("showing the instance webview: {e}"))?;
    window
        .set_title(&format!("Talaria — {}", instance.label))
        .map_err(|e| format!("setting the window title: {e}"))?;
    Ok(())
}

/// Back to the welcome screen: every instance webview hidden (they stay
/// loaded — sessions and SSE survive), launcher back to full-window.
#[tauri::command]
pub fn show_welcome(app: AppHandle) -> Result<(), String> {
    let state = app.state::<ShellState>();
    let window = main_window(&app)?;
    *state.active.lock().unwrap() = None;
    for webview in window.webviews() {
        if webview.label().starts_with("instance-") {
            let _ = webview.hide();
        }
    }
    layout::relayout(&window);
    window
        .set_title("Talaria")
        .map_err(|e| format!("setting the window title: {e}"))?;
    Ok(())
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
        layout::relayout(&window);
        window
            .set_title("Talaria")
            .map_err(|e| format!("setting the window title: {e}"))?;
    }
    Ok(state.instances.lock().unwrap().clone())
}
