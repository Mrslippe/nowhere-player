//! 输入映射：把按键 + 当前上下文解析成语义动作。
//!
//! 这里不碰播放器、库、网络；只回答“现在这个键是什么动作”。
//! 展示层需要键名时也查同一张表，避免 player_bar / 状态栏各写一份。

use crossterm::event::{KeyCode, KeyModifiers};

use crate::model::{BrowseKind, Focus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    PrevTrack,
    NextTrack,
    SeekBack,
    SeekForward,
    TogglePlay,
    MoveUp,
    MoveDown,
    FocusLeft,
    FocusRight,
    Enter,
    Escape,
    TogglePlaylist,
    CreatePlaylist,
    DeletePlaylist,
    Add,
    Remove,
    ResetSetting,
    ClearSearch,
    OpenSearch,
    NextSearchPlatform,
    OpenCommand,
    ToggleFocusMode,
    RefreshLive,
    LiveChat,
    ToggleVideo,
    ToggleDanmaku,
    CycleMode,
    VolumeUp,
    VolumeDown,
    Quit,
}

pub(crate) const TRANSPORT_ACTIONS: [Action; 5] = [
    Action::PrevTrack,
    Action::SeekBack,
    Action::TogglePlay,
    Action::SeekForward,
    Action::NextTrack,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Global,
    Live,
    Browse,
    List,
    PlaylistRoot,
    Search,
    SearchList,
    SettingsList,
}

struct Binding {
    key: KeyCode,
    shift: bool,
    scope: Scope,
    action: Action,
    label: &'static str,
}

/// 带 Ctrl 的全局键；不能和普通字符键混在 `BINDINGS` 里，否则 `Ctrl+F`
/// 会退化成普通 `f`（`resolve` 目前只为 Enter 区分修饰键）。
struct ControlBinding {
    key: KeyCode,
    action: Action,
    label: &'static str,
}

const CONTROL_BINDINGS: &[ControlBinding] = &[ControlBinding {
    key: KeyCode::Char('f'),
    action: Action::ToggleFocusMode,
    label: "Ctrl+F",
}];

