//! 全局搜索入口与平台切换：只改 UI 选择，不在这里发请求。

use crate::app::App;
use crate::model::{BrowseKind, Focus, SearchDisplay};

pub(crate) fn open_search(app: &mut App) {
    let Some(index) = app
        .view_kind
        .iter()
        .position(|kind| *kind == BrowseKind::Search)
    else {
        return;
    };
    app.view_index = index;
    app.row_index = 0;
    app.focus = Focus::List;
    if !matches!(app.search_display(), Some(SearchDisplay::Empty { .. })) {
        app.reset_search_to_empty();
    }
    app.begin_search_edit();
}

pub(crate) fn cycle_platform(app: &mut App) {
    app.search_platform = app.search_platform.next();
}
