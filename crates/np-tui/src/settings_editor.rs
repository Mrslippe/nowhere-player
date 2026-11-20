//! 设置页编辑浮层：选择态、草稿、提交/取消与类型调整。
//!
//! `SettingsEditor` 只保存编辑状态；EventLoop/Controller 通过
//! [`SettingsOutcome`] 接线，真正的设置落盘与播放器联动仍由 App 完成。
//! 键位、文案、TTL 与 T-0098 完全一致。

use crossterm::event::KeyCode;

use crate::app::App;
use crate::audio::TuiAudio;
use crate::model::{VisualMode, playback_mode_symbol};
use crate::settings_page::{
    SettingsField, SettingsKind, bool_text, default_value, live_buffer_ms, set_row_value,
};
use crate::sound::SoundCue;
use crate::startup;
use np_app::{SettingsHandle, ThemeApi};
use np_core::SettingValue;

/// 编辑态：值只存在 `draft` 里，Enter 才写回设置与播放器。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SettingsEdit {
    pub(crate) index: usize,
    pub(crate) draft: SettingValue,
}

/// 设置页浮层状态：当前选择行与可选编辑草稿。
#[derive(Debug, Default)]
pub(crate) struct SettingsEditor {
    selected: usize,
    edit: Option<SettingsEdit>,
}

/// `handle_key` 的结果；EventLoop/Controller 只按此接线。
#[derive(Debug, Clone)]
pub(crate) enum SettingsOutcome {
    Handled,
    Submit(SettingsField, SettingValue),
    Cancel,
}

impl SettingsEditor {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn select(&mut self, index: usize) {
        self.selected = index;
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// 清空编辑态；视图刷新由调用方负责。
    pub(crate) fn clear_edit(&mut self) -> bool {
        self.edit.take().is_some()
    }

    fn start_edit(&mut self, draft: SettingValue) {
        self.edit = Some(SettingsEdit {
            index: self.selected,
            draft,
        });
    }

    fn selected_field<'a>(&self, fields: &'a [SettingsField]) -> Result<&'a SettingsField, String> {
        fields
            .get(self.selected)
            .ok_or_else(|| "当前没有可编辑的设置项".to_owned())
    }

    /// 编辑态按键：Left/Right 只改草稿，Enter 产出提交，Esc 产出取消。
    ///
    /// 其它键返回 `Handled`，由调用方吞掉，不落到全局焦点键。
    pub(crate) fn handle_key(
        &mut self,
        code: KeyCode,
        fields: &[SettingsField],
        settings: &SettingsHandle,
        theme: &dyn ThemeApi,
        theme_id: &str,
    ) -> Result<SettingsOutcome, String> {
        match code {
            KeyCode::Left => {
                self.adjust(-1, fields, settings, theme, theme_id)?;
                Ok(SettingsOutcome::Handled)
            }
            KeyCode::Right => {
                self.adjust(1, fields, settings, theme, theme_id)?;
                Ok(SettingsOutcome::Handled)
            }
            KeyCode::Enter => {
                let (field, value) = self.submission(fields)?;
                Ok(SettingsOutcome::Submit(field, value))
            }
            KeyCode::Esc => Ok(SettingsOutcome::Cancel),
            _ => Ok(SettingsOutcome::Handled),
        }
    }

    fn adjust(
        &mut self,
        direction: i32,
        fields: &[SettingsField],
        settings: &SettingsHandle,
        theme: &dyn ThemeApi,
        theme_id: &str,
    ) -> Result<(), String> {
        let Some(edit) = self.edit.clone() else {
            return Ok(());
        };
        let field = fields
            .get(edit.index)
            .ok_or_else(|| "当前没有可编辑的设置项".to_owned())?;
        if field.kind == SettingsKind::ReadOnly {
            return Err(format!("{} 是只读配置", field.field_label()));
        }
        let draft = step_value(field, &edit.draft, direction, settings, theme, theme_id)?;
        if let Some(edit) = self.edit.as_mut() {
            edit.draft = draft;
        }
        Ok(())
    }

    fn submission(
        &self,
        fields: &[SettingsField],
    ) -> Result<(SettingsField, SettingValue), String> {
        let edit = self
            .edit
            .as_ref()
            .ok_or_else(|| "当前没有可编辑的设置项".to_owned())?;
        let field = fields
            .get(edit.index)
            .cloned()
            .ok_or_else(|| "当前没有可编辑的设置项".to_owned())?;
        Ok((field, edit.draft.clone()))
    }

    /// 当前草稿在值列的显示文本：`‹ 100% ›`。
    fn draft_display(
        &self,
        theme_id: &str,
        theme_name: &str,
        fields: &[SettingsField],
    ) -> Option<(usize, String)> {
        let edit = self.edit.as_ref()?;
        let field = fields.get(edit.index)?;
        Some((
            edit.index,
            draft_text(field, &edit.draft, theme_id, theme_name),
        ))
    }
}