const BINDINGS: &[Binding] = &[
    // Live 页：r 刷新到最新、c 弹幕入口（T-0094 占位）。
    Binding {
        key: KeyCode::Char('r'),
        shift: false,
        scope: Scope::Live,
        action: Action::RefreshLive,
        label: "r",
    },
    Binding {
        key: KeyCode::Char('c'),
        shift: false,
        scope: Scope::Live,
        action: Action::LiveChat,
        label: "c",
    },
    // 直播画面：拉起 / 关闭外部播放器窗口（T-0104）。
    Binding {
        key: KeyCode::Char('v'),
        shift: false,
        scope: Scope::Live,
        action: Action::ToggleVideo,
        label: "v",
    },
    // 右侧弹幕浮层开关；隐藏态不改变任何既有键位。
    Binding {
        key: KeyCode::Char('d'),
        shift: false,
        scope: Scope::Live,
        action: Action::ToggleDanmaku,
        label: "d",
    },
    // F5 手动刷新直播到 live edge；非直播时只给提示。
    Binding {
        key: KeyCode::F(5),
        shift: false,
        scope: Scope::Global,
        action: Action::RefreshLive,
        label: "F5",
    },
    // 设置页的 `r` 重置优先于全局键；←/→ 在选择态交给全局焦点切换，
    // 编辑态由事件循环先行拦截，不改值。
    Binding {
        key: KeyCode::Char('r'),
        shift: false,
        scope: Scope::SettingsList,
        action: Action::ResetSetting,
        label: "r",
    },
    // PlayList 根。
    Binding {
        key: KeyCode::Char('n'),
        shift: false,
        scope: Scope::PlaylistRoot,
        action: Action::CreatePlaylist,
        label: "n",
    },
    Binding {
        key: KeyCode::Char('d'),
        shift: false,
        scope: Scope::PlaylistRoot,
        action: Action::DeletePlaylist,
        label: "d",
    },
    // Search 结果清空。
    Binding {
        key: KeyCode::Char('c'),
        shift: false,
        scope: Scope::SearchList,
        action: Action::ClearSearch,
        label: "c",
    },
    // Search 内切平台。
    Binding {
        key: KeyCode::Tab,
        shift: false,
        scope: Scope::Search,
        action: Action::NextSearchPlatform,
        label: "Tab",
    },
    // 列表通用。
    Binding {
        key: KeyCode::Char('a'),
        shift: false,
        scope: Scope::List,
        action: Action::Add,
        label: "a",
    },
    Binding {
        key: KeyCode::Char('r'),
        shift: false,
        scope: Scope::List,
        action: Action::Remove,
        label: "r",
    },
    // Browse 内展开/折叠歌单。
    Binding {
        key: KeyCode::Enter,
        shift: true,
        scope: Scope::Browse,
        action: Action::TogglePlaylist,
        label: "Shift+Enter",
    },
    // 全局。
    Binding {
        key: KeyCode::Char(' '),
        shift: false,
        scope: Scope::Global,
        action: Action::TogglePlay,
        label: "Space",
    },
    Binding {
        key: KeyCode::Char(','),
        shift: false,
        scope: Scope::Global,
        action: Action::PrevTrack,
        label: ",",
    },
    Binding {
        key: KeyCode::Char('，'),
        shift: false,
        scope: Scope::Global,
        action: Action::PrevTrack,
        label: ",",
    },
    Binding {
        key: KeyCode::Char('.'),
        shift: false,
        scope: Scope::Global,
        action: Action::NextTrack,
        label: ".",
    },
    Binding {
        key: KeyCode::Char('。'),
        shift: false,
        scope: Scope::Global,
        action: Action::NextTrack,
        label: ".",
    },
    Binding {
        key: KeyCode::Char('['),
        shift: false,
        scope: Scope::Global,
        action: Action::SeekBack,
        label: "[",
    },
    Binding {
        key: KeyCode::Char('{'),
        shift: false,
        scope: Scope::Global,
        action: Action::SeekBack,
        label: "[",
    },
    Binding {
        key: KeyCode::Char('【'),
        shift: false,
        scope: Scope::Global,
        action: Action::SeekBack,
        label: "[",
    },
    Binding {
        key: KeyCode::Char(']'),
        shift: false,
        scope: Scope::Global,
        action: Action::SeekForward,
        label: "]",
    },
    Binding {
        key: KeyCode::Char('}'),
        shift: false,
        scope: Scope::Global,
        action: Action::SeekForward,
        label: "]",
    },
    Binding {
        key: KeyCode::Char('】'),
        shift: false,
        scope: Scope::Global,
        action: Action::SeekForward,
        label: "]",
    },
    Binding {
        key: KeyCode::Char('+'),
        shift: false,
        scope: Scope::Global,
        action: Action::VolumeUp,
        label: "+",
    },
    Binding {
        key: KeyCode::Char('='),
        shift: false,
        scope: Scope::Global,
        action: Action::VolumeUp,
        label: "+",
    },
    Binding {
        key: KeyCode::Char('-'),
        shift: false,
        scope: Scope::Global,
        action: Action::VolumeDown,
        label: "-",
    },
    Binding {
        key: KeyCode::Char('m'),
        shift: false,
        scope: Scope::Global,
        action: Action::CycleMode,
        label: "m",
    },
    Binding {
        key: KeyCode::Char('s'),
        shift: false,
        scope: Scope::Global,
        action: Action::OpenSearch,
        label: "s",
    },
    Binding {
        key: KeyCode::Char('/'),
        shift: false,
        scope: Scope::Global,
        action: Action::OpenCommand,
        label: "/",
    },
    Binding {
        key: KeyCode::Char('q'),
        shift: false,
        scope: Scope::Global,
        action: Action::Quit,
        label: "q",
    },
    Binding {
        key: KeyCode::Char('j'),
        shift: false,
        scope: Scope::Global,
        action: Action::MoveDown,
        label: "↓",
    },
    Binding {
        key: KeyCode::Down,
        shift: false,
        scope: Scope::Global,
        action: Action::MoveDown,
        label: "↓",
    },
    Binding {
        key: KeyCode::Char('k'),
        shift: false,
        scope: Scope::Global,
        action: Action::MoveUp,
        label: "↑",
    },
    Binding {
        key: KeyCode::Up,
        shift: false,
        scope: Scope::Global,
        action: Action::MoveUp,
        label: "↑",
    },
    Binding {
        key: KeyCode::Left,
        shift: false,
        scope: Scope::Global,
        action: Action::FocusLeft,
        label: "←",
    },
    Binding {
        key: KeyCode::Right,
        shift: false,
        scope: Scope::Global,
        action: Action::FocusRight,
        label: "→",
    },
    Binding {
        key: KeyCode::Enter,
        shift: false,
        scope: Scope::Global,
        action: Action::Enter,
        label: "Enter",
    },
    Binding {
        key: KeyCode::Enter,
        shift: true,
        scope: Scope::Global,
        action: Action::Enter,
        label: "Enter",
    },
    Binding {
        key: KeyCode::Esc,
        shift: false,
        scope: Scope::Global,
        action: Action::Escape,
        label: "Esc",
    },
];

