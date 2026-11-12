use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TuiSpan};
use unicode_width::UnicodeWidthStr;

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

pub(crate) fn input_width_before(app: &App) -> usize {
    let prefix: String = app.input[..app.cursor].iter().collect();
    UnicodeWidthStr::width("› ") + UnicodeWidthStr::width(prefix.as_str())
}

pub(crate) fn input_line(app: &App) -> Line<'static> {
    let before: String = app.input[..app.cursor].iter().collect();
    let after: String = app.input[app.cursor..].iter().collect();
    let cursor_char = if app.cursor < app.input.len() {
        app.input[app.cursor].to_string()
    } else {
        " ".to_owned()
    };
    Line::from(vec![
        TuiSpan::styled("› ", Style::default().fg(app.palette.accent)),
        TuiSpan::raw(before),
        TuiSpan::styled(
            cursor_char,
            Style::default()
                .bg(app.palette.fg)
                .fg(app.palette.bg.unwrap_or(Color::Reset))
                .add_modifier(Modifier::BOLD),
        ),
        TuiSpan::raw(after),
    ])
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
        .join("  ")
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