/// 选择态 Enter：Bool 直接切换提交；其余可编辑项进入编辑态；ReadOnly 不进入。
pub(crate) fn begin_settings_edit(
    app: &mut App,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    if app.settings_editor.is_editing() {
        return Ok(());
    }
    app.settings_editor.select(app.row_index);
    let field = app
        .settings_editor
        .selected_field(&app.settings_fields)?
        .clone();
    match field.kind {
        SettingsKind::Category => {
            let folded = if app.settings_collapsed.remove(&field.key) {
                false
            } else {
                app.settings_collapsed.insert(field.key.clone());
                true
            };
            app.set_transient_status(format!(
                "{} {}",
                field.label,
                if folded { "已折叠" } else { "已展开" }
            ));
            app.refresh_settings_view(settings);
            Ok(())
        }
        SettingsKind::ReadOnly => {
            app.set_transient_status(format!("{} 是只读配置", field.field_label()));
            Ok(())
        }
        SettingsKind::Bool => adjust_setting(app, 1, player, settings, theme),
        _ => {
            let draft = current_value(&field, player, settings, &app.theme_id, app.visual_mode);
            app.settings_editor.start_edit(draft);
            paint_draft_row(app);
            Ok(())
        }
    }
}

/// 选择态直接调整（Bool 立即提交；测试与旧入口共用）。
pub(crate) fn adjust_setting(
    app: &mut App,
    direction: i32,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    app.settings_editor.select(app.row_index);
    let field = app
        .settings_editor
        .selected_field(&app.settings_fields)?
        .clone();
    if field.kind == SettingsKind::Category {
        return Err("分类不能直接调整".to_owned());
    }
    if field.kind == SettingsKind::ReadOnly {
        return Err(format!("{} 是只读配置", field.field_label()));
    }
    let current = current_value(&field, player, settings, &app.theme_id, app.visual_mode);
    let value = step_value(&field, &current, direction, settings, theme, &app.theme_id)?;
    set_setting_value(app, &field, value, player, settings, theme)?;
    app.refresh_settings_view(settings);
    Ok(())
}

/// 编辑态 ←/→：只改 draft，不写设置、不碰播放器。
#[cfg(test)]
pub(crate) fn adjust_settings_edit(
    app: &mut App,
    direction: i32,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    app.settings_editor.adjust(
        direction,
        &app.settings_fields,
        settings,
        theme,
        &app.theme_id,
    )?;
    paint_draft_row(app);
    Ok(())
}

/// 编辑态 Enter：提交 draft；失败时保留编辑态，由调用方写入错误状态。
#[cfg(test)]
pub(crate) fn commit_settings_edit(
    app: &mut App,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    if !app.settings_editor.is_editing() {
        return Ok(());
    }
    let (field, value) = app.settings_editor.submission(&app.settings_fields)?;
    set_setting_value(app, &field, value, player, settings, theme)?;
    app.settings_editor.clear_edit();
    app.refresh_settings_view(settings);
    Ok(())
}

/// 编辑态 Esc：丢弃 draft，恢复显示当前值。
#[cfg(test)]
pub(crate) fn cancel_settings_edit(app: &mut App, settings: &SettingsHandle) -> bool {
    if !cancel_edit(app, settings) {
        return false;
    }
    app.set_transient_status("已取消修改");
    true
}

fn cancel_edit(app: &mut App, settings: &SettingsHandle) -> bool {
    if !app.settings_editor.clear_edit() {
        return false;
    }
    app.refresh_settings_view(settings);
    true
}