pub(crate) struct InputContext {
    pub(crate) focus: Focus,
    pub(crate) kind: Option<BrowseKind>,
    pub(crate) live: bool,
}

impl InputContext {
    fn scopes(&self) -> Vec<Scope> {
        let mut scopes = Vec::with_capacity(6);
        // Live 页独占整屏，r/c 必须先于列表 / 全局作用域解析。
        if self.live {
            scopes.push(Scope::Live);
        }
        if self.focus == Focus::List {
            if matches!(self.kind, Some(BrowseKind::Settings)) {
                scopes.push(Scope::SettingsList);
            }
            if matches!(self.kind, Some(BrowseKind::PlaylistRoot)) {
                scopes.push(Scope::PlaylistRoot);
            }
            if matches!(self.kind, Some(BrowseKind::Search)) {
                scopes.push(Scope::SearchList);
            }
            scopes.push(Scope::List);
        } else {
            if matches!(self.kind, Some(BrowseKind::PlaylistRoot)) {
                scopes.push(Scope::PlaylistRoot);
            }
            scopes.push(Scope::Browse);
        }
        if matches!(self.kind, Some(BrowseKind::Search)) {
            scopes.push(Scope::Search);
        }
        scopes.push(Scope::Global);
        scopes
    }
}

pub(crate) fn resolve(
    context: &InputContext,
    key: KeyCode,
    modifiers: KeyModifiers,
) -> Option<Action> {
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    // Ctrl 表只吃 Ctrl（不带 Shift）；未命中的 Ctrl 组合继续走原逻辑，
    // 保持 Ctrl+J / Ctrl+K 等既有行为不变。
    if modifiers.contains(KeyModifiers::CONTROL)
        && !shift
        && let Some(binding) = CONTROL_BINDINGS.iter().find(|binding| binding.key == key)
    {
        return Some(binding.action);
    }
    for scope in context.scopes() {
        for binding in BINDINGS {
            if binding.scope != scope || binding.key != key {
                continue;
            }
            if key == KeyCode::Enter && binding.shift != shift {
                continue;
            }
            return Some(binding.action);
        }
    }
    None
}

