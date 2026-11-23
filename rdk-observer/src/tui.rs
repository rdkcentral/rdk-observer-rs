use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};
use rdk_observer_core::process_filter::ProcessFilter as CoreProcessFilter;
use rdk_observer_core::process_registry::{ProcessRegistry, RegisteredProcess};
use std::cmp::Ordering;
use std::io::Stdout;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};

const REFRESH_INTERVAL: Duration = Duration::from_millis(500);
const ACCENT_STYLE: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const ACTIVE_HEADER_STYLE: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const HEADER_STYLE: Style = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);
const HOTKEY_STYLE: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
const BORDER_STYLE: Style = Style::new().fg(Color::DarkGray);
const KERNEL_STYLE: Style = Style::new().fg(Color::Magenta);
const USERSPACE_STYLE: Style = Style::new().fg(Color::Green);
const UNAVAILABLE_STYLE: Style = Style::new().fg(Color::DarkGray);

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

struct TerminalUi {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    event_stream: EventStream,
    process_registry: ProcessRegistry,
    process_sort_column: ProcessSortColumn,
    process_filter: ProcessFilter,
}

#[tracing::instrument(name = "tui", skip_all)]
pub async fn run(process_registry: ProcessRegistry) -> anyhow::Result<()> {
    let mut interrupt_signal = signal(SignalKind::interrupt())?;
    let mut terminate_signal = signal(SignalKind::terminate())?;
    let mut terminal_ui = TerminalUi {
        terminal: Terminal::new(CrosstermBackend::new(std::io::stdout()))?,
        event_stream: EventStream::new(),
        process_registry,
        process_sort_column: ProcessSortColumn::Cpu,
        process_filter: ProcessFilter::All,
    };

    enable_raw_mode()?;
    execute!(terminal_ui.terminal.backend_mut(), EnterAlternateScreen)?;

    loop {
        terminal_ui.render()?;

        tokio::select! {
            keep_running = terminal_ui.handle_input_events() => {
                if !keep_running? {
                    break;
                }
            }
            _ = interrupt_signal.recv() => break,
            _ = terminate_signal.recv() => break,
        }
    }

    Ok(())
}

impl TerminalUi {
    fn render(&mut self) -> anyhow::Result<()> {
        self.terminal.autoresize()?;

        let mut frame = self.terminal.get_frame();

        let [process_area, totals_area, controls_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(5),
                Constraint::Length(3),
            ])
            .areas(frame.area());

        let mut processes = self.process_registry.get_snapshot();
        let total_process_count = processes.len();
        let (kernel_totals, userspace_totals, rdk_totals) = calculate_process_totals(&processes);
        filter_processes(&mut processes, self.process_filter);
        sort_processes(&mut processes, self.process_sort_column);

        Self::render_processes(
            &mut frame,
            process_area,
            &processes,
            total_process_count,
            self.process_sort_column,
            self.process_filter,
        );
        Self::render_totals(
            &mut frame,
            totals_area,
            &kernel_totals,
            &userspace_totals,
            &rdk_totals,
        );
        Self::render_controls(&mut frame, controls_area);

        self.terminal.apply_buffer()?;

        Ok(())
    }

    fn render_processes(
        frame: &mut Frame<'_>,
        area: Rect,
        processes: &[RegisteredProcess],
        total_process_count: usize,
        process_sort_column: ProcessSortColumn,
        process_filter: ProcessFilter,
    ) {
        let rows = processes.iter().map(process_row);
        let title = Line::from(vec![
            Span::styled(
                format!(
                    " PROCESSES: {} / {total_process_count} | Filter: ",
                    processes.len(),
                ),
                ACCENT_STYLE,
            ),
            Span::styled(format!("[{}]", process_filter.name()), ACCENT_STYLE),
            Span::styled(" ", ACCENT_STYLE),
        ]);

        let table = Table::new(rows, process_table_constraints())
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
            .column_spacing(2);

        frame.render_widget(table, area);
    }

    fn render_controls(frame: &mut Frame<'_>, area: Rect) {
        let controls = Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled("[q/Esc]", HOTKEY_STYLE),
            Span::raw(" Quit   "),
            Span::styled("[a]", HOTKEY_STYLE),
            Span::raw(" All  "),
            Span::styled("[k]", HOTKEY_STYLE),
            Span::raw(" Kernel  "),
            Span::styled("[u]", HOTKEY_STYLE),
            Span::raw(" Userspace  "),
            Span::styled("[r]", HOTKEY_STYLE),
            Span::raw(" RDK   "),
            Span::styled("[ ← / → ]", HOTKEY_STYLE),
            Span::raw(" Sort column"),
        ]))
        .block(
            Block::default()
                .title(Span::styled(" CONTROLS ", ACCENT_STYLE))
                .borders(Borders::ALL)
                .border_style(BORDER_STYLE),
        );

        frame.render_widget(controls, area);
    }

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

        let table = Table::new(rows, process_table_constraints())
            .block(
                Block::default()
                    .title(Span::styled(" TOTALS ", ACCENT_STYLE))
                    .borders(Borders::ALL)
                    .border_style(BORDER_STYLE),
            )
            .column_spacing(2);

        frame.render_widget(table, area);
    }

    async fn handle_input_events(&mut self) -> anyhow::Result<bool> {
        let event_future = self.event_stream.next();
        let event_timeout = tokio::time::timeout(REFRESH_INTERVAL, event_future);

        let Ok(event) = event_timeout.await else {
            return Ok(true);
        };

        let Some(event) = event else {
            return Ok(false);
        };

        let Event::Key(event) = event? else {
            return Ok(true);
        };

        if event.kind != KeyEventKind::Press {
            return Ok(true);
        }

        if matches!(
            event.code,
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc
        ) || (event.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(event.code, KeyCode::Char('c') | KeyCode::Char('C')))
        {
            return Ok(false);
        }

        match event.code {
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.process_filter = ProcessFilter::All;
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                self.process_filter = ProcessFilter::Kernel;
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                self.process_filter = ProcessFilter::Userspace;
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.process_filter = ProcessFilter::Rdk;
            }
            KeyCode::Left => {
                self.process_sort_column = self.process_sort_column.previous();
            }
            KeyCode::Right => {
                self.process_sort_column = self.process_sort_column.next();
            }
            _ => {}
        }

        Ok(true)
    }
}

