use crate::model::ViewData;
use np_app::SettingsHandle;

pub(crate) fn settings_view(settings: &SettingsHandle) -> ViewData {
    let known = [
        "playback.volume",
        "playback.mode",
        "theme.id",
        "ui.splash",
        "tui.focusMode",
        "startup.enabled",
        "source.bilibili.enabled",
        "source.netease.enabled",
    ];
    let mut rows = Vec::new();
    for (category, keys) in [
        ("播放", &["playback.volume", "playback.mode"][..]),
        ("界面", &["theme.id", "ui.splash", "tui.focusMode"][..]),
        ("启动", &["startup.enabled"][..]),
        (
            "音源",
            &["source.bilibili.enabled", "source.netease.enabled"][..],
        ),
    ] {
        for key in keys {
            rows.push(vec![
                category.to_owned(),
                (*key).to_owned(),
                settings_value_text(settings, key),
            ]);
        }
    }
    for (key, value) in settings.entries() {
        if known.contains(&key.as_str()) {
            continue;
        }
        rows.push(vec!["其他".to_owned(), key, value.display()]);
    }
    ViewData {
        title: "设置".to_owned(),
        columns: vec!["分类".to_owned(), "设置项".to_owned(), "值".to_owned()],
        rows,
        refs: Vec::new(),
        durations: Vec::new(),
    }
}

fn settings_value_text(settings: &SettingsHandle, key: &str) -> String {
    let value = settings.get(key);
    let default = match key {
        "playback.volume" => "100%".to_owned(),
        "playback.mode" => "顺序播放".to_owned(),
        "theme.id" => "milkgreen".to_owned(),
        "ui.splash" => "开".to_owned(),
        "tui.focusMode" => "关".to_owned(),
        "startup.enabled" => "关".to_owned(),
        "source.bilibili.enabled" | "source.netease.enabled" => "开".to_owned(),
        _ => "未设置".to_owned(),
    };
    match value {
        None => default,
        Some(np_core::SettingValue::Bool(enabled)) => {
            if enabled {
                "开".to_owned()
            } else {
                "关".to_owned()
            }
        }
        Some(np_core::SettingValue::Num(number)) => {
            if key == "playback.volume" {
                format!("{}%", (number * 100.0).round() as i64)
            } else {
                number.to_string()
            }
        }
        Some(np_core::SettingValue::Str(text)) => {
            if key == "playback.mode" {
                playback_mode_name(&text)
            } else {
                text
            }
        }
    }
}

fn playback_mode_name(id: &str) -> String {
    match id {
        "single" => "单曲循环",
        "shuffle" => "随机播放",
        "list" => "列表循环",
        "sequential" => "顺序播放",
        other => other,
    }
    .to_owned()
}