/// 编辑态按键：Left/Right 调整，Enter 提交，Esc 取消；其它键一律吞掉。
///
/// 返回 `true` 表示按键已被编辑态消费，事件循环不应再走全局焦点键。
pub(crate) fn handle_settings_edit_key(
    code: KeyCode,
    app: &mut App,
    player: &TuiAudio,
    theme: &dyn ThemeApi,
    settings: &SettingsHandle,
) -> bool {
    if !app.settings_editor.is_editing() {
        return false;
    }
    let outcome =
        app.settings_editor
            .handle_key(code, &app.settings_fields, settings, theme, &app.theme_id);
    // 设置编辑态只给 ←/→/Enter/Esc 配提示音；其它键被吞掉但不发声。
    let cue = match code {
        KeyCode::Left | KeyCode::Right => Some(SoundCue::Move),
        KeyCode::Enter => Some(SoundCue::Confirm),
        KeyCode::Esc => Some(SoundCue::Back),
        _ => None,
    };
    match outcome {
        Ok(SettingsOutcome::Handled) => paint_draft_row(app),
        Ok(SettingsOutcome::Submit(field, value)) => {
            match set_setting_value(app, &field, value, player, settings, theme) {
                Ok(()) => {
                    app.settings_editor.clear_edit();
                    app.refresh_settings_view(settings);
                }
                Err(error) => app.set_status(error),
            }
        }
        Ok(SettingsOutcome::Cancel) => {
            cancel_edit(app, settings);
            app.set_transient_status("已取消修改");
        }
        Err(error) => app.set_status(error),
    }
    if let Some(cue) = cue {
        app.sound.play(cue);
    }
    true
}

pub(crate) fn reset_setting(
    app: &mut App,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    app.settings_editor.select(app.row_index);
    let field = app
        .settings_editor
        .selected_field(&app.settings_fields)?
        .clone();
    if field.kind == SettingsKind::Category {
        return Err("分类不能直接重置".to_owned());
    }
    let Some(value) = field.default.clone() else {
        return Err(format!("{} 是只读配置", field.field_label()));
    };
    set_setting_value(app, &field, value, player, settings, theme)?;
    app.set_transient_status(format!("已重置：{}", field.label));
    app.refresh_settings_view(settings);
    Ok(())
}

fn paint_draft_row(app: &mut App) {
    let Some((index, text)) =
        app.settings_editor
            .draft_display(&app.theme_id, &app.theme_name, &app.settings_fields)
    else {
        return;
    };
    set_row_value(app, index, text);
}

fn current_value(
    field: &SettingsField,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme_id: &str,
    visual: VisualMode,
) -> SettingValue {
    match field.kind {
        SettingsKind::Category => SettingValue::Bool(false),
        SettingsKind::Volume => SettingValue::Num(f64::from(player.volume().clamp(0.0, 1.0))),
        SettingsKind::PlaybackMode => SettingValue::Str(player.mode_id().to_owned()),
        SettingsKind::Theme => SettingValue::Str(theme_id.to_owned()),
        SettingsKind::Bool => SettingValue::Bool(
            settings
                .get_bool(&field.key)
                .unwrap_or_else(|| default_bool(&field.key)),
        ),
        SettingsKind::BufferMs => SettingValue::Num(live_buffer_ms(settings) as f64),
        // 优先读持久化值；从未写过时回退 App 当前模式（可能来自 NP_TUI_VISUAL）。
        SettingsKind::VisualMode => SettingValue::Str(
            settings
                .get_str(&field.key)
                .and_then(|id| VisualMode::from_id(&id))
                .unwrap_or(visual)
                .id()
                .to_owned(),
        ),
        SettingsKind::ReadOnly => settings
            .get(&field.key)
            .unwrap_or(SettingValue::Bool(false)),
    }
}

