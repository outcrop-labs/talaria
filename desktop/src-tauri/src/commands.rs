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

use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, EventTarget, Manager, Webview, WebviewBuilder, WebviewUrl, Window,
    window::Color,
};
use tauri_plugin_updater::UpdaterExt;

use crate::{
    AddAttempt, LoadFailure, ShellState, Suspended, View, beacon, layout, notify, registry,
    registry::Instance, settings, suspend, watch,
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
    /// 0 is Off (settings.rs).
    pub suspend_after_minutes: u32,
    pub instances: Vec<Instance>,
    /// Instances with a live webview this session: opening one is instant.
    pub loaded: Vec<String>,
    /// Instances this session suspended (suspend.rs): opening one recreates
    /// its webview at its last page, with no probe.
    pub suspended: Vec<String>,
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
    let (titlebar, suspend_after_minutes) = {
        let settings = state.settings.lock().unwrap();
        (settings.titlebar, settings.suspend_after_minutes)
    };
    let instances = state.instances.lock().unwrap().clone();
    let notices = state.notices.lock().unwrap().clone();
    let reopen = state.reopen.lock().unwrap().clone();
    let failure = state.failure.lock().unwrap().clone();
    let mut suspended: Vec<String> = state.suspended.lock().unwrap().keys().cloned().collect();
    suspended.sort();
    ShellSnapshot {
        view,
        active_id,
        titlebar,
        suspend_after_minutes,
        instances,
        loaded,
        suspended,
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
///
/// The page leaving the screen has its URL remembered, and starts its
/// suspend clock (suspend.rs); the one coming on screen stops its own.
fn set_view(app: &AppHandle, view: View) -> Result<(), String> {
    let state = app.state::<ShellState>();
    let window = main_window(app)?;
    *state.view.lock().unwrap() = view;
    let active = state.active.lock().unwrap().clone();
    let shown = match (view, &active) {
        (View::Active, Some(id)) => Some(id.clone()),
        _ => None,
    };
    let leaving = state
        .activity
        .lock()
        .unwrap()
        .shown()
        .filter(|id| Some(*id) != shown.as_deref())
        .map(str::to_string);
    if let Some(id) = leaving
        && let Some(webview) = window.get_webview(&instance_label(&id))
        && remember_page(app, &id, &webview)
        && let Err(e) = state.persist()
    {
        eprintln!("talaria-desktop: {e}");
    }
    state
        .activity
        .lock()
        .unwrap()
        .show(shown.as_deref(), Instant::now());
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
    #[cfg(target_os = "linux")]
    set_header_title(&window, &title);
    Ok(())
}

/// GTK on Wayland: the OS titlebar is tao's own `GtkHeaderBar`, built with the
/// window's first title and wrapped in a `GtkEventBox`. GTK passes a new window
/// title on to a header bar only when the header bar IS the titlebar, so
/// through the wrapper `set_title` never reached it and the bar said
/// "Talaria" for every instance. Set it directly. On X11 the window manager
/// draws the title from the window's own, which `set_title` already set; there
/// is no header bar to find and this does nothing.
#[cfg(target_os = "linux")]
fn set_header_title(window: &Window, title: &str) {
    use gtk::prelude::*;
    let Ok(gtk_window) = window.gtk_window() else {
        return;
    };
    let Some(bar) = gtk_window.titlebar() else {
        return;
    };
    let header = match bar.clone().downcast::<gtk::HeaderBar>() {
        Ok(header) => Some(header),
        Err(bar) => bar
            .downcast::<gtk::Bin>()
            .ok()
            .and_then(|b| b.child())
            .and_then(|c| c.downcast::<gtk::HeaderBar>().ok()),
    };
    if let Some(header) = header {
        header.set_title(Some(title));
    }
}

/// Remember the page `webview` shows as `id`'s last page. True when the record
/// changed (the caller persists).
///
/// A page on the instance's own origin is remembered, unless it is a sign-in
/// or callback page (suspend::restorable_url), which clears the record so a
/// restore opens the root. A page on another origin (a link that left the
/// instance) changes nothing: the last page of the instance itself stands.
fn remember_page(app: &AppHandle, id: &str, webview: &Webview) -> bool {
    let state = app.state::<ShellState>();
    let Ok(instance) = state.find(id) else {
        return false;
    };
    let Ok(url) = webview.url() else {
        return false;
    };
    remember_url(&state, &instance, url.as_str())
}

fn remember_url(state: &ShellState, instance: &Instance, raw: &str) -> bool {
    let same_origin = match (url::Url::parse(raw), url::Url::parse(&instance.url)) {
        (Ok(page), Ok(base)) => page.origin() == base.origin(),
        _ => false,
    };
    if !same_origin {
        return false;
    }
    let page = suspend::restorable_url(&instance.url, raw);
    let mut urls = state.last_urls.lock().unwrap();
    match page {
        Some(page) if urls.get(&instance.id) != Some(&page) => {
            urls.insert(instance.id.clone(), page);
            true
        }
        None => urls.remove(&instance.id).is_some(),
        Some(_) => false,
    }
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

/// Show an instance: create its webview if it has none — the first open, a
/// relaunch, or a suspended one coming back — born at its final rect
/// (WebviewBuilder cannot create hidden, and WebKitGTK mis-places new
/// children until a set_bounds, tauri#10420), then hand the whole window to
/// it. A new webview opens at the instance's last page (suspend.rs).
#[tauri::command]
pub fn activate_instance(app: AppHandle, id: String) -> Result<(), String> {
    activate(&app, &id, None).map(|_| ())
}

/// `activate_instance`, plus a notification's `href`: a webview created for
/// it opens there instead of at the last page. True when it did, so the
/// caller does not navigate it a second time.
fn activate(app: &AppHandle, id: &str, href: Option<&str>) -> Result<bool, String> {
    let state = app.state::<ShellState>();
    let instance = state.find(id)?;
    let window = main_window(app)?;
    let label = instance_label(&instance.id);

    for other in window.webviews() {
        if other.label().starts_with("instance-") && other.label() != label {
            let _ = other.hide();
        }
    }
    let mut born_at_href = false;
    if window.get_webview(&label).is_none() {
        // Restoring: the load starts here, on the first signal. No probe —
        // a suspended instance answered a moment ago, and a page that fails
        // now comes back to the launcher as a load failure (Linux).
        let resumed = state.suspended.lock().unwrap().remove(&instance.id);
        if let Some(watcher) = resumed.as_ref().and_then(|r| r.watcher.as_ref()) {
            watcher.abort();
        }
        let page = state
            .last_urls
            .lock()
            .unwrap()
            .get(&instance.id)
            .and_then(|u| suspend::restorable_url(&instance.url, u));
        let (target, scroll) = match href.and_then(notify::openable_href) {
            Some(href) => {
                born_at_href = true;
                (format!("{}{href}", instance.url), None)
            }
            None => {
                let scroll = resumed.as_ref().zip(page.as_deref()).and_then(|(r, page)| {
                    (r.page.as_deref() == Some(page))
                        .then(|| {
                            suspend::restore_js(
                                page,
                                &r.scroll,
                                &r.session,
                                registry::now_ms() as u64,
                            )
                        })
                        .flatten()
                });
                (page.unwrap_or_else(|| instance.url.clone()), scroll)
            }
        };
        if resumed.is_some() {
            eprintln!(
                "talaria-desktop: restoring {label} (t={} ms)",
                registry::now_ms()
            );
        }
        create_instance_webview(app, &window, &instance, &target, scroll)?;
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
    set_view(app, View::Active)?;
    emit_state(app);
    Ok(born_at_href)
}

fn create_instance_webview(
    app: &AppHandle,
    window: &Window,
    instance: &Instance,
    target: &str,
    restore: Option<String>,
) -> Result<Webview, String> {
    let state = app.state::<ShellState>();
    let label = instance_label(&instance.id);
    // Per-instance data dir = per-instance cookie jar and storage.
    // Cookies ignore ports, so two dev instances on 127.0.0.1:530x
    // would share a session in one shared jar — directories don't.
    let dir = state.data_root.join(&instance.id).join("webview");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let url = target
        .parse()
        .or_else(|_| instance.url.parse())
        .map_err(|e| format!("stored URL no longer parses: {e}"))?;
    let rect = layout::creation_rect(window);
    let mut builder = WebviewBuilder::new(&label, WebviewUrl::External(url))
        .data_directory(dir)
        .background_color(GROUND)
        .initialization_script(ESCAPE_HATCH);
    if let Some(script) = restore {
        builder = builder.initialization_script(script);
    }
    let webview = window
        .add_child(builder, rect.position, rect.size)
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
    state.activity.lock().unwrap().forget(id);
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
    if let Some(watcher) = state
        .suspended
        .lock()
        .unwrap()
        .remove(&id)
        .and_then(|s| s.watcher)
    {
        watcher.abort();
    }
    state.last_urls.lock().unwrap().remove(&id);
    state.activity.lock().unwrap().forget(&id);
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

/// How long an instance's page may stay off screen before it is suspended,
/// in minutes; 0 is Off. The launcher and Settings → Profile both set it.
#[tauri::command]
pub fn set_suspend_after(
    app: AppHandle,
    minutes: u32,
) -> Result<settings::DesktopSettings, String> {
    if minutes > settings::MAX_SUSPEND_MINUTES {
        return Err(format!(
            "the longest suspend timeout is {} minutes",
            settings::MAX_SUSPEND_MINUTES
        ));
    }
    let state = app.state::<ShellState>();
    let saved = {
        let mut current = state.settings.lock().unwrap();
        current.suspend_after_minutes = minutes;
        settings::save(&state.settings_path, &current)?;
        current.clone()
    };
    emit_state(&app);
    Ok(saved)
}

/// The instance webviews that exist now, by local id.
fn live_instances(window: &Window) -> Vec<(String, Webview)> {
    window
        .webviews()
        .into_iter()
        .filter_map(|w| {
            w.label()
                .strip_prefix("instance-")
                .map(str::to_string)
                .map(|id| (id, w))
        })
        .collect()
}

/// Minimized or hidden — not merely unfocused.
///
/// The shell's own minimize counts until the window is focused again (GTK on
/// Wayland reports no minimized state). A window found focused is not
/// minimized, which clears it here, on a focus event, and once just after the
/// minimize (`MINIMIZE_SETTLE`), so a compositor that ignored the request
/// (sway and other wlroots window managers) does not leave it set.
fn window_away(app: &AppHandle, window: &Window) -> bool {
    let state = app.state::<ShellState>();
    if state.self_minimized.load(Ordering::Relaxed) && window.is_focused().unwrap_or(false) {
        state.self_minimized.store(false, Ordering::Relaxed);
    }
    !window.is_visible().unwrap_or(true)
        || window.is_minimized().unwrap_or(false)
        || state.self_minimized.load(Ordering::Relaxed)
}

/// The instance on screen was suspended while the window was away, and the
/// window is back: recreate it at its page, now.
fn restore_shown(app: &AppHandle, window: &Window) {
    let state = app.state::<ShellState>();
    let view = *state.view.lock().unwrap();
    let Some(id) = state.active.lock().unwrap().clone() else {
        return;
    };
    if view != View::Active || window.get_webview(&instance_label(&id)).is_some() {
        return;
    }
    let suspended = state.suspended.lock().unwrap().contains_key(&id);
    if suspended && let Err(e) = activate(app, &id, None) {
        eprintln!("talaria-desktop: restoring {id}: {e}");
    }
}

/// The window was focused, restored or resized: if it is on screen again and
/// its instance was suspended meanwhile, bring that instance back at once
/// rather than on the next tick.
pub fn window_returned(app: &AppHandle, focused: bool) {
    let state = app.state::<ShellState>();
    if focused {
        state.self_minimized.store(false, Ordering::Relaxed);
    }
    let Ok(window) = main_window(app) else {
        return;
    };
    let away = window_away(app, &window);
    state.activity.lock().unwrap().window(away, Instant::now());
    if !away {
        restore_shown(app, &window);
    }
}

/// Remember every live instance's page now (the window is closing).
pub fn remember_all(app: &AppHandle) {
    let Ok(window) = main_window(app) else {
        return;
    };
    let mut changed = false;
    for (id, webview) in live_instances(&window) {
        changed |= remember_page(app, &id, &webview);
    }
    if changed && let Err(e) = app.state::<ShellState>().persist() {
        eprintln!("talaria-desktop: {e}");
    }
}

/// The suspend clock's tick (suspend::TICK), on the main thread: note what
/// the window is doing, keep each live page's URL current, and ask every page
/// that has been off screen for the timeout whether it can go.
pub fn suspend_tick(app: &AppHandle) {
    let state = app.state::<ShellState>();
    let Ok(window) = main_window(app) else {
        return;
    };
    let now = Instant::now();
    let away = window_away(app, &window);
    state.activity.lock().unwrap().window(away, now);
    if !away {
        restore_shown(app, &window);
    }
    let live = live_instances(&window);
    let mut changed = false;
    for (id, webview) in &live {
        changed |= remember_page(app, id, webview);
    }
    if changed && let Err(e) = state.persist() {
        eprintln!("talaria-desktop: {e}");
    }
    let Some(timeout) = state.settings.lock().unwrap().suspend_after() else {
        return;
    };
    let ids: Vec<String> = live.iter().map(|(id, _)| id.clone()).collect();
    let due = state.activity.lock().unwrap().due(&ids, now, timeout);
    for id in due {
        let nonce = state.activity.lock().unwrap().ask(&id, now);
        if let Some((_, webview)) = live.iter().find(|(i, _)| *i == id) {
            let _ = webview.eval(suspend::report_js(nonce));
        }
    }
}

/// A page's answer to `suspend::report_js`. Only an instance webview may call
/// it, and only about itself; an answer nobody asked for is ignored.
#[tauri::command]
pub fn desktop_suspend_report(
    app: AppHandle,
    webview: Webview,
    report: suspend::Report,
) -> Result<(), String> {
    let Some(id) = webview
        .label()
        .strip_prefix("instance-")
        .map(str::to_string)
    else {
        return Err("only an instance page reports".into());
    };
    let state = app.state::<ShellState>();
    let instance = state.find(&id)?;
    if !state.activity.lock().unwrap().answered(&id, report.nonce) {
        return Ok(());
    }
    let Some(timeout) = state.settings.lock().unwrap().suspend_after() else {
        return Ok(());
    };
    if !state
        .activity
        .lock()
        .unwrap()
        .is_due(&id, Instant::now(), timeout)
    {
        return Ok(());
    }
    let page = match report.page {
        Some(page) if page.version >= 1 => page,
        _ => {
            eprintln!(
                "talaria-desktop: keeping {}: its Talaria predates suspend support",
                webview.label()
            );
            state
                .activity
                .lock()
                .unwrap()
                .keep(&id, suspend::Kept::Unsupported);
            return Ok(());
        }
    };
    if page.holds_input {
        eprintln!(
            "talaria-desktop: keeping {}: it holds input a reload would lose",
            webview.label()
        );
        state
            .activity
            .lock()
            .unwrap()
            .keep(&id, suspend::Kept::Input);
        return Ok(());
    }
    let scroll = suspend::clean_scrolls(&report.scroll);
    let session = suspend::clean_session(&report.session);
    let nonce = report.nonce;
    // From here until finish_suspend (or keep_for_notify) the page is closing:
    // the ticks meanwhile do not ask it again, which would start a second
    // cookie read, a second baseline and a second watcher.
    state
        .activity
        .lock()
        .unwrap()
        .closing(&id, nonce, Instant::now());
    let app = app.clone();
    // Off the main thread: the webview's cookie store answers through the main
    // loop (and on Windows deadlocks a sync command that waits for it), and
    // the notifications baseline is a network read. The page stays open until
    // both are done; the close itself is back on the main thread.
    tauri::async_runtime::spawn(async move {
        let watch = if page.notify {
            match read_cookie(webview.clone(), &instance.url).await {
                // The store answered with no cookie: signed out, and the page
                // is not fetching notifications either.
                Ok(None) => None,
                Ok(Some(cookie)) => match watch::baseline(&instance.url, &cookie).await {
                    watch::Baseline::Ready(arrivals) => Some((cookie, arrivals)),
                    watch::Baseline::SignedOut => None,
                    watch::Baseline::Unreachable => {
                        keep_for_notify(
                            &app,
                            &instance.id,
                            "the shell could not read its notifications the way the page does",
                        );
                        return;
                    }
                },
                Err(e) => {
                    keep_for_notify(&app, &instance.id, &e);
                    return;
                }
            }
        } else {
            None
        };
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            finish_suspend(
                &handle,
                &instance,
                nonce,
                &report.url,
                scroll,
                session,
                watch,
            )
        });
    });
    Ok(())
}

/// A page whose notifications the shell cannot carry stays open until it is
/// shown again.
fn keep_for_notify(app: &AppHandle, id: &str, why: &str) {
    eprintln!("talaria-desktop: keeping instance-{id}: {why}");
    app.state::<ShellState>()
        .activity
        .lock()
        .unwrap()
        .keep(id, suspend::Kept::Notify);
}

/// How long the webview's cookie store may take to answer before the page is
/// kept instead (it answers through the main loop, which a busy GTK can hold).
const COOKIE_WAIT: Duration = Duration::from_secs(10);

/// The `Cookie` header `webview` would send to `origin`, read from its own
/// store. `Ok(None)`: it holds none.
async fn read_cookie(webview: Webview, origin: &str) -> Result<Option<String>, String> {
    let url: tauri::Url = origin
        .parse()
        .map_err(|e| format!("the stored address no longer parses: {e}"))?;
    let read = tauri::async_runtime::spawn_blocking(move || webview.cookies_for_url(url));
    let cookies = tokio::time::timeout(COOKIE_WAIT, read)
        .await
        .map_err(|_| {
            format!(
                "its cookie store did not answer in {} s",
                COOKIE_WAIT.as_secs()
            )
        })?
        .map_err(|e| format!("reading its cookies: {e}"))?
        .map_err(|e| format!("reading its cookies: {e}"))?;
    Ok(watch::cookie_header(
        cookies.iter().map(|c| (c.name(), c.value())),
    ))
}

fn finish_suspend(
    app: &AppHandle,
    instance: &Instance,
    nonce: u64,
    reported_url: &str,
    scroll: Vec<suspend::Scroll>,
    session: Vec<(String, String)>,
    watch: Option<(String, watch::Arrivals)>,
) {
    let state = app.state::<ShellState>();
    let id = instance.id.clone();
    let label = instance_label(&id);
    // The close this answer started ends here, whichever way: everything below
    // runs on the main thread, as the tick does, so no ask can come between.
    state.activity.lock().unwrap().closed(&id, nonce);
    // Still due? The person may have come back while the cookies were read.
    let Some(timeout) = state.settings.lock().unwrap().suspend_after() else {
        return;
    };
    if !state
        .activity
        .lock()
        .unwrap()
        .is_due(&id, Instant::now(), timeout)
    {
        return;
    }
    let Ok(window) = main_window(app) else {
        return;
    };
    let Some(webview) = window.get_webview(&label) else {
        return;
    };
    remember_url(&state, instance, reported_url);
    let page = state.last_urls.lock().unwrap().get(&id).cloned();
    if let Err(e) = webview.close() {
        eprintln!("talaria-desktop: suspending {label}: {e}");
        return;
    }
    let watcher = watch.map(|(cookie, arrivals)| {
        watch::spawn(
            app.clone(),
            label.clone(),
            instance.url.clone(),
            cookie,
            arrivals,
        )
    });
    eprintln!(
        "talaria-desktop: suspended {label}{} (t={} ms)",
        if watcher.is_some() {
            ", notifications watched"
        } else {
            ""
        },
        registry::now_ms()
    );
    let replaced = state.suspended.lock().unwrap().insert(
        id,
        Suspended {
            page,
            scroll,
            session,
            watcher,
        },
    );
    // Never two watchers for one instance: one replaced here would post every
    // banner twice, and keep posting after the restore.
    if let Some(old) = replaced.and_then(|s| s.watcher) {
        old.abort();
    }
    if let Err(e) = state.persist() {
        eprintln!("talaria-desktop: {e}");
    }
    emit_state(app);
}

/// How long after the shell's own minimize it looks whether the compositor
/// honoured it: a minimized window has lost focus by then.
const MINIMIZE_SETTLE: Duration = Duration::from_millis(750);

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
        WindowAction::Minimize => {
            // GTK on Wayland never reports a minimized window (the protocol
            // has no such state for the client), so the suspend clock needs
            // the shell's own word for the minimize it did. The next focus
            // clears it.
            app.state::<ShellState>()
                .self_minimized
                .store(true, Ordering::Relaxed);
            window.minimize().map_err(|e| format!("minimizing: {e}"))?;
            // A compositor may ignore the request and leave the window where
            // it is, focused. Look once, a moment later: still focused means
            // not minimized. Otherwise the word would outlive the person
            // clicking into another app, and the instance on screen would be
            // suspended while visible.
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(MINIMIZE_SETTLE).await;
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if main_window(&handle).is_ok_and(|w| w.is_focused().unwrap_or(false)) {
                        handle
                            .state::<ShellState>()
                            .self_minimized
                            .store(false, Ordering::Relaxed);
                    }
                });
            });
        }
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

