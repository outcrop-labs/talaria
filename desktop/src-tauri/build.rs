fn main() {
    // AppManifest::commands gates every shell command behind a capability:
    // without this, commands registered in invoke_handler are callable from
    // EVERY webview — including remote instance webviews. With it, only the
    // local launcher webview (capabilities/launcher.json) can invoke.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "list_instances",
            "add_instance",
            "activate_instance",
            "show_welcome",
            "remove_instance",
        ]),
    ))
    .expect("failed to run tauri-build");
}