fn step_value(
    field: &SettingsField,
    current: &SettingValue,
    direction: i32,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
    theme_id: &str,
) -> Result<SettingValue, String> {
    let delta = direction.signum();
    match field.kind {
        SettingsKind::Category => Err("分类不能直接调整".to_owned()),
        SettingsKind::Volume => {
            let number = match current {
                SettingValue::Num(value) => *value,
                _ => 1.0,
            };
            // 1% 步进：先用百分比取整，避免浮点误差累积。
            let percent = (number * 100.0).round() as i32 + delta;
            Ok(SettingValue::Num(f64::from(percent.clamp(0, 100)) / 100.0))
        }
        SettingsKind::PlaybackMode => {
            let ids = ["sequential", "list", "single", "shuffle"];
            let current = match current {
                SettingValue::Str(id) => id.as_str(),
                _ => "sequential",
            };
            let index = ids.iter().position(|id| *id == current).unwrap_or(0) as i32;
            let next = ids[(index + delta).rem_euclid(ids.len() as i32) as usize];
            Ok(SettingValue::Str(next.to_owned()))
        }
        SettingsKind::Theme => {
            let entries = theme.list().map_err(|error| error.message)?;
            if entries.is_empty() {
                return Err("没有可用主题".to_owned());
            }
            let current = match current {
                SettingValue::Str(id) => id.as_str(),
                _ => theme_id,
            };
            let index = entries
                .iter()
                .position(|entry| entry.id == current)
                .unwrap_or(0) as i32;
            let next = entries[(index + delta).rem_euclid(entries.len() as i32) as usize]
                .id
                .clone();
            Ok(SettingValue::Str(next))
        }
        SettingsKind::Bool => {
            let value = match current {
                SettingValue::Bool(value) => *value,
                _ => settings
                    .get_bool(&field.key)
                    .unwrap_or_else(|| default_bool(&field.key)),
            };
            Ok(SettingValue::Bool(!value))
        }
        SettingsKind::BufferMs => {
            let value = match current {
                SettingValue::Num(value) => *value,
                _ => live_buffer_ms(settings) as f64,
            };
            let next = (value.round() as i64 + i64::from(delta) * 100).clamp(
                np_core::settings::LIVE_BUFFER_MS_MIN as i64,
                np_core::settings::LIVE_BUFFER_MS_MAX as i64,
            );
            Ok(SettingValue::Num(next as f64))
        }
        SettingsKind::VisualMode => {
            let mode = match current {
                SettingValue::Str(id) => VisualMode::from_id(id).unwrap_or_default(),
                _ => VisualMode::default(),
            };
            let index = VisualMode::ALL
                .iter()
                .position(|candidate| *candidate == mode)
                .unwrap_or(0) as i32;
            let next =
                VisualMode::ALL[(index + delta).rem_euclid(VisualMode::ALL.len() as i32) as usize];
            Ok(SettingValue::Str(next.id().to_owned()))
        }
        SettingsKind::ReadOnly => Err("只读配置不能修改".to_owned()),
    }
}

/// 编辑态在值列显示的文本：`‹ 100% ›`。
fn draft_text(
    field: &SettingsField,
    value: &SettingValue,
    theme_id: &str,
    theme_name: &str,
) -> String {
    let text = match field.kind {
        SettingsKind::Category => String::new(),
        SettingsKind::Volume => match value {
            SettingValue::Num(number) => format!("{}%", (number * 100.0).round() as i64),
            _ => "100%".to_owned(),
        },
        SettingsKind::PlaybackMode => match value {
            SettingValue::Str(id) => playback_mode_symbol(id).to_owned(),
            _ => "·".to_owned(),
        },
        SettingsKind::Theme => match value {
            SettingValue::Str(id) if id == theme_id => theme_name.to_owned(),
            SettingValue::Str(id) => id.clone(),
            _ => theme_name.to_owned(),
        },
        SettingsKind::Bool => bool_text(matches!(value, SettingValue::Bool(true))),
        SettingsKind::BufferMs => match value {
            SettingValue::Num(number) => format!("{} ms", number.round() as i64),
            _ => "— ms".to_owned(),
        },
        SettingsKind::VisualMode => match value {
            SettingValue::Str(id) => VisualMode::from_id(id)
                .unwrap_or_default()
                .label()
                .to_owned(),
            _ => VisualMode::default().label().to_owned(),
        },
        SettingsKind::ReadOnly => value.display(),
    };
    format!("‹ {text} ›")
}