/// A notification click: bring `label`'s instance forward, open `href` in it,
/// and put the window in front. A suspended instance is recreated straight at
/// `href`: the notification's own page wins over the page it was on.
pub(crate) fn reveal_notice(app: &AppHandle, label: &str, href: Option<&str>) {
    let label = label.to_string();
    let href = href.map(str::to_string);
    let app = app.clone();
    // WebKitGTK eval and window ops belong on the main thread. The click
    // waiter is a blocking thread; this only queues the reveal.
    let _ = app.clone().run_on_main_thread(move || {
        let mut born_at_href = false;
        if let Some(id) = label.strip_prefix("instance-") {
            born_at_href = activate(&app, id, href.as_deref()).unwrap_or(false);
        }
        if let Some(window) = app.get_window("main") {
            let _ = window.unminimize();
            // The shell asked for the window back. On Wayland that brings no
            // focus event without an activation token, so the shell's own
            // minimize would keep counting the instance on screen as away
            // and suspend it while it is visible. Never risk that: the
            // window is back as far as the suspend clock is concerned.
            app.state::<ShellState>()
                .self_minimized
                .store(false, Ordering::Relaxed);
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
        if !born_at_href
            && let Some(href) = href
            && let Some(js) = notify::assign_js(&href)
            && let Some(webview) = app.get_webview(&label)
        {
            let _ = webview.eval(js);
        }
    });
}
