//! Tauri 壳：插件装配 + 启动编排，不放业务逻辑（见开发方案 §4.2）。

mod command_exec;
mod ipc;
mod player_actor;
mod shortcut;
mod tray;
mod windows;

use tauri::Manager;

/// 启动桌面壳：先注册插件，再建窗、注册快捷键、装托盘。
pub fn run() {
    let app = tauri::Builder::default()
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
        .invoke_handler(tauri::generate_handler![
            ipc::player_snapshot,
            ipc::player_play,
            ipc::player_pause,
            ipc::player_next,
            ipc::player_prev,
            ipc::player_seek,
            ipc::player_volume,
            ipc::player_mode,
            ipc::queue_list,
            ipc::queue_add,
            ipc::queue_remove,
            ipc::queue_clear,
            ipc::queue_jump,
            ipc::library_add_root,
            ipc::library_scan,
            ipc::library_list,
            ipc::library_search,
            ipc::library_stats,
            ipc::library_cover_url,
            ipc::library_track_by_path,
            ipc::theme_list,
            ipc::theme_apply,
            ipc::theme_export_builtin,
            command_exec::cmd_exec,
            command_exec::cmd_complete,
        ])
        .setup(|app| {
            // actor 线程在此启动；`Player` 由线程内部惰性创建。
            app.manage(player_actor::PlayerHandle::spawn(app.handle().clone())?);
            // 媒体库连接与扫描任务延迟到首次使用时打开；这里只记路径。
            let db_path = app.path().app_data_dir()?.join("library.db");
            app.manage(command_exec::LibraryHandle::new(db_path));
            let handle = app.handle();
            windows::create_player(handle)?;
            shortcut::register(handle);
            tray::create(handle)?;
            Ok(())
        })
        .on_window_event(windows::on_window_event)
        .build(tauri::generate_context!())
        .expect("failed to build Nowhere Player");

    app.run(|app, event| {
        // 退出前让 actor 在自己线程 drop `Player`，释放音频设备（T-0013 §7）；
        // 同时打断媒体库扫描线程，让它提交已入库部分。
        if let tauri::RunEvent::Exit = event {
            app.state::<command_exec::LibraryHandle>().shutdown();
            app.state::<player_actor::PlayerHandle>().shutdown();
        }
    });
}
