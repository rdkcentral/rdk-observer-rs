//! Process metrics screen

use super::common::{
    ACCENT_STYLE, ACTIVE_HEADER_STYLE, BORDER_STYLE, HEADER_STYLE, HOTKEY_STYLE,
    SELECTED_ROW_STYLE, ScrollDirection, ScrollableTableState, common_controls, format_byte_rate,
    format_bytes, format_percent, metric_cell, mouse_scroll_direction, render_scrollable_table,
};
use crossterm::event::{KeyCode, MouseEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use rdk_observer_core::process_filter::ProcessFilter as CoreProcessFilter;
use rdk_observer_core::process_registry::RegisteredProcess;
use rdk_observer_core::runtime::RuntimeContext;
use std::cmp::Ordering;

////////////////////////////////////////////////////////////////////////////////
// Layout and styles

pub const CONTROLS_HEIGHT: u16 = 4;

const MIN_CONTENT_HEIGHT: u16 = 5;
const TOTALS_HEIGHT: u16 = 5;
const TABLE_WIDTHS: [Constraint; 10] = [
    Constraint::Length(8),  // PID
    Constraint::Min(24),    // COMMAND
    Constraint::Length(12), // TYPE
    Constraint::Length(10), // CPU
    Constraint::Length(14), // VIRT
    Constraint::Length(14), // RSS
    Constraint::Length(14), // PSS
    Constraint::Length(14), // READ
    Constraint::Length(14), // WRITE
    Constraint::Length(16), // START TIME
];
const KERNEL_STYLE: Style = Style::new().fg(Color::Magenta);
const USERSPACE_STYLE: Style = Style::new().fg(Color::Green);

////////////////////////////////////////////////////////////////////////////////
// Process screen state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessSortColumn {
    Cpu,
    VirtualMemory,
    ResidentMemory,
    ProportionalMemory,
    ReadRate,
    WriteRate,
    StartTime,
}

