//! 开机自启：写 HKCU Run；配置键 `startup.enabled`。

use std::process::Command;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
fn value() -> String {
    std::env::var("NP_TUI_STARTUP_KEY").unwrap_or_else(|_| "NowherePlayer".to_owned())
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let program = format!("\"{}\"", exe.display());
    let value = value();
    let mut command = Command::new("reg.exe");
    if enabled {
        command.args([
            "add", RUN_KEY, "/v", &value, "/t", "REG_SZ", "/d", &program, "/f",
        ]);
    } else {
        command.args(["delete", RUN_KEY, "/v", &value, "/f"]);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}
