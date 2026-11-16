use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TuiSpan};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::App;
#[cfg(test)]
use crate::theme::Palette;
#[cfg(test)]
use np_core::{Output, Span, SpanColor, Tone};

pub(crate) fn truncate_width(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for character in value.chars() {
        let char_width = UnicodeWidthStr::width(character.to_string().as_str());
        if used + char_width + 1 > width {
            break;
        }
        out.push(character);
        used += char_width;
    }
    out.push('…');
    out
}

/// Controller 输入行：按显示宽度裁剪，光标靠近右侧时把文本整体左移。
/// 返回行和光标相对内容区左边缘的列偏移。
pub(crate) fn input_line(app: &App, width: usize) -> (Line<'static>, usize) {
    let width = width.max(3);
    let text_width = width - 2;
    let (start, end) = input_window(&app.input, app.cursor, text_width);
    let mut spans = vec![TuiSpan::styled(
        "› ",
        Style::default().fg(app.palette.accent),
    )];
    for (index, character) in app.input.iter().enumerate().take(end).skip(start) {
        if index == app.cursor {
            spans.push(TuiSpan::styled(
                character.to_string(),
                Style::default()
                    .bg(app.palette.fg)
                    .fg(app.palette.bg.unwrap_or(Color::Reset))
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(TuiSpan::raw(character.to_string()));
        }
    }
    if app.cursor == app.input.len() {
        spans.push(TuiSpan::styled(
            " ",
            Style::default()
                .bg(app.palette.fg)
                .fg(app.palette.bg.unwrap_or(Color::Reset)),
        ));
    }
    let prefix: String = app.input[start..app.cursor].iter().collect();
    let cursor_x = 2 + UnicodeWidthStr::width(prefix.as_str());
    (Line::from(spans), cursor_x.min(width.saturating_sub(1)))
}

/// 返回输入文本的可见区间；宽度按终端显示列计算，CJK 占两格。
pub(crate) fn input_window(chars: &[char], cursor: usize, width: usize) -> (usize, usize) {
    if width == 0 {
        return (cursor, cursor);
    }
    let char_width = |character: char| UnicodeWidthChar::width(character).unwrap_or(0);
    let cursor_width = chars
        .get(cursor)
        .map(|character| char_width(*character))
        .unwrap_or(1)
        .max(1)
        .min(width);
    let mut start = cursor;
    let mut used = 0;
    while start > 0 {
        let character_width = char_width(chars[start - 1]);
        if used + character_width + cursor_width > width {
            break;
        }
        used += character_width;
        start -= 1;
    }
    let mut end = start;
    let mut visible = 0;
    while end < chars.len() {
        let character_width = char_width(chars[end]);
        if visible + character_width > width {
            break;
        }
        visible += character_width;
        end += 1;
    }
    (start, end)
}

#[cfg(test)]
pub(crate) fn all_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for block in &app.blocks {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        if !block.input.is_empty() {
            lines.push(Line::from(vec![
                TuiSpan::styled("› ", Style::default().fg(app.palette.accent)),
                TuiSpan::raw(block.input.clone()),
            ]));
        }
        for output in &block.outputs {
            output_lines(output, &mut lines, &app.palette);
        }
        if let Some(footer) = &block.footer {
            lines.push(Line::styled(
                footer.clone(),
                Style::default().fg(app.palette.muted),
            ));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "输入 help 或 library list --limit=10 开始",
            Style::default().fg(app.palette.muted),
        ));
    }
    lines
}

#[cfg(test)]
pub(crate) fn span_style(span: &Span, tone: Option<Tone>, palette_ref: &Palette) -> Style {
    let mut style = Style::default();
    if span.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.dim {
        style = style.fg(palette_ref.muted);
    }
    let color = span.color.map(|color| match color {
        SpanColor::Accent => palette_ref.accent,
        SpanColor::Danger => palette_ref.danger,
        SpanColor::Warn => palette_ref.accent,
        SpanColor::Success => palette_ref.fg,
        SpanColor::Muted => palette_ref.muted,
    });
    let tone = tone.map(|tone| match tone {
        Tone::Accent => palette_ref.accent,
        Tone::Danger => palette_ref.danger,
        Tone::Warn => palette_ref.accent,
        Tone::Success => palette_ref.fg,
        Tone::Muted => palette_ref.muted,
    });
    style.fg(color.or(tone).unwrap_or(palette_ref.fg))
}

