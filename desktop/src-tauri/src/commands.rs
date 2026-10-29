//! The shell commands. Thin by design: validation and persistence live
//! in beacon.rs / registry.rs, geometry in layout.rs — these wire them to the
//! window and its webviews.
//!
//! Threading: a sync `#[tauri::command]` runs on the main thread in Tauri v2,
//! which is also the GTK loop every webview paints from. Anything that can
//! wait on the network or on another process — the beacon probe, the
//! updater, the OS notification service — is `async` and off that thread.
//! Window and webview operations stay sync, on the main thread, because
//! WebKitGTK geometry depends on their order (docs/DESKTOP.md).

use std::{sync::atomic::Ordering, time::Duration};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, EventTarget, Manager, Webview, WebviewBuilder, WebviewUrl, Window,
    window::Color,
};
use tauri_plugin_updater::UpdaterExt;

use crate::{
    AddAttempt, LoadFailure, ShellState, View, beacon, layout, notify, registry,
    registry::Instance, settings,
};

/// The launcher's ground (`--color-ground` in desktop/src/app.css), painted by
/// every instance webview until its page does. Without it WebKitGTK shows
/// white, then the page, on every first open.
const GROUND: Color = Color(9, 10, 9, 255);

/// Emitted to the launcher webview whenever anything it shows changes — from
/// either side. An instance's switcher, its Settings → Profile titlebar
/// choice and a notification click all change shell state while the launcher
/// is hidden, and it used to keep showing what it had loaded at mount.
pub const STATE_EVENT: &str = "shell-state";

/// What an add that the person cancelled answers.
pub const CANCELLED: &str = "cancelled";

/// The update check's bound. GitHub answers in about a second and a half; the
/// plugin's default is no bound at all, which was 30 s of a disabled button
/// on a network that drops packets.
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `desktop_notify` waits for the OS notification service before it
/// answers the page anyway. The post keeps going on its own thread.
const NOTIFY_WAIT: Duration = Duration::from_secs(2);

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

/// Everything the launcher renders, in one read. The launcher asks for it at
/// boot and whenever it becomes visible, and Rust pushes it on every change
/// (`STATE_EVENT`).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSnapshot {
    pub view: View,
    pub active_id: Option<String>,
    pub titlebar: settings::TitlebarMode,
    pub instances: Vec<Instance>,
    /// Instances with a live webview this session: opening one is instant.
    pub loaded: Vec<String>,
    /// What went wrong at startup (an unreadable registry or settings file).
    pub notices: Vec<String>,
    /// The instance to reopen at boot, until the first view change.
    pub reopen: Option<String>,
    pub failure: Option<LoadFailure>,
}

