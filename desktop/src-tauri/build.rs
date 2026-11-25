fn main() {
    // The drag-image shim (src/drag_icon.rs, main.rs): libwebkit2gtk must find
    // the executable's gtk_drag_set_icon_surface. GNU ld and lld already export
    // a definition that a shared library on the link line references, and
    // libwebkit2gtk-4.1 is on it; this names the symbol explicitly, in case
    // the link ever changes. desktop/packaging/linux/check-drag-icon-shim.sh
    // is the proof, in CI. Linux targets only: nothing else has the symbol.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bins=-Wl,--export-dynamic-symbol=gtk_drag_set_icon_surface");
    }

    // AppManifest::commands gates every shell command behind a capability:
    // without this, commands registered in invoke_handler are callable from
    // EVERY webview — including remote instance webviews. With it, only the
    // local launcher webview (capabilities/launcher.json) can invoke.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "list_instances",
            "get_shell_state",
            "add_instance",
            "cancel_add_instance",
            "probe_instance",
            "activate_instance",
            "show_welcome",
            "close_instance_page",
            "show_instance_page",
            "remove_instance",
            "get_desktop_settings",
            "set_titlebar_mode",
            "set_suspend_after",
            "desktop_suspend_report",
            "desktop_window",
            "check_for_update",
            "install_update",
            "snooze_update",
            "desktop_notify",
            "desktop_notification_status",
            "desktop_notification_request",
            "desktop_notification_test",
            "desktop_set_badge",
            "desktop_quit",
            "set_close_to_background",
            "desktop_notify_close",
            "desktop_os_idle",
            "set_instance_muted",
            "desktop_reload",
            "desktop_open_link",
        ]),
    ))
    .expect("failed to run tauri-build");
}
