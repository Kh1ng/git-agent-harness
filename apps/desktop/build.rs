fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "desktop_settings",
            "save_presence",
            "connect_dashboard",
            "open_central_settings",
            "node_role_status",
            "set_node_role",
            "worker_status",
            "set_worker_running",
        ]),
    ))
    .expect("failed to build desktop permissions");
}