fn snapshot(app: &AppHandle) -> ShellSnapshot {
    let state = app.state::<ShellState>();
    let loaded = app
        .get_window("main")
        .map(|w| {
            w.webviews()
                .iter()
                .filter_map(|v| v.label().strip_prefix("instance-").map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    // One lock at a time, each guard dropping at its own `;`. Read inside the
    // struct literal, every guard lived to its end: view, active and settings
    // were held while instances was taken, the reverse of `persist`, which an
    // add runs on a tokio worker while the main thread reads this.
    let view = *state.view.lock().unwrap();
    let active_id = state.active.lock().unwrap().clone();
    let titlebar = state.settings.lock().unwrap().titlebar;
    let instances = state.instances.lock().unwrap().clone();
    let notices = state.notices.lock().unwrap().clone();
    let reopen = state.reopen.lock().unwrap().clone();
    let failure = state.failure.lock().unwrap().clone();
    ShellSnapshot {
        view,
        active_id,
        titlebar,
        instances,
        loaded,
        notices,
        reopen,
        failure,
    }
}

/// Push the current state to the launcher. Never call it holding a lock.
fn emit_state(app: &AppHandle) {
    let _ = app.emit_to(EventTarget::webview("main"), STATE_EVENT, snapshot(app));
}

/// Change the view, then make exactly one webview visible: the launcher in
/// welcome, the active instance in active. Every other one is hidden FIRST —
/// wry packs child webviews in a vertical GtkBox, so two visible children for
/// even one layout pass split the window between them.
fn set_view(app: &AppHandle, view: View) -> Result<(), String> {
    let state = app.state::<ShellState>();
    let window = main_window(app)?;
    *state.view.lock().unwrap() = view;
    let active = state.active.lock().unwrap().clone();
    let (keep, title) = match (view, active) {
        (View::Active, Some(id)) => {
            let title = state
                .find(&id)
                .map(|i| format!("Talaria — {}", i.label))
                .unwrap_or_else(|_| "Talaria".into());
            (instance_label(&id), title)
        }
        _ => ("main".to_string(), "Talaria".to_string()),
    };
    for webview in window.webviews() {
        if webview.label() != keep {
            let _ = webview.hide();
        }
    }
    layout::relayout(&window);
    if let Some(webview) = window.get_webview(&keep) {
        webview
            .show()
            .map_err(|e| format!("showing the {keep} webview: {e}"))?;
    }
    window
        .set_title(&title)
        .map_err(|e| format!("setting the window title: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn list_instances(app: AppHandle) -> Vec<Instance> {
    app.state::<ShellState>().instances.lock().unwrap().clone()
}

#[tauri::command]
pub fn get_shell_state(app: AppHandle) -> ShellSnapshot {
    snapshot(&app)
}

/// Validate a URL against the instance beacon, then register it. The beacon's
/// uuid is the dedupe key: the same deployment reachable two ways is one
/// instance, not two.
///
/// The probe runs as its own task so `cancel_add_instance` can stop it: a
/// cancelled add answers `CANCELLED` and never reaches the registry, even
/// when the instance answers a moment later.
#[tauri::command]
pub async fn add_instance(app: AppHandle, url: String) -> Result<Instance, String> {
    let state = app.state::<ShellState>();
    let origin = beacon::normalize_origin(&url)?;
    let client = state.http.clone();
    let probe_origin = origin.clone();
    let task = tokio::spawn(async move { beacon::probe(&client, &probe_origin).await });
    let seq = state.add_seq.fetch_add(1, Ordering::Relaxed) + 1;
    let previous = state.adding.lock().unwrap().replace(AddAttempt {
        seq,
        abort: task.abort_handle(),
    });
    // One add at a time: a new one supersedes whatever was still probing.
    if let Some(previous) = previous {
        previous.abort.abort();
    }
    let outcome = task.await;

    // Held through registration, so a Cancel either lands before it (and the
    // add answers CANCELLED) or finds nothing left to cancel.
    let mut adding = state.adding.lock().unwrap();
    if !adding.as_ref().is_some_and(|a| a.seq == seq) {
        return Err(CANCELLED.into());
    }
    *adding = None;
    let beacon = match outcome {
        Ok(result) => result?,
        Err(e) if e.is_cancelled() => return Err(CANCELLED.into()),
        Err(e) => return Err(format!("checking the instance: {e}")),
    };
    let base = beacon::origin_string(&origin.url);
    {
        let instances = state.instances.lock().unwrap();
        if let Some(existing) = instances.iter().find(|i| i.instance_id == beacon.instance) {
            return Err(format!(
                "That instance is already added as “{}”",
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
        .unwrap_or_else(|| match (origin.url.host_str(), origin.url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            _ => base.clone(),
        });
    let instance = Instance {
        id: uuid::Uuid::new_v4().to_string(),
        instance_id: beacon.instance,
        label,
        url: base,
        created_at: registry::now_ms(),
        last_opened_at: None,
    };
    state.instances.lock().unwrap().push(instance.clone());
    state.persist()?;
    // The new origin can host the in-UI switcher from the moment it loads.
    crate::grant_switcher(&app, &instance)?;
    drop(adding);
    emit_state(&app);
    Ok(instance)
}

/// Stop the add in flight, if any. True when there was one to stop.
#[tauri::command]
pub fn cancel_add_instance(app: AppHandle) -> bool {
    let state = app.state::<ShellState>();
    let attempt = state.adding.lock().unwrap().take();
    match attempt {
        Some(attempt) => {
            attempt.abort.abort();
            true
        }
        None => false,
    }
}

/// Is this registered instance answering? The launcher asks before it opens
/// an instance that has no webview yet — at boot, and on a cold click — so an
/// unreachable instance is a message with a retry on the launcher, not a
/// blank window or WebKit's bare error page with no way back (in the themed
/// titlebar mode the only chrome is drawn by the page that failed).
#[tauri::command]
pub async fn probe_instance(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<ShellState>();
    let instance = state.find(&id)?;
    let url = instance
        .url
        .parse()
        .map_err(|e| format!("the stored address no longer parses: {e}"))?;
    let origin = beacon::Origin {
        url,
        scheme_assumed: false,
    };
    beacon::probe(&state.http, &origin).await.map(|_| ())
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
    if window.get_webview(&label).is_none() {
        create_instance_webview(&app, &window, &instance)?;
    }
    *state.active.lock().unwrap() = Some(instance.id.clone());
    {
        let mut instances = state.instances.lock().unwrap();
        if let Some(i) = instances.iter_mut().find(|i| i.id == instance.id) {
            i.last_opened_at = Some(registry::now_ms());
        }
    }
    // A failed write must not stop the switch: the instance is already
    // created, and a half-done activate is two visible webviews.
    if let Err(e) = state.persist() {
        eprintln!("talaria-desktop: {e}");
    }
    *state.reopen.lock().unwrap() = None;
    *state.failure.lock().unwrap() = None;
    set_view(&app, View::Active)?;
    emit_state(&app);
    Ok(())
}

fn create_instance_webview(
    app: &AppHandle,
    window: &Window,
    instance: &Instance,
) -> Result<Webview, String> {
    let state = app.state::<ShellState>();
    let label = instance_label(&instance.id);
    // Per-instance data dir = per-instance cookie jar and storage.
    // Cookies ignore ports, so two dev instances on 127.0.0.1:530x
    // would share a session in one shared jar — directories don't.
    let dir = state.data_root.join(&instance.id).join("webview");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let url = instance
        .url
        .parse()
        .map_err(|e| format!("stored URL no longer parses: {e}"))?;
    let rect = layout::creation_rect(window);
    let webview = window
        .add_child(
            WebviewBuilder::new(&label, WebviewUrl::External(url))
                .data_directory(dir)
                .background_color(GROUND)
                .initialization_script(ESCAPE_HATCH),
            rect.position,
            rect.size,
        )
        .map_err(|e| format!("creating the instance webview: {e}"))?;
    #[cfg(target_os = "linux")]
    watch_load_failures(app, &webview, &instance.id);
    Ok(webview)
}

/// WebKitGTK: when an instance's page fails to load (refused, unreachable,
/// timed out, TLS), WebKit shows its own error page — no titlebar in themed
/// mode, no switcher (the instance UI never loaded), and the Ctrl+Shift+H
/// script does not run there. So the shell takes the failure instead: back
/// to the launcher, which says what failed and offers a retry.
///
/// Linux only: this is WebKitGTK's `load-failed` signal. macOS and Windows
/// rely on the launcher's probe before a cold open (`probe_instance`).
#[cfg(target_os = "linux")]
fn watch_load_failures(app: &AppHandle, webview: &Webview, id: &str) {
    let app = app.clone();
    let id = id.to_string();
    let _ = webview.with_webview(move |platform| {
        use webkit2gtk::WebViewExt;
        platform
            .inner()
            .connect_load_failed(move |_view, _event, uri, error| {
                // Only a web page that could not be reached is a failure to act
                // on. Not: a navigation another one replaced, a download, a link
                // WebKit hands elsewhere (mailto:, an app scheme), or content it
                // cannot show inline. For those the answer is "handled", so the
                // page the person is on stays: left to WebKit, a mailto: link
                // replaced the whole instance with "The URL can't be shown".
                let web = uri.starts_with("http://") || uri.starts_with("https://");
                if !web
                    || error.matches(webkit2gtk::NetworkError::Cancelled)
                    || error.matches(webkit2gtk::NetworkError::UnknownProtocol)
                    || error.matches(webkit2gtk::PolicyError::FrameLoadInterruptedByPolicyChange)
                    || error.matches(webkit2gtk::PolicyError::CannotShowMimeType)
                    || error.matches(webkit2gtk::PolicyError::CannotShowUri)
                {
                    return true;
                }
                let app = app.clone();
                let id = id.clone();
                let message = error.message().to_string();
                // Never close a webview from inside its own signal handler:
                // run on the next turn of the main loop.
                webkit2gtk::glib::idle_add_local_once(move || {
                    instance_load_failed(&app, &id, &message);
                });
                true
            });
    });
}

#[cfg(target_os = "linux")]
fn instance_load_failed(app: &AppHandle, id: &str, message: &str) {
    let state = app.state::<ShellState>();
    let Ok(instance) = state.find(id) else {
        return;
    };
    // The failed page holds nothing worth keeping; the next open starts over.
    if let Some(window) = app.get_window("main")
        && let Some(webview) = window.get_webview(&instance_label(id))
    {
        let _ = webview.close();
    }
    let seq = state.failure_seq.fetch_add(1, Ordering::Relaxed) + 1;
    *state.failure.lock().unwrap() = Some(LoadFailure {
        seq,
        id: id.to_string(),
        label: instance.label.clone(),
        message: format!("{} did not load: {message}", instance.label),
    });
    let was_active = state.active.lock().unwrap().as_deref() == Some(id);
    if was_active {
        *state.active.lock().unwrap() = None;
        if let Err(e) = state.persist() {
            eprintln!("talaria-desktop: {e}");
        }
        let _ = set_view(app, View::Welcome);
    }
    emit_state(app);
}

/// Back to the launcher: every instance webview hidden (they stay loaded —
/// sessions and SSE survive), the welcome screen full-window. Reachable from
/// the in-UI switcher ("manage instances") as well as the shell itself.
#[tauri::command]
pub fn show_welcome(app: AppHandle) -> Result<(), String> {
    let state = app.state::<ShellState>();
    *state.active.lock().unwrap() = None;
    *state.reopen.lock().unwrap() = None;
    // A relaunch lands where the person left: here.
    if let Err(e) = state.persist() {
        eprintln!("talaria-desktop: {e}");
    }
    set_view(&app, View::Welcome)?;
    emit_state(&app);
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
    if was_active {
        *state.active.lock().unwrap() = None;
    }
    let registered = {
        let mut instances = state.instances.lock().unwrap();
        let before = instances.len();
        instances.retain(|i| i.id != id);
        instances.len() != before
    };
    // Recorded before the write, so the next launch deletes the dir again if
    // the closing network process re-creates it after the deletes below.
    if registered {
        state.removed.lock().unwrap().push(id.clone());
    }
    {
        let mut failure = state.failure.lock().unwrap();
        if failure.as_ref().is_some_and(|f| f.id == id) {
            *failure = None;
        }
    }
    state.persist()?;
    let dir = state.data_root.join(&id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("deleting {}: {e}", dir.display()))?;
    }
    // The closed webview's WebKit network process can recreate its storage
    // dir while it shuts down, after the delete above. Delete again once it
    // has; the next launch's sweep of recorded removals
    // (registry::sweep_removed) is the backstop.
    tauri::async_runtime::spawn(async move {
        for wait in [2, 8] {
            tokio::time::sleep(Duration::from_secs(wait)).await;
            if dir.exists() {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    });
    if was_active {
        set_view(&app, View::Welcome)?;
    }
    emit_state(&app);
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
    // The decoration change moves the client area. Re-place the visible
    // webview now instead of a frame later on the next Resized, which showed
    // a band of bare window for a frame or two (the Resized pass still runs).
    layout::relayout(&window);
    emit_state(&app);
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

/// An updater failure in words: a timeout says so, a transport error says
/// which (beacon::classify), and the rest keep the plugin's own text.
fn update_failure(e: tauri_plugin_updater::Error) -> String {
    match e {
        tauri_plugin_updater::Error::Reqwest(e) => {
            let host = e
                .url()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| "the update server".into());
            if e.is_timeout() {
                format!(
                    "Could not check for updates: {host} did not answer within {} seconds",
                    UPDATE_CHECK_TIMEOUT.as_secs()
                )
            } else {
                let url = e
                    .url()
                    .cloned()
                    .unwrap_or_else(|| "https://github.com".parse().unwrap());
                format!(
                    "Could not check for updates: {}",
                    beacon::classify(&e).reason(&url)
                )
            }
        }
        other => format!("Could not check for updates: {other}"),
    }
}

/// Compare this build to GitHub's latest stable `latest.json`. None means
/// this version is current (or the endpoint had nothing newer).
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app
        .updater_builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .build()
        .map_err(|e| format!("updater: {e}"))?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(UpdateInfo {
            version: update.version,
            notes: update.body,
        })),
        Ok(None) => Ok(None),
        Err(e) => Err(update_failure(e)),
    }
}

/// Download the latest signed payload, apply it, and relaunch. The updater
/// verifies the minisign signature against the pubkey in tauri.conf.json
/// before touching the install. No short bound here: the download is the
/// whole installer, and the plugin applies one timeout to check and download.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| format!("updater: {e}"))?;
    let Some(update) = updater.check().await.map_err(update_failure)? else {
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
///
/// Async, and the post itself on a blocking thread: notify-rust's `show()` is
/// a blocking D-Bus call, and as a sync command it ran on the main thread — a
/// notification server that took 30 s to answer froze every webview and the
/// window for those 30 s. The page gets its answer within `NOTIFY_WAIT`.
#[tauri::command]
pub async fn desktop_notify(
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
    let posting = tauri::async_runtime::spawn_blocking(move || {
        let on_open = {
            let app = app.clone();
            move || reveal_notice(&app, &label, href.as_deref())
        };
        notify::post(&app, &title, body.as_deref(), tag.as_deref(), on_open)
    });
    match tokio::time::timeout(NOTIFY_WAIT, posting).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(format!("posting the notification: {e}")),
        Err(_) => {
            eprintln!(
                "talaria-desktop: the notification service has not answered in {} s; the post continues",
                NOTIFY_WAIT.as_secs()
            );
            Ok(())
        }
    }
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
            // NEVER `show()` a window that is already visible. On Linux tao's
            // show is gtk `show_all`, which re-shows every hidden child
            // webview — and wry's vertical GtkBox then gives the hidden
            // launcher and every hidden instance an equal slice of the window
            // (the instance came up at half height under a blank band). Only
            // a window that really is hidden is shown, and the view is put
            // back straight after.
            if !window.is_visible().unwrap_or(true) {
                let _ = window.show();
                let view = *app.state::<ShellState>().view.lock().unwrap();
                let _ = set_view(&app, view);
            }
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
