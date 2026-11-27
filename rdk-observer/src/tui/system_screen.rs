//! System metrics screen

use super::common::{
    ACCENT_STYLE, BORDER_STYLE, HEADER_STYLE, HOTKEY_STYLE, SELECTED_ROW_STYLE, ScrollDirection,
    ScrollableTableState, UNAVAILABLE_STYLE, UNAVAILABLE_VALUE, common_controls, display_value,
    format_byte_rate, format_bytes, format_count, format_percent, metric_cell,
    mouse_scroll_direction, render_scrollable_table,
};
use crossterm::event::{KeyCode, MouseEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, LineGauge, Paragraph, Row, Table};
use rdk_observer_core::output_model::{
    PressureResourceRecord, SystemNetworkRecord, SystemStorageRecord,
};
use rdk_observer_core::runtime::RuntimeContext;
use rdk_observer_core::system_registry::SystemRegistrySnapshot;

////////////////////////////////////////////////////////////////////////////////
// Layout and styles

pub const CONTROLS_HEIGHT: u16 = 3;

const HORIZONTAL_TABLES_MIN_WIDTH: u16 = 150;
const MIN_CONTENT_HEIGHT: u16 = 5;
const SUMMARY_HEIGHT: u16 = 7;
const WARNING_USAGE_PERCENT: f64 = 60.0;
const CRITICAL_USAGE_PERCENT: f64 = 85.0;
const SUMMARY_WIDTHS: [Constraint; 3] = [
    Constraint::Percentage(20), // CPU
    Constraint::Percentage(35), // Memory
    Constraint::Percentage(45), // Pressure
];
const ACTIVITY_WIDTHS: [Constraint; 2] = [
    Constraint::Percentage(45), // Storage
    Constraint::Percentage(55), // Network
];
const STACKED_ACTIVITY_HEIGHTS: [Constraint; 2] = [
    Constraint::Percentage(50), // Storage
    Constraint::Percentage(50), // Network
];
const STORAGE_TABLE_WIDTHS: [Constraint; 5] = [
    Constraint::Percentage(30), // DEVICE
    Constraint::Percentage(18), // READ
    Constraint::Percentage(18), // WRITE
    Constraint::Percentage(17), // READ OPS
    Constraint::Percentage(17), // WRITE OPS
];
const NETWORK_TABLE_WIDTHS: [Constraint; 6] = [
    Constraint::Percentage(25), // INTERFACE
    Constraint::Percentage(15), // RX
    Constraint::Percentage(15), // TX
    Constraint::Percentage(20), // PKTS R/T
    Constraint::Percentage(12), // ERR R/T
    Constraint::Percentage(13), // DROP R/T
];
const PRESSURE_TABLE_WIDTHS: [Constraint; 7] = [
    Constraint::Min(9),    // RESOURCE
    Constraint::Length(7), // S10
    Constraint::Length(7), // S60
    Constraint::Length(7), // S300
    Constraint::Length(7), // F10
    Constraint::Length(7), // F60
    Constraint::Length(7), // F300
];
const HEALTHY_STYLE: Style = Style::new().fg(Color::Green).add_modifier(Modifier::BOLD);
const WARNING_STYLE: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
const CRITICAL_STYLE: Style = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);

////////////////////////////////////////////////////////////////////////////////
// System screen state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemListFocus {
    Storage,
    Network,
}

pub struct SystemScreen {
    focus: SystemListFocus,
    storage_table: ScrollableTableState,
    network_table: ScrollableTableState,
}

impl Default for SystemScreen {
    ////////////////////////////////////////////////////////////////////////////////
    fn default() -> Self {
        Self {
            focus: SystemListFocus::Storage,
            storage_table: ScrollableTableState::default(),
            network_table: ScrollableTableState::default(),
        }
    }
}