impl ProcessSortColumn {
    ////////////////////////////////////////////////////////////////////////////////
    fn next(self) -> Self {
        match self {
            Self::Cpu => Self::VirtualMemory,
            Self::VirtualMemory => Self::ResidentMemory,
            Self::ResidentMemory => Self::ProportionalMemory,
            Self::ProportionalMemory => Self::ReadRate,
            Self::ReadRate => Self::WriteRate,
            Self::WriteRate => Self::StartTime,
            Self::StartTime => Self::Cpu,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn previous(self) -> Self {
        match self {
            Self::Cpu => Self::StartTime,
            Self::VirtualMemory => Self::Cpu,
            Self::ResidentMemory => Self::VirtualMemory,
            Self::ProportionalMemory => Self::ResidentMemory,
            Self::ReadRate => Self::ProportionalMemory,
            Self::WriteRate => Self::ReadRate,
            Self::StartTime => Self::WriteRate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessFilter {
    All,
    Kernel,
    Userspace,
    Rdk,
}

impl ProcessFilter {
    ////////////////////////////////////////////////////////////////////////////////
    fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Kernel => "kernel",
            Self::Userspace => "userspace",
            Self::Rdk => "rdk",
        }
    }
}

#[derive(Default)]
struct ProcessTotals {
    cpu: Option<f64>,
    virtual_memory: Option<u64>,
    resident_memory: Option<u64>,
    proportional_memory: Option<u64>,
    read_rate: Option<f64>,
    write_rate: Option<f64>,
}

pub struct ProcessScreen {
    table: ScrollableTableState,
    sort_column: ProcessSortColumn,
    filter: ProcessFilter,
}

impl Default for ProcessScreen {
    ////////////////////////////////////////////////////////////////////////////////
    fn default() -> Self {
        Self {
            table: ScrollableTableState::default(),
            sort_column: ProcessSortColumn::Cpu,
            filter: ProcessFilter::All,
        }
    }
}

impl ProcessScreen {
    ////////////////////////////////////////////////////////////////////////////////
    /// Handle process screen keys
    pub fn handle_key_event(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('a' | 'A') => self.set_filter(ProcessFilter::All),
            KeyCode::Char('k' | 'K') => self.set_filter(ProcessFilter::Kernel),
            KeyCode::Char('u' | 'U') => self.set_filter(ProcessFilter::Userspace),
            KeyCode::Char('r' | 'R') => self.set_filter(ProcessFilter::Rdk),
            KeyCode::Left => self.sort_column = self.sort_column.previous(),
            KeyCode::Right => self.sort_column = self.sort_column.next(),
            KeyCode::Up => self.table.scroll(ScrollDirection::Up),
            KeyCode::Down => self.table.scroll(ScrollDirection::Down),
            _ => {}
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Handle process table mouse scrolling
    pub fn handle_mouse_event(&mut self, event: MouseEvent) -> bool {
        let Some(direction) = mouse_scroll_direction(event) else {
            return false;
        };
        if !self.table.contains(event) {
            return false;
        }

        self.table.scroll(direction);
        true
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw process metrics and controls
    pub fn render(
        &mut self,
        frame: &mut Frame<'_>,
        content_area: Rect,
        controls_area: Rect,
        context: &RuntimeContext,
    ) {
        let mut processes = context.process_registry.get_snapshot();
        let total_process_count = processes.len();
        let (kernel_totals, userspace_totals, rdk_totals) = calculate_process_totals(&processes);
        filter_processes(&mut processes, self.filter);
        sort_processes(&mut processes, self.sort_column);

        let [process_area, totals_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(MIN_CONTENT_HEIGHT),
                Constraint::Length(TOTALS_HEIGHT),
            ])
            .areas(content_area);

        self.render_processes(frame, process_area, &processes, total_process_count);
        render_totals(
            frame,
            totals_area,
            &kernel_totals,
            &userspace_totals,
            &rdk_totals,
        );
        self.render_controls(frame, controls_area);
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn set_filter(&mut self, filter: ProcessFilter) {
        self.filter = filter;
        self.table.reset_selection();
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw the process table
    fn render_processes(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        processes: &[RegisteredProcess],
        total_process_count: usize,
    ) {
        let process_sort_column = self.sort_column;
        let rows = processes.iter().map(process_row);
        let title = Line::from(vec![
            Span::styled(
                format!(
                    " PROCESSES: {} / {total_process_count} | Filter: ",
                    processes.len(),
                ),
                ACCENT_STYLE,
            ),
            Span::styled(format!("[{}]", self.filter.name()), ACCENT_STYLE),
            Span::styled(" ", ACCENT_STYLE),
        ]);

        let table = Table::new(rows, TABLE_WIDTHS)
            .header(Row::new([
                Cell::from(Line::from("PID").right_aligned()).style(HEADER_STYLE),
                header_cell("COMMAND", None, process_sort_column),
                header_cell("TYPE", None, process_sort_column),
                header_cell("CPU", Some(ProcessSortColumn::Cpu), process_sort_column),
                header_cell(
                    "VIRT",
                    Some(ProcessSortColumn::VirtualMemory),
                    process_sort_column,
                ),
                header_cell(
                    "RSS",
                    Some(ProcessSortColumn::ResidentMemory),
                    process_sort_column,
                ),
                header_cell(
                    "PSS",
                    Some(ProcessSortColumn::ProportionalMemory),
                    process_sort_column,
                ),
                header_cell(
                    "READ",
                    Some(ProcessSortColumn::ReadRate),
                    process_sort_column,
                ),
                header_cell(
                    "WRITE",
                    Some(ProcessSortColumn::WriteRate),
                    process_sort_column,
                ),
                header_cell(
                    "START TIME",
                    Some(ProcessSortColumn::StartTime),
                    process_sort_column,
                ),
            ]))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(BORDER_STYLE),
            )
            .row_highlight_style(SELECTED_ROW_STYLE)
            .column_spacing(2);

        render_scrollable_table(frame, area, table, &mut self.table, processes.len());
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw process controls
    fn render_controls(&self, frame: &mut Frame<'_>, area: Rect) {
        let mut table_controls = common_controls();
        table_controls.extend([
            Span::styled("[ ← / → ]", HOTKEY_STYLE),
            Span::raw(" Sort column   "),
            Span::styled("[ ↑ / ↓ / Wheel ]", HOTKEY_STYLE),
            Span::raw(" Scroll"),
        ]);

        let controls = Paragraph::new(vec![
            Line::from(table_controls),
            Line::from(vec![
                Span::raw(" "),
                Span::raw("Filter: "),
                Span::styled("[a]", HOTKEY_STYLE),
                Span::raw(" All   "),
                Span::styled("[k]", HOTKEY_STYLE),
                Span::raw(" Kernel   "),
                Span::styled("[u]", HOTKEY_STYLE),
                Span::raw(" Userspace   "),
                Span::styled("[r]", HOTKEY_STYLE),
                Span::raw(" RDK"),
            ]),
        ])
        .block(
            Block::default()
                .title(Span::styled(" CONTROLS ", ACCENT_STYLE))
                .borders(Borders::ALL)
                .border_style(BORDER_STYLE),
        );

        frame.render_widget(controls, area);
    }
}

////////////////////////////////////////////////////////////////////////////////
// Process rendering

/// Draw process totals
fn render_totals(
    frame: &mut Frame<'_>,
    area: Rect,
    kernel_totals: &ProcessTotals,
    userspace_totals: &ProcessTotals,
    rdk_totals: &ProcessTotals,
) {
    let rows = [
        totals_row(
            "kernel",
            kernel_totals,
            KERNEL_STYLE.add_modifier(Modifier::BOLD),
        ),
        totals_row(
            "userspace",
            userspace_totals,
            USERSPACE_STYLE.add_modifier(Modifier::BOLD),
        ),
        totals_row("rdk", rdk_totals, HOTKEY_STYLE),
    ];

    let table = Table::new(rows, TABLE_WIDTHS)
        .block(
            Block::default()
                .title(Span::styled(" TOTALS ", ACCENT_STYLE))
                .borders(Borders::ALL)
                .border_style(BORDER_STYLE),
        )
        .column_spacing(2);

    frame.render_widget(table, area);
}

////////////////////////////////////////////////////////////////////////////////
/// Build one process row
fn process_row(process: &RegisteredProcess) -> Row<'_> {
    let virtual_memory = process_virtual_memory(process);
    let resident_memory = process_resident_memory(process);
    let proportional_memory = process_proportional_memory(process);
    let read_rate = process_read_rate(process);
    let write_rate = process_write_rate(process);
    let process_type_style = if process.kernel_thread() {
        KERNEL_STYLE
    } else {
        USERSPACE_STYLE
    };

    Row::new([
        Cell::from(Line::from(process.key().pid.to_string()).right_aligned()),
        Cell::from(process.command()),
        Cell::from(process_type(process)).style(process_type_style),
        metric_cell(format_percent(cpu_utilization(process))),
        metric_cell(format_bytes(virtual_memory)),
        metric_cell(format_bytes(resident_memory)),
        metric_cell(format_bytes(proportional_memory)),
        metric_cell(format_byte_rate(read_rate)),
        metric_cell(format_byte_rate(write_rate)),
        metric_cell(Some(process.key().start_time.to_string())),
    ])
}

////////////////////////////////////////////////////////////////////////////////
/// Build one totals row
fn totals_row(
    process_type: &'static str,
    totals: &ProcessTotals,
    label_style: Style,
) -> Row<'static> {
    Row::new([
        Cell::from(""),
        Cell::from(format!("{process_type} total")).style(label_style),
        Cell::from(""),
        metric_cell(format_percent(totals.cpu)),
        metric_cell(format_bytes(totals.virtual_memory)),
        metric_cell(format_bytes(totals.resident_memory)),
        metric_cell(format_bytes(totals.proportional_memory)),
        metric_cell(format_byte_rate(totals.read_rate)),
        metric_cell(format_byte_rate(totals.write_rate)),
        Cell::from(""),
    ])
}

////////////////////////////////////////////////////////////////////////////////
/// Build a process table header cell
fn header_cell(
    name: &'static str,
    column: Option<ProcessSortColumn>,
    process_sort_column: ProcessSortColumn,
) -> Cell<'static> {
    let (name, style) = if column == Some(process_sort_column) {
        (Line::from(format!("[{name}]")), ACTIVE_HEADER_STYLE)
    } else {
        (Line::from(name), HEADER_STYLE)
    };

    if column.is_some() {
        Cell::from(name.right_aligned()).style(style)
    } else {
        Cell::from(name).style(style)
    }
}

////////////////////////////////////////////////////////////////////////////////
// Process data

/// Calculate process totals
fn calculate_process_totals(
    processes: &[RegisteredProcess],
) -> (ProcessTotals, ProcessTotals, ProcessTotals) {
    let mut kernel_totals = ProcessTotals::default();
    let mut userspace_totals = ProcessTotals::default();
    let mut rdk_totals = ProcessTotals::default();

    for process in processes {
        let process_totals = if process.kernel_thread() {
            &mut kernel_totals
        } else {
            &mut userspace_totals
        };

        add_process_to_totals(process_totals, process);

        if CoreProcessFilter::is_rdk_process(process.command()) {
            add_process_to_totals(&mut rdk_totals, process);
        }
    }

    (kernel_totals, userspace_totals, rdk_totals)
}

////////////////////////////////////////////////////////////////////////////////
/// Add one process to the totals
fn add_process_to_totals(totals: &mut ProcessTotals, process: &RegisteredProcess) {
    add_optional_float(&mut totals.cpu, cpu_utilization(process));
    add_optional_value(&mut totals.virtual_memory, process_virtual_memory(process));
    add_optional_value(
        &mut totals.resident_memory,
        process_resident_memory(process),
    );
    add_optional_value(
        &mut totals.proportional_memory,
        process_proportional_memory(process),
    );
    add_optional_float(&mut totals.read_rate, process_read_rate(process));
    add_optional_float(&mut totals.write_rate, process_write_rate(process));
}

////////////////////////////////////////////////////////////////////////////////
fn add_optional_value(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or_default() + value);
    }
}

////////////////////////////////////////////////////////////////////////////////
fn add_optional_float(total: &mut Option<f64>, value: Option<f64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or_default() + value);
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Filter the process list
fn filter_processes(processes: &mut Vec<RegisteredProcess>, process_filter: ProcessFilter) {
    processes.retain(|process| match process_filter {
        ProcessFilter::All => true,
        ProcessFilter::Kernel => process.kernel_thread(),
        ProcessFilter::Userspace => !process.kernel_thread(),
        ProcessFilter::Rdk => CoreProcessFilter::is_rdk_process(process.command()),
    });
}

////////////////////////////////////////////////////////////////////////////////
/// Sort the process list
fn sort_processes(processes: &mut [RegisteredProcess], process_sort_column: ProcessSortColumn) {
    processes.sort_by(|left, right| {
        let ordering = match process_sort_column {
            ProcessSortColumn::Cpu => {
                compare_optional_floats(cpu_utilization(left), cpu_utilization(right))
            }
            ProcessSortColumn::VirtualMemory => {
                process_virtual_memory(right).cmp(&process_virtual_memory(left))
            }
            ProcessSortColumn::ResidentMemory => {
                process_resident_memory(right).cmp(&process_resident_memory(left))
            }
            ProcessSortColumn::ProportionalMemory => {
                process_proportional_memory(right).cmp(&process_proportional_memory(left))
            }
            ProcessSortColumn::ReadRate => {
                compare_optional_floats(process_read_rate(left), process_read_rate(right))
            }
            ProcessSortColumn::WriteRate => {
                compare_optional_floats(process_write_rate(left), process_write_rate(right))
            }
            ProcessSortColumn::StartTime => right.key().start_time.cmp(&left.key().start_time),
        };

        ordering.then_with(|| left.key().cmp(&right.key()))
    });
}

////////////////////////////////////////////////////////////////////////////////
fn compare_optional_floats(left: Option<f64>, right: Option<f64>) -> Ordering {
    // Put missing values at the end
    let left = left.unwrap_or(f64::NEG_INFINITY);
    let right = right.unwrap_or(f64::NEG_INFINITY);

    right.total_cmp(&left)
}

////////////////////////////////////////////////////////////////////////////////
// Process metrics

fn cpu_utilization(process: &RegisteredProcess) -> Option<f64> {
    process
        .last_cpu_record
        .as_ref()
        .and_then(|record| record.1.utilization)
        .map(|value| value.percent)
}

////////////////////////////////////////////////////////////////////////////////
fn process_type(process: &RegisteredProcess) -> &'static str {
    if process.kernel_thread() {
        "kernel"
    } else {
        "userspace"
    }
}

