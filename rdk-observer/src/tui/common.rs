//! Shared TUI state, widgets, styles, and value formatting

use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState};

////////////////////////////////////////////////////////////////////////////////
// Shared values and styles

const TABLE_DECORATION_HEIGHT: u16 = 3;

pub const UNAVAILABLE_VALUE: &str = "-";
pub const ACCENT_STYLE: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
pub const ACTIVE_HEADER_STYLE: Style = ACCENT_STYLE;
pub const HEADER_STYLE: Style = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);
pub const HOTKEY_STYLE: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
pub const BORDER_STYLE: Style = Style::new().fg(Color::DarkGray);
pub const UNAVAILABLE_STYLE: Style = Style::new().fg(Color::DarkGray);
pub const SELECTED_ROW_STYLE: Style = Style::new()
    .fg(Color::Black)
    .bg(Color::Cyan)
    .add_modifier(Modifier::BOLD);

////////////////////////////////////////////////////////////////////////////////
// Shared table state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
}

#[derive(Default)]
pub struct ScrollableTableState {
    table: TableState,
    area: Rect,
    item_count: usize,
}

impl ScrollableTableState {
    ////////////////////////////////////////////////////////////////////////////////
    /// Update the table size and keep the selected row valid
    pub fn set_content(&mut self, area: Rect, item_count: usize) {
        self.area = area;
        self.item_count = item_count;

        let selected = (item_count > 0).then(|| {
            self.table
                .selected()
                .unwrap_or_default()
                .min(item_count - 1)
        });
        self.table.select(selected);
        *self.table.offset_mut() = self.table.offset().min(item_count.saturating_sub(1));
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Select the first row
    pub fn reset_selection(&mut self) {
        self.table.select(Some(0));
        *self.table.offset_mut() = 0;
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Move the selection by one row
    pub fn scroll(&mut self, direction: ScrollDirection) {
        if self.item_count == 0 {
            self.table.select(None);
            return;
        }

        match direction {
            ScrollDirection::Up => self.table.select_previous(),
            ScrollDirection::Down => self.table.select_next(),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Check whether a mouse event is inside the table
    pub fn contains(&self, event: MouseEvent) -> bool {
        event.column >= self.area.left()
            && event.column < self.area.right()
            && event.row >= self.area.top()
            && event.row < self.area.bottom()
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Return the scroll direction for a mouse wheel event
pub fn mouse_scroll_direction(event: MouseEvent) -> Option<ScrollDirection> {
    match event.kind {
        MouseEventKind::ScrollUp => Some(ScrollDirection::Up),
        MouseEventKind::ScrollDown => Some(ScrollDirection::Down),
        _ => None,
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Draw a table with a scrollbar
pub fn render_scrollable_table(
    frame: &mut Frame<'_>,
    area: Rect,
    table: Table<'_>,
    state: &mut ScrollableTableState,
    item_count: usize,
) {
    state.set_content(area, item_count);
    frame.render_stateful_widget(table, area, &mut state.table);
    render_table_scrollbar(
        frame,
        area,
        item_count,
        state.table.selected().unwrap_or_default(),
    );
}

////////////////////////////////////////////////////////////////////////////////
/// Draw a scrollbar when needed
fn render_table_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    content_length: usize,
    position: usize,
) {
    let viewport_length = usize::from(area.height.saturating_sub(TABLE_DECORATION_HEIGHT));
    if viewport_length == 0 || content_length <= viewport_length {
        return;
    }

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .thumb_style(ACCENT_STYLE)
        .track_style(BORDER_STYLE);
    let mut state = ScrollbarState::new(content_length)
        .position(position)
        .viewport_content_length(viewport_length);
    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut state,
    );
}

////////////////////////////////////////////////////////////////////////////////
/// Build controls shared by both screens
pub fn common_controls() -> Vec<Span<'static>> {
    vec![
        Span::raw(" "),
        Span::styled("[q/Esc]", HOTKEY_STYLE),
        Span::raw(" Quit   "),
        Span::styled("[Tab]", HOTKEY_STYLE),
        Span::raw(" Switch view   "),
    ]
}

////////////////////////////////////////////////////////////////////////////////
/// Build a metric cell
pub fn metric_cell(value: Option<String>) -> Cell<'static> {
    match value {
        Some(value) => Cell::from(Line::from(value).right_aligned()),
        None => Cell::from(Line::from(UNAVAILABLE_VALUE).right_aligned()).style(UNAVAILABLE_STYLE),
    }
}

////////////////////////////////////////////////////////////////////////////////
// Value formatting

/// Show "-" for a missing value
pub fn display_value(value: Option<String>) -> String {
    value.unwrap_or_else(|| UNAVAILABLE_VALUE.to_string())
}

////////////////////////////////////////////////////////////////////////////////
/// Format a percentage
pub fn format_percent(value: Option<f64>) -> Option<String> {
    value.map(|value| format!("{value:.1}%"))
}

////////////////////////////////////////////////////////////////////////////////
/// Format bytes with binary units
pub fn format_bytes(value: Option<u64>) -> Option<String> {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    value.map(|value| {
        if value >= GIB {
            format_scaled_integer(value, GIB, " GiB")
        } else if value >= MIB {
            format_scaled_integer(value, MIB, " MiB")
        } else if value >= KIB {
            format_scaled_integer(value, KIB, " KiB")
        } else {
            format!("{value}.0 B")
        }
    })
}

////////////////////////////////////////////////////////////////////////////////
/// Format bytes per second
pub fn format_byte_rate(value: Option<f64>) -> Option<String> {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    value.map(|value| {
        let value = if value >= GIB {
            format!("{:.1} GiB", value / GIB)
        } else if value >= MIB {
            format!("{:.1} MiB", value / MIB)
        } else if value >= KIB {
            format!("{:.1} KiB", value / KIB)
        } else {
            format!("{value:.1} B")
        };

        format!("{value}/s")
    })
}

////////////////////////////////////////////////////////////////////////////////
/// Format a count with decimal units
pub fn format_count(value: Option<u64>) -> Option<String> {
    const KILO: u64 = 1_000;
    const MEGA: u64 = KILO * 1_000;
    const GIGA: u64 = MEGA * 1_000;

    value.map(|value| {
        if value >= GIGA {
            format_scaled_integer(value, GIGA, "G")
        } else if value >= MEGA {
            format_scaled_integer(value, MEGA, "M")
        } else if value >= KILO {
            format_scaled_integer(value, KILO, "K")
        } else {
            value.to_string()
        }
    })
}

////////////////////////////////////////////////////////////////////////////////
/// Format an integer with one decimal place
fn format_scaled_integer(value: u64, unit: u64, suffix: &str) -> String {
    format!("{:.1}{suffix}", value as f64 / unit as f64)
}