impl SystemScreen {
    ////////////////////////////////////////////////////////////////////////////////
    /// Handle system screen keys
    pub fn handle_key_event(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left => self.focus = SystemListFocus::Storage,
            KeyCode::Right => self.focus = SystemListFocus::Network,
            KeyCode::Up => self.scroll_list(ScrollDirection::Up),
            KeyCode::Down => self.scroll_list(ScrollDirection::Down),
            _ => {}
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Handle system table mouse scrolling
    pub fn handle_mouse_event(&mut self, event: MouseEvent) -> bool {
        let Some(direction) = mouse_scroll_direction(event) else {
            return false;
        };

        let focus = if self.storage_table.contains(event) {
            Some(SystemListFocus::Storage)
        } else if self.network_table.contains(event) {
            Some(SystemListFocus::Network)
        } else {
            None
        };
        let Some(focus) = focus else {
            return false;
        };

        self.focus = focus;
        self.scroll_list(direction);
        true
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw system metrics and controls
    pub fn render(
        &mut self,
        frame: &mut Frame<'_>,
        content_area: Rect,
        controls_area: Rect,
        context: &RuntimeContext,
    ) {
        // Use one snapshot for the whole frame
        let snapshot = context.system_registry.get_snapshot();
        let summary_height = content_area.height.min(SUMMARY_HEIGHT);
        let [summary_area, activity_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(summary_height),
                Constraint::Min(MIN_CONTENT_HEIGHT),
            ])
            .areas(content_area);

        let [cpu_area, memory_area, pressure_area] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(SUMMARY_WIDTHS)
            .areas(summary_area);

        self.render_cpu_panel(frame, cpu_area, &snapshot);
        self.render_memory_panel(frame, memory_area, &snapshot);
        self.render_pressure_panel(frame, pressure_area, &snapshot);

        // Stack the tables on narrow terminals
        let [storage_area, network_area] = if activity_area.width >= HORIZONTAL_TABLES_MIN_WIDTH {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints(ACTIVITY_WIDTHS)
                .areas(activity_area)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints(STACKED_ACTIVITY_HEIGHTS)
                .areas(activity_area)
        };
        self.render_storage_panel(frame, storage_area, &snapshot);
        self.render_network_panel(frame, network_area, &snapshot);
        self.render_controls(frame, controls_area);
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn scroll_list(&mut self, direction: ScrollDirection) {
        match self.focus {
            SystemListFocus::Storage => self.storage_table.scroll(direction),
            SystemListFocus::Network => self.network_table.scroll(direction),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw system CPU usage
    fn render_cpu_panel(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        snapshot: &SystemRegistrySnapshot,
    ) {
        let utilization = snapshot
            .last_cpu_record
            .as_ref()
            .and_then(|(_, record)| record.utilization)
            .map(|value| value.percent);
        let style = utilization.map_or(UNAVAILABLE_STYLE, usage_style);
        let block = system_block(" CPU ");
        let inner = block.inner(area);
        let [usage_area, gauge_area, _] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(inner);

        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("Usage  "),
                Span::styled(display_value(format_percent(utilization)), style),
            ])),
            usage_area,
        );
        render_metric_gauge(frame, gauge_area, utilization, style);
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw system memory usage
    fn render_memory_panel(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        snapshot: &SystemRegistrySnapshot,
    ) {
        let memory = snapshot
            .last_memory_record
            .as_ref()
            .map(|(_, record)| record);
        let total = memory.and_then(|record| record.total_bytes);
        let available = memory.and_then(|record| record.available_bytes);
        let used = total
            .zip(available)
            .and_then(|(total, available)| total.checked_sub(available));
        let percent = used
            .zip(total)
            .and_then(|(used, total)| (total > 0).then_some(used as f64 / total as f64 * 100.0));
        let style = percent.map_or(UNAVAILABLE_STYLE, usage_style);
        let block = system_block(" MEMORY ");
        let inner = block.inner(area);
        let [used_area, available_area, gauge_area, _] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(inner);

        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("Used   "),
                Span::styled(display_value(format_bytes(used)), style),
                Span::raw(" / "),
                Span::raw(display_value(format_bytes(total))),
                Span::raw("  "),
                Span::styled(display_value(format_percent(percent)), style),
            ])),
            used_area,
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("Avail  "),
                Span::raw(display_value(format_bytes(available))),
            ])),
            available_area,
        );
        render_metric_gauge(frame, gauge_area, percent, style);
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw system pressure
    fn render_pressure_panel(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        snapshot: &SystemRegistrySnapshot,
    ) {
        let pressure = snapshot
            .last_pressure_record
            .as_ref()
            .map(|(_, record)| record);
        let rows = [
            pressure_row("CPU", pressure.and_then(|record| record.cpu.as_ref())),
            pressure_row("Memory", pressure.and_then(|record| record.memory.as_ref())),
            pressure_row("I/O", pressure.and_then(|record| record.io.as_ref())),
        ];
        let table = Table::new(rows, PRESSURE_TABLE_WIDTHS)
            .header(system_table_header([
                "RESOURCE", "S10", "S60", "S300", "F10", "F60", "F300",
            ]))
            .block(system_block(" PRESSURE % "))
            .column_spacing(1);

        frame.render_widget(table, area);
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw the storage table
    fn render_storage_panel(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        snapshot: &SystemRegistrySnapshot,
    ) {
        let focused = self.focus == SystemListFocus::Storage;
        let Some((_, records)) = snapshot.last_io_records.as_ref() else {
            self.storage_table.set_content(area, 0);
            render_unavailable(frame, area, " STORAGE ", focused);
            return;
        };

        let rows = records.iter().map(storage_row);
        let table = Table::new(rows, STORAGE_TABLE_WIDTHS)
            .header(system_table_header([
                "DEVICE",
                "READ",
                "WRITE",
                "READ OPS",
                "WRITE OPS",
            ]))
            .block(system_list_block(" STORAGE ", focused))
            .row_highlight_style(if focused {
                SELECTED_ROW_STYLE
            } else {
                Style::default()
            })
            .column_spacing(1);
        render_scrollable_table(frame, area, table, &mut self.storage_table, records.len());
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw the network table
    fn render_network_panel(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        snapshot: &SystemRegistrySnapshot,
    ) {
        let focused = self.focus == SystemListFocus::Network;
        let Some((_, records)) = snapshot.last_network_records.as_ref() else {
            self.network_table.set_content(area, 0);
            render_unavailable(frame, area, " NETWORK ", focused);
            return;
        };

        let rows = records.iter().map(network_row);
        let table = Table::new(rows, NETWORK_TABLE_WIDTHS)
            .header(system_table_header([
                "INTERFACE",
                "RX",
                "TX",
                "PKTS R/T",
                "ERR R/T",
                "DROP R/T",
            ]))
            .block(system_list_block(" NETWORK ", focused))
            .row_highlight_style(if focused {
                SELECTED_ROW_STYLE
            } else {
                Style::default()
            })
            .column_spacing(1);
        render_scrollable_table(frame, area, table, &mut self.network_table, records.len());
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw system controls
    fn render_controls(&self, frame: &mut Frame<'_>, area: Rect) {
        let mut controls = common_controls();
        controls.extend([
            Span::styled("[ ← / → ]", HOTKEY_STYLE),
            Span::raw(" Select list   "),
            Span::styled("[ ↑ / ↓ / Wheel ]", HOTKEY_STYLE),
            Span::raw(" Scroll"),
        ]);
        let controls = Paragraph::new(Line::from(controls)).block(
            Block::default()
                .title(Span::styled(" CONTROLS ", ACCENT_STYLE))
                .borders(Borders::ALL)
                .border_style(BORDER_STYLE),
        );

        frame.render_widget(controls, area);
    }
}

////////////////////////////////////////////////////////////////////////////////
// System table rows and widgets

/// Build one storage row
fn storage_row(record: &SystemStorageRecord) -> Row<'_> {
    let read_rate = record
        .rates
        .as_ref()
        .and_then(|rates| rates.read_bytes_per_second);
    let write_rate = record
        .rates
        .as_ref()
        .and_then(|rates| rates.write_bytes_per_second);
    Row::new([
        Cell::from(record.device_identity.name.as_str()),
        metric_cell(format_byte_rate(read_rate)),
        metric_cell(format_byte_rate(write_rate)),
        metric_cell(format_count(record.counters.read_operations)),
        metric_cell(format_count(record.counters.write_operations)),
    ])
}

////////////////////////////////////////////////////////////////////////////////
/// Build one network row
fn network_row(record: &SystemNetworkRecord) -> Row<'_> {
    let rx_rate = record
        .rates
        .as_ref()
        .and_then(|rates| rates.rx_bytes_per_second);
    let tx_rate = record
        .rates
        .as_ref()
        .and_then(|rates| rates.tx_bytes_per_second);
    Row::new([
        Cell::from(record.name.as_str()),
        metric_cell(format_byte_rate(rx_rate)),
        metric_cell(format_byte_rate(tx_rate)),
        metric_cell(Some(format!(
            "{}/{}",
            display_value(format_count(Some(record.counters.rx_packets))),
            display_value(format_count(Some(record.counters.tx_packets)))
        ))),
        metric_cell(Some(format!(
            "{}/{}",
            display_value(format_count(Some(record.counters.rx_errors))),
            display_value(format_count(Some(record.counters.tx_errors)))
        ))),
        metric_cell(Some(format!(
            "{}/{}",
            display_value(format_count(Some(record.counters.rx_dropped))),
            display_value(format_count(Some(record.counters.tx_dropped)))
        ))),
    ])
}

