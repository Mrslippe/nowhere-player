//! Tauri 壳：插件装配 + 启动编排，不放业务逻辑（见开发方案 §4.2）。

mod shortcut;
mod tray;
mod windows;

/// 启动桌面壳：先注册插件，再建窗、注册快捷键、装托盘。
pub fn run() {
    tauri::Builder::default()
        // 单实例必须第一个注册，否则不生效（T-0009 §11）。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            println!("[single-instance] secondary launch, showing player");
            windows::show_player(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(shortcut::plugin())
        .setup(|app| {
            let handle = app.handle();
            windows::create_player(handle)?;
            shortcut::register(handle);
            tray::create(handle)?;
            Ok(())
        })
        .on_window_event(windows::on_window_event)
        .run(tauri::generate_context!())
        .expect("failed to run Nowhere Player");
}