#[cfg(test)]
pub(crate) fn output_lines(output: &Output, lines: &mut Vec<Line<'static>>, palette_ref: &Palette) {
    match output {
        Output::Text { spans, tone } => {
            lines.push(Line::from(
                spans
                    .iter()
                    .map(|span| {
                        TuiSpan::styled(span.text.clone(), span_style(span, *tone, palette_ref))
                    })
                    .collect::<Vec<_>>(),
            ));
        }
        Output::Table { columns, rows, .. } => {
            if columns.is_empty() {
                return;
            }
            let widths = table_widths(columns, rows);
            lines.push(Line::styled(
                format_row(columns, &widths),
                Style::default()
                    .fg(palette_ref.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::styled(
                widths
                    .iter()
                    .map(|width| "─".repeat(*width))
                    .collect::<Vec<_>>()
                    .join("  "),
                Style::default().fg(palette_ref.muted),
            ));
            for row in rows {
                lines.push(Line::raw(format_row(row, &widths)));
            }
        }
        Output::Error {
            code,
            message,
            hint,
            ..
        } => {
            lines.push(Line::styled(
                format!("错误 {code} · {message}"),
                Style::default().fg(palette_ref.danger),
            ));
            if let Some(hint) = hint {
                lines.push(Line::styled(
                    hint.clone(),
                    Style::default().fg(palette_ref.muted),
                ));
            }
        }
        Output::Hint { text, suggestion } => {
            let mut line = text.clone();
            if let Some(suggestion) = suggestion {
                line.push_str(" · ");
                line.push_str(suggestion);
            }
            lines.push(Line::styled(line, Style::default().fg(palette_ref.muted)));
        }
        Output::Progress {
            label,
            current,
            total,
            unit,
        } => {
            let percent = if *total == 0 {
                0
            } else {
                current.saturating_mul(100) / total
            };
            lines.push(Line::styled(
                format!("{label}  {percent}%  {current}/{total} {unit}"),
                Style::default().fg(palette_ref.accent),
            ));
        }
        Output::Media { kind, .. } => {
            lines.push(Line::styled(
                format!("[媒体 {kind:?}]"),
                Style::default().fg(palette_ref.muted),
            ));
        }
    }
}

pub(crate) fn table_widths(columns: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    let mut widths: Vec<usize> = columns.iter().map(|value| display_width(value)).collect();
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            if index >= widths.len() {
                widths.resize(index + 1, 0);
            }
            widths[index] = widths[index].max(display_width(value));
        }
    }
    widths
}

pub(crate) fn format_row(row: &[String], widths: &[usize]) -> String {
    (0..widths.len())
        .map(|index| {
            let value = row.get(index).map_or("", String::as_str);
            let value = truncate_width(value, widths[index]);
            let pad = widths[index].saturating_sub(display_width(&value));
            format!("{value}{}", " ".repeat(pad))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// List 的固定列宽规则：# 4、标题 24、作者 10、时长 8，列间 1 格。
/// 标题/作者随窗口伸缩；可用宽度不足 49 时只压缩这两列。
pub(crate) fn list_widths(
    columns: &[String],
    rows: &[Vec<String>],
    available: usize,
) -> Vec<usize> {
    if columns.len() == 3 {
        let category = 4.min(available.saturating_sub(2));
        let label = 16.min(available.saturating_sub(category + 3)).max(1);
        let value = available.saturating_sub(category + label + 2).max(1);
        return vec![category, label, value];
    }
    if columns.len() != 4 {
        return fit_table_widths(columns, rows, available);
    }
    if columns.get(1).is_some_and(|value| value == "名称")
        && columns.get(2).is_some_and(|value| value == "创建时间")
    {
        return vec![4, 24, 12, 5];
    }
    // 72 列下 List 内容 50：左间隔 1 + 表格 48 + 右间隔 1。
    // 表格 48 时固定 Title 24、Artist 9；宽出来的 1 格先补 Artist。
    let title = if available < 48 {
        available.saturating_sub(4 + 9 + 8 + 3).max(8)
    } else {
        24
    };
    let mut widths = vec![4, title, 9, 8];
    if available >= 48 {
        let extra = available - 48;
        if extra >= 1 {
            widths[2] = 10;
        }
        if extra > 1 {
            let rest = extra - 1;
            widths[1] += rest.div_ceil(2);
            widths[2] += rest / 2;
        }
    }
    let used: usize = widths.iter().sum::<usize>() + 3;
    if used > available {
        let mut overflow = used - available;
        for index in [1, 2] {
            let cut = overflow.min(widths[index].saturating_sub(1));
            widths[index] -= cut;
            overflow -= cut;
        }
    }
    widths
}

/// 在固定宽度内分配列宽：首列/末列保留，中间列按比例压缩。
pub(crate) fn fit_table_widths(
    columns: &[String],
    rows: &[Vec<String>],
    available: usize,
) -> Vec<usize> {
    let natural = table_widths(columns, rows);
    let count = natural.len();
    if count == 0 {
        return natural;
    }
    let separators = 2 * count.saturating_sub(1);
    if natural.iter().sum::<usize>() + separators <= available {
        return natural;
    }
    let mut widths = vec![1; count];
    if count == 1 {
        widths[0] = available.max(1);
        return widths;
    }
    let first = natural[0].min(4).min(available);
    let last = natural[count - 1].min(available.saturating_sub(first + separators).max(1));
    widths[0] = first;
    widths[count - 1] = last;
    let budget = available.saturating_sub(first + last + separators);
    let flexible = count - 2;
    if flexible == 0 {
        return widths;
    }
    if budget <= flexible {
        return widths;
    }
    let natural_flex: usize = natural[1..count - 1].iter().sum();
    let mut assigned = 0;
    for index in 1..count - 1 {
        let width = if index + 1 == count - 1 {
            budget.saturating_sub(assigned).max(1)
        } else {
            (natural[index] * budget / natural_flex.max(1)).max(1)
        };
        widths[index] = width;
        assigned += width;
    }
    widths
}

pub(crate) fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}