////////////////////////////////////////////////////////////////////////////////
/// Build one pressure row
fn pressure_row(name: &'static str, record: Option<&PressureResourceRecord>) -> Row<'static> {
    Row::new([
        Cell::from(name),
        metric_cell(format_percent(
            record.map(|value| f64::from(value.some.avg10_percent)),
        )),
        metric_cell(format_percent(
            record.map(|value| f64::from(value.some.avg60_percent)),
        )),
        metric_cell(format_percent(
            record.map(|value| f64::from(value.some.avg300_percent)),
        )),
        metric_cell(format_percent(
            record.map(|value| f64::from(value.full.avg10_percent)),
        )),
        metric_cell(format_percent(
            record.map(|value| f64::from(value.full.avg60_percent)),
        )),
        metric_cell(format_percent(
            record.map(|value| f64::from(value.full.avg300_percent)),
        )),
    ])
}

////////////////////////////////////////////////////////////////////////////////
/// Build a system table header
fn system_table_header<const N: usize>(names: [&'static str; N]) -> Row<'static> {
    Row::new(names.into_iter().enumerate().map(|(index, name)| {
        if index == 0 {
            Cell::from(name)
        } else {
            Cell::from(Line::from(name).right_aligned())
        }
    }))
    .style(HEADER_STYLE)
}

////////////////////////////////////////////////////////////////////////////////
/// Build a system block
fn system_block(title: &'static str) -> Block<'static> {
    Block::default()
        .title(Span::styled(title, ACCENT_STYLE))
        .borders(Borders::ALL)
        .border_style(BORDER_STYLE)
}

////////////////////////////////////////////////////////////////////////////////
/// Highlight the active system table
fn system_list_block(title: &'static str, focused: bool) -> Block<'static> {
    system_block(title).border_style(if focused { ACCENT_STYLE } else { BORDER_STYLE })
}

////////////////////////////////////////////////////////////////////////////////
/// Show a missing metric
fn render_unavailable(frame: &mut Frame<'_>, area: Rect, title: &'static str, focused: bool) {
    frame.render_widget(
        Paragraph::new(Span::styled(UNAVAILABLE_VALUE, UNAVAILABLE_STYLE))
            .block(system_list_block(title, focused)),
        area,
    );
}

////////////////////////////////////////////////////////////////////////////////
/// Choose a color for a usage value
fn usage_style(percent: f64) -> Style {
    if percent >= CRITICAL_USAGE_PERCENT {
        CRITICAL_STYLE
    } else if percent >= WARNING_USAGE_PERCENT {
        WARNING_STYLE
    } else {
        HEALTHY_STYLE
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Draw a metric gauge
fn render_metric_gauge(frame: &mut Frame<'_>, area: Rect, percent: Option<f64>, style: Style) {
    let Some(percent) = percent else {
        frame.render_widget(
            Paragraph::new(Span::styled(UNAVAILABLE_VALUE, UNAVAILABLE_STYLE)),
            area,
        );
        return;
    };

    frame.render_widget(
        LineGauge::default()
            .ratio((percent / 100.0).clamp(0.0, 1.0))
            .label("")
            .filled_symbol("▮")
            .unfilled_symbol("▯")
            .filled_style(style)
            .unfilled_style(UNAVAILABLE_STYLE),
        area,
    );
}
