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

/// WebKitGTK's call to GTK's `gtk_drag_set_icon_surface` lands here first:
/// the executable exports this definition (build.rs), and the loader looks in
/// the executable before libgtk-3. WebKitGTK hands over a device-pixel image
/// that GTK would show at the display scale times the web size;
/// drag_icon.rs fits it, then calls GTK's own function with the same
/// arguments (docs/DESKTOP.md, "Drag images on a HiDPI Linux display").
///
/// # Safety
/// GTK's contract for the function it stands in for: called on the GTK main
/// thread with the source side's live drag context and a live cairo surface.
// webview-threading: callback
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gtk_drag_set_icon_surface(
    context: *mut talaria_desktop::drag_icon::Context,
    surface: *mut talaria_desktop::drag_icon::Surface,
) {
    unsafe { talaria_desktop::drag_icon::set_icon_surface(context, surface) }
}