/// 展示层查键名：和 resolve 共用同一张表。
pub(crate) fn label(action: Action) -> &'static str {
    if let Some(binding) = CONTROL_BINDINGS
        .iter()
        .find(|binding| binding.action == action)
    {
        return binding.label;
    }
    BINDINGS
        .iter()
        .find(|binding| binding.action == action)
        .map_or("", |binding| binding.label)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(focus: Focus, kind: BrowseKind) -> InputContext {
        InputContext {
            focus,
            kind: Some(kind),
            live: false,
        }
    }

    #[test]
    fn live_scope_overrides_list_and_global_keys() {
        let live = InputContext {
            focus: Focus::List,
            kind: Some(BrowseKind::Library),
            live: true,
        };
        assert_eq!(
            resolve(&live, KeyCode::Char('r'), KeyModifiers::NONE),
            Some(Action::RefreshLive)
        );
        assert_eq!(
            resolve(&live, KeyCode::Char('c'), KeyModifiers::NONE),
            Some(Action::LiveChat)
        );
        assert_eq!(
            resolve(&live, KeyCode::Char('d'), KeyModifiers::NONE),
            Some(Action::ToggleDanmaku)
        );
        assert_eq!(
            resolve(&live, KeyCode::Char('v'), KeyModifiers::NONE),
            Some(Action::ToggleVideo)
        );
        assert_eq!(label(Action::ToggleVideo), "v");
        assert_eq!(
            resolve(&live, KeyCode::Char(' '), KeyModifiers::NONE),
            Some(Action::TogglePlay)
        );
        assert_eq!(
            resolve(&live, KeyCode::F(5), KeyModifiers::NONE),
            Some(Action::RefreshLive)
        );
        assert_eq!(
            resolve(&live, KeyCode::Char('/'), KeyModifiers::NONE),
            Some(Action::OpenCommand)
        );
        let library = ctx(Focus::List, BrowseKind::Library);
        assert_eq!(
            resolve(&library, KeyCode::Char('c'), KeyModifiers::NONE),
            None
        );
    }

    #[test]
    fn transport_keys_are_resolved_from_the_table() {
        let library = ctx(Focus::List, BrowseKind::Library);
        assert_eq!(
            resolve(&library, KeyCode::Char(','), KeyModifiers::NONE),
            Some(Action::PrevTrack)
        );
        assert_eq!(
            resolve(&library, KeyCode::Char('，'), KeyModifiers::NONE),
            Some(Action::PrevTrack)
        );
        assert_eq!(
            resolve(&library, KeyCode::Char('.'), KeyModifiers::NONE),
            Some(Action::NextTrack)
        );
        assert_eq!(
            resolve(&library, KeyCode::Char('。'), KeyModifiers::NONE),
            Some(Action::NextTrack)
        );
        assert_eq!(label(Action::PrevTrack), ",");
        assert_eq!(label(Action::NextTrack), ".");
    }

    #[test]
    fn n_is_contextual_and_not_next_track() {
        let root = ctx(Focus::List, BrowseKind::PlaylistRoot);
        assert_eq!(
            resolve(&root, KeyCode::Char('n'), KeyModifiers::NONE),
            Some(Action::CreatePlaylist)
        );
        let library = ctx(Focus::List, BrowseKind::Library);
        assert_eq!(
            resolve(&library, KeyCode::Char('n'), KeyModifiers::NONE),
            None
        );
    }

    #[test]
    fn settings_left_right_do_not_change_values_in_selection_mode() {
        let settings = ctx(Focus::List, BrowseKind::Settings);
        // 选择态 ←/→ 落回全局焦点切换；改值只发生在编辑态。
        assert_eq!(
            resolve(&settings, KeyCode::Left, KeyModifiers::NONE),
            Some(Action::FocusLeft)
        );
        assert_eq!(
            resolve(&settings, KeyCode::Right, KeyModifiers::NONE),
            Some(Action::FocusRight)
        );
        let search = ctx(Focus::List, BrowseKind::Search);
        assert_eq!(
            resolve(&search, KeyCode::Tab, KeyModifiers::NONE),
            Some(Action::NextSearchPlatform)
        );
    }

    #[test]
    fn shift_enter_only_toggles_playlist_from_browse() {
        let browse = ctx(Focus::Browse, BrowseKind::PlaylistRoot);
        assert_eq!(
            resolve(&browse, KeyCode::Enter, KeyModifiers::SHIFT),
            Some(Action::TogglePlaylist)
        );
        let list = ctx(Focus::List, BrowseKind::PlaylistRoot);
        assert_eq!(
            resolve(&list, KeyCode::Enter, KeyModifiers::SHIFT),
            Some(Action::Enter)
        );
    }

    #[test]
    fn ctrl_f_toggles_focus_mode_without_stealing_plain_f() {
        let library = ctx(Focus::List, BrowseKind::Library);
        assert_eq!(
            resolve(&library, KeyCode::Char('f'), KeyModifiers::CONTROL),
            Some(Action::ToggleFocusMode)
        );
        assert_eq!(label(Action::ToggleFocusMode), "Ctrl+F");
        // 普通 f 没有绑定；Ctrl+Shift+F 也不走 Ctrl 表。
        assert_eq!(
            resolve(&library, KeyCode::Char('f'), KeyModifiers::NONE),
            None
        );
        assert_eq!(
            resolve(
                &library,
                KeyCode::Char('f'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            None
        );
        // 未登记的 Ctrl 组合仍按普通键解析，避免破坏既有行为。
        assert_eq!(
            resolve(&library, KeyCode::Char('j'), KeyModifiers::CONTROL),
            Some(Action::MoveDown)
        );
    }
}
