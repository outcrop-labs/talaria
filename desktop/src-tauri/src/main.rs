// No extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK's DMABUF renderer aborts on a Wayland protocol error inside
    // the Flatpak sandbox — the runtime's WebKit and the host render node
    // disagree, and a .desktop launch (Terminal=false) shows nothing.
    // Native packages keep the fast path. A manifest or user override wins.
    #[cfg(target_os = "linux")]
    if std::env::var_os("FLATPAK_ID").is_some()
        && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
    {
        // SAFETY: called before any GTK/WebKit init, and only when the
        // variable is unset, so this cannot race another writer.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    talaria_desktop::run()
}
