// Windows 发布版不弹控制台窗口；dev 版保留 stdout 供验收断言窗口日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nowhere_desktop::run();
}
