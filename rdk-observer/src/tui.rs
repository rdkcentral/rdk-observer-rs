//! Terminal UI for process and system metrics
//!
//! The TUI displays data from the process and system registries

mod common;
mod process_screen;
mod system_screen;

use common::{ACCENT_STYLE, ACTIVE_HEADER_STYLE, BORDER_STYLE};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEvent,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use process_screen::ProcessScreen;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Tabs};
use ratatui::{Frame, Terminal};
use rdk_observer_core::runtime::RuntimeContext;
use std::io::Stdout;
use std::time::Duration;
use system_screen::SystemScreen;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{Instant, timeout_at};

////////////////////////////////////////////////////////////////////////////////
// Layout

const REFRESH_INTERVAL: Duration = Duration::from_millis(500);
const TABS_HEIGHT: u16 = 3;
const MIN_CONTENT_HEIGHT: u16 = 5;

////////////////////////////////////////////////////////////////////////////////
// TUI state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiView {
    Processes,
    System,
}

impl TuiView {
    ////////////////////////////////////////////////////////////////////////////////
    fn toggle(self) -> Self {
        match self {
            Self::Processes => Self::System,
            Self::System => Self::Processes,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn index(self) -> usize {
        match self {
            Self::Processes => 0,
            Self::System => 1,
        }
    }
}

/// TUI coordinator
struct TerminalUi {
    event_stream: EventStream,
    context: RuntimeContext,
    view: TuiView,
    process_screen: ProcessScreen,
    system_screen: SystemScreen,
}

/// Terminal setup and cleanup
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

////////////////////////////////////////////////////////////////////////////////
/// Run the TUI until exit
#[tracing::instrument(name = "tui", skip_all)]
pub async fn run(context: RuntimeContext) -> anyhow::Result<()> {
    let mut interrupt_signal = signal(SignalKind::interrupt())?;
    let mut terminate_signal = signal(SignalKind::terminate())?;
    let mut terminal_session = TerminalSession::new()?;
    let mut terminal_ui = TerminalUi::new(context);

    loop {
        terminal_session.render(&mut terminal_ui)?;

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

impl TerminalSession {
    ////////////////////////////////////////////////////////////////////////////////
    /// Set up the terminal
    fn new() -> anyhow::Result<Self> {
        let mut session = Self {
            terminal: Terminal::new(CrosstermBackend::new(std::io::stdout()))?,
        };

        enable_raw_mode()?;
        execute!(
            session.terminal.backend_mut(),
            EnterAlternateScreen,
            EnableMouseCapture
        )?;

        Ok(session)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw one frame
    fn render(&mut self, ui: &mut TerminalUi) -> anyhow::Result<()> {
        self.terminal.autoresize()?;
        let mut frame = self.terminal.get_frame();
        ui.render(&mut frame);
        self.terminal.apply_buffer()?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    ////////////////////////////////////////////////////////////////////////////////
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}

impl TerminalUi {
    ////////////////////////////////////////////////////////////////////////////////
    /// Create the initial TUI state
    fn new(context: RuntimeContext) -> Self {
        Self {
            event_stream: EventStream::new(),
            context,
            view: TuiView::Processes,
            process_screen: ProcessScreen::default(),
            system_screen: SystemScreen::default(),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw the current screen
    fn render(&mut self, frame: &mut Frame<'_>) {
        let controls_height = match self.view {
            TuiView::Processes => process_screen::CONTROLS_HEIGHT,
            TuiView::System => system_screen::CONTROLS_HEIGHT,
        };
        let [tabs_area, content_area, controls_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(TABS_HEIGHT),
                Constraint::Min(MIN_CONTENT_HEIGHT),
                Constraint::Length(controls_height),
            ])
            .areas(frame.area());

        self.render_tabs(frame, tabs_area);

        match self.view {
            TuiView::Processes => {
                self.process_screen
                    .render(frame, content_area, controls_area, &self.context);
            }
            TuiView::System => {
                self.system_screen
                    .render(frame, content_area, controls_area, &self.context);
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Wait for input or the next redraw
    async fn handle_input_events(&mut self) -> anyhow::Result<bool> {
        // Keep one deadline so ignored mouse events cannot delay the periodic refresh
        let refresh_deadline = Instant::now() + REFRESH_INTERVAL;

        loop {
            let event = match timeout_at(refresh_deadline, self.event_stream.next()).await {
                Ok(Some(event)) => event?,
                Ok(None) => return Ok(false),
                Err(_) => return Ok(true),
            };

            match event {
                Event::Key(event) if event.kind == KeyEventKind::Press => {
                    return Ok(self.handle_key_event(event));
                }
                Event::Mouse(event) => {
                    // Ignore mouse events that do not change table state
                    if self.handle_mouse_event(event) {
                        return Ok(true);
                    }
                }
                _ => return Ok(true),
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Handle global keys and pass other keys to the current screen
    fn handle_key_event(&mut self, event: KeyEvent) -> bool {
        if matches!(event.code, KeyCode::Char('q' | 'Q') | KeyCode::Esc)
            || (event.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(event.code, KeyCode::Char('c' | 'C')))
        {
            return false;
        }

        if event.code == KeyCode::Tab {
            self.view = self.view.toggle();
            return true;
        }

        match self.view {
            TuiView::Processes => self.process_screen.handle_key_event(event.code),
            TuiView::System => self.system_screen.handle_key_event(event.code),
        }

        true
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Pass mouse input to the current screen
    fn handle_mouse_event(&mut self, event: MouseEvent) -> bool {
        match self.view {
            TuiView::Processes => self.process_screen.handle_mouse_event(event),
            TuiView::System => self.system_screen.handle_mouse_event(event),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Draw the screen tabs
    fn render_tabs(&self, frame: &mut Frame<'_>, area: Rect) {
        let tabs = Tabs::new(["Processes", "System"])
            .select(self.view.index())
            .highlight_style(ACTIVE_HEADER_STYLE)
            .divider(" | ")
            .block(
                Block::default()
                    .title(Span::styled(" RDK OBSERVER ", ACCENT_STYLE))
                    .borders(Borders::ALL)
                    .border_style(BORDER_STYLE),
            );

        frame.render_widget(tabs, area);
    }
}