impl Drop for TerminalUi {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
    }
}

fn process_row(process: &RegisteredProcess) -> Row<'static> {
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
        Cell::from(process.command().to_string()),
        Cell::from(process_type(process)).style(process_type_style),
        metric_cell(format_cpu(cpu_utilization(process))),
        metric_cell(format_bytes(virtual_memory.map(|value| value as f64))),
        metric_cell(format_bytes(resident_memory.map(|value| value as f64))),
        metric_cell(format_bytes(proportional_memory.map(|value| value as f64))),
        metric_cell(format_rate(read_rate)),
        metric_cell(format_rate(write_rate)),
        metric_cell(process.key().start_time.to_string()),
    ])
}

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

fn totals_row(
    process_type: &'static str,
    totals: &ProcessTotals,
    label_style: Style,
) -> Row<'static> {
    Row::new([
        Cell::from(""),
        Cell::from(format!("{process_type} total")).style(label_style),
        Cell::from(""),
        metric_cell(format_cpu(totals.cpu)),
        metric_cell(format_bytes(
            totals.virtual_memory.map(|value| value as f64),
        )),
        metric_cell(format_bytes(
            totals.resident_memory.map(|value| value as f64),
        )),
        metric_cell(format_bytes(
            totals.proportional_memory.map(|value| value as f64),
        )),
        metric_cell(format_rate(totals.read_rate)),
        metric_cell(format_rate(totals.write_rate)),
        Cell::from(""),
    ])
}

fn metric_cell(value: String) -> Cell<'static> {
    let style = if value == "-" {
        UNAVAILABLE_STYLE
    } else {
        Style::default()
    };

    Cell::from(Line::from(value).right_aligned()).style(style)
}

fn process_table_constraints() -> [Constraint; 10] {
    [
        Constraint::Length(8),
        Constraint::Min(24),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Length(16),
    ]
}

fn add_optional_value(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or_default() + value);
    }
}

fn add_optional_float(total: &mut Option<f64>, value: Option<f64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or_default() + value);
    }
}

fn filter_processes(processes: &mut Vec<RegisteredProcess>, process_filter: ProcessFilter) {
    processes.retain(|process| match process_filter {
        ProcessFilter::All => true,
        ProcessFilter::Kernel => process.kernel_thread(),
        ProcessFilter::Userspace => !process.kernel_thread(),
        ProcessFilter::Rdk => CoreProcessFilter::is_rdk_process(process.command()),
    });
}

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

fn cpu_utilization(process: &RegisteredProcess) -> Option<f64> {
    process
        .last_cpu_record
        .as_ref()
        .and_then(|record| record.utilization)
        .map(|value| value.percent)
}

fn process_type(process: &RegisteredProcess) -> &'static str {
    if process.kernel_thread() {
        "kernel"
    } else {
        "userspace"
    }
}

fn process_virtual_memory(process: &RegisteredProcess) -> Option<u64> {
    process
        .last_memory_record
        .as_ref()
        .and_then(|record| record.virtual_bytes)
}

fn process_resident_memory(process: &RegisteredProcess) -> Option<u64> {
    process
        .last_memory_record
        .as_ref()
        .and_then(|record| record.resident_bytes)
}

fn process_proportional_memory(process: &RegisteredProcess) -> Option<u64> {
    process
        .last_memory_record
        .as_ref()
        .and_then(|record| record.proportional_bytes)
}

fn process_read_rate(process: &RegisteredProcess) -> Option<f64> {
    process
        .last_io_record
        .as_ref()
        .and_then(|record| record.rates.as_ref())
        .and_then(|rates| rates.read_bytes_per_second)
}

fn process_write_rate(process: &RegisteredProcess) -> Option<f64> {
    process
        .last_io_record
        .as_ref()
        .and_then(|record| record.rates.as_ref())
        .and_then(|rates| rates.write_bytes_per_second)
}

fn format_cpu(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), |value| format!("{value:.1}%"))
}

fn format_bytes(value: Option<f64>) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let Some(value) = value else {
        return "-".to_string();
    };

    if value >= GIB {
        format!("{:.1} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{:.1} B", value)
    }
}

fn format_rate(value: Option<f64>) -> String {
    let Some(value) = value else {
        return "-".to_string();
    };

    format!("{}/s", format_bytes(Some(value)))
}

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

fn compare_optional_floats(left: Option<f64>, right: Option<f64>) -> Ordering {
    let left = left.unwrap_or(f64::NEG_INFINITY);
    let right = right.unwrap_or(f64::NEG_INFINITY);

    right.total_cmp(&left)
}