////////////////////////////////////////////////////////////////////////////////
fn process_virtual_memory(process: &RegisteredProcess) -> Option<u64> {
    process
        .last_memory_record
        .as_ref()
        .and_then(|record| record.1.virtual_bytes)
}

////////////////////////////////////////////////////////////////////////////////
fn process_resident_memory(process: &RegisteredProcess) -> Option<u64> {
    process
        .last_memory_record
        .as_ref()
        .and_then(|record| record.1.resident_bytes)
}

////////////////////////////////////////////////////////////////////////////////
fn process_proportional_memory(process: &RegisteredProcess) -> Option<u64> {
    process
        .last_memory_record
        .as_ref()
        .and_then(|record| record.1.proportional_bytes)
}

////////////////////////////////////////////////////////////////////////////////
fn process_read_rate(process: &RegisteredProcess) -> Option<f64> {
    process
        .last_io_record
        .as_ref()
        .and_then(|record| record.1.rates.as_ref())
        .and_then(|rates| rates.read_bytes_per_second)
}

////////////////////////////////////////////////////////////////////////////////
fn process_write_rate(process: &RegisteredProcess) -> Option<f64> {
    process
        .last_io_record
        .as_ref()
        .and_then(|record| record.1.rates.as_ref())
        .and_then(|rates| rates.write_bytes_per_second)
}