fn default_bool(key: &str) -> bool {
    match default_value(key) {
        SettingValue::Bool(value) => value,
        _ => false,
    }
}

fn set_setting_value(
    app: &mut App,
    field: &SettingsField,
    value: SettingValue,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    match field.kind {
        SettingsKind::Category => return Err("分类不能直接修改".to_owned()),
        SettingsKind::Volume => {
            let SettingValue::Num(number) = value else {
                return Err("音量设置类型错误".to_owned());
            };
            let volume = (number as f32).clamp(0.0, 1.0);
            save(settings, &field.key, SettingValue::Num(f64::from(volume)))?;
            player.set_volume(volume);
            app.set_transient_status(format!("音量 {}%", (volume * 100.0).round() as i32));
        }
        SettingsKind::PlaybackMode => {
            let SettingValue::Str(id) = value else {
                return Err("播放模式设置类型错误".to_owned());
            };
            if !player.set_mode_id(&id) {
                return Err(format!("未知播放模式：{id}"));
            }
            save(settings, &field.key, SettingValue::Str(id.clone()))?;
            app.set_transient_status(format!("播放模式 {}", playback_mode_symbol(&id)));
        }
        SettingsKind::Theme => {
            let SettingValue::Str(id) = value else {
                return Err("主题设置类型错误".to_owned());
            };
            app.load_theme(theme, &id);
            let actual = app.theme_id.clone();
            save(settings, &field.key, SettingValue::Str(actual.clone()))?;
            if actual == id {
                app.set_transient_status(format!("主题 {}", app.theme_name));
            }
        }
        SettingsKind::Bool => {
            let SettingValue::Bool(enabled) = value else {
                return Err("开关设置类型错误".to_owned());
            };
            if field.key == "startup.enabled" {
                if enabled {
                    startup::set_enabled(true).map_err(|error| format!("自启设置失败：{error}"))?;
                } else {
                    let _ = startup::set_enabled(false);
                }
            }
            save(settings, &field.key, SettingValue::Bool(enabled))?;
            if field.key == "ui.sound" {
                // 音效开关立即生效，不等到下次启动。
                app.sound.set_enabled(enabled);
                app.set_transient_status(format!("{} {}", field.label, bool_text(enabled)));
                return Ok(());
            }
            if field.key == "tui.focusMode" {
                // 专注模式运行期立即生效；Ctrl+F 也写回同一个配置键。
                app.focus_mode = enabled;
                app.set_transient_status(format!("{} {}", field.label, bool_text(enabled)));
                return Ok(());
            }
            if field.key == "ui.motion" {
                app.set_transient_status(format!(
                    "{} {} · 下次启动生效",
                    field.label,
                    bool_text(enabled)
                ));
                return Ok(());
            }
            app.set_transient_status(format!("{} {}", field.label, bool_text(enabled)));
        }
        SettingsKind::BufferMs => {
            let SettingValue::Num(number) = value else {
                return Err("直播缓冲设置类型错误".to_owned());
            };
            let milliseconds = (number.round() as i64).clamp(
                np_core::settings::LIVE_BUFFER_MS_MIN as i64,
                np_core::settings::LIVE_BUFFER_MS_MAX as i64,
            );
            save(settings, &field.key, SettingValue::Num(milliseconds as f64))?;
            player.set_live_buffer_target(milliseconds as u64);
            app.set_transient_status(format!("直播缓冲 {milliseconds}ms"));
        }
        SettingsKind::VisualMode => {
            let SettingValue::Str(id) = value else {
                return Err("视觉样式设置类型错误".to_owned());
            };
            let mode = VisualMode::from_id(&id).ok_or_else(|| format!("未知视觉样式：{id}"))?;
            save(
                settings,
                &field.key,
                SettingValue::Str(mode.id().to_owned()),
            )?;
            app.visual_mode = mode;
            app.set_transient_status(format!("视觉样式 {}", mode.label()));
        }
        SettingsKind::ReadOnly => return Err("只读配置不能修改".to_owned()),
    }
    Ok(())
}

fn save(settings: &SettingsHandle, key: &str, value: SettingValue) -> Result<(), String> {
    settings
        .set_and_save(key, value)
        .map_err(|error| error.message)
}
