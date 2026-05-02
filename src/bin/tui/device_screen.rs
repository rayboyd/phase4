//! Device selection screen, displayed before the pipeline starts.

use phase4::managers::audio::Input;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

/// Holds the mutable state for the device selection screen.
pub struct DeviceScreen {
    pub devices: Vec<(usize, String, u32, u16)>,
    pub list_state: ListState,
}

impl DeviceScreen {
    /// Loads the device list from the host audio system.
    ///
    /// # Errors
    ///
    /// Returns an error if device enumeration fails.
    pub fn load() -> anyhow::Result<Self> {
        let devices = Input::enumerate_devices()?;
        let mut list_state = ListState::default();
        if !devices.is_empty() {
            list_state.select(Some(0));
        }
        Ok(Self {
            devices,
            list_state,
        })
    }

    /// Returns the index of the currently selected device, if any.
    #[must_use]
    pub fn selected_device_index(&self) -> Option<usize> {
        self.list_state
            .selected()
            .and_then(|i| self.devices.get(i))
            .map(|(idx, _, _, _)| *idx)
    }

    /// Moves the selection one row towards the top of the list.
    pub fn move_up(&mut self) {
        if self.devices.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(i.saturating_sub(1)));
    }

    /// Moves the selection one row towards the bottom of the list.
    pub fn move_down(&mut self) {
        if self.devices.is_empty() {
            return;
        }
        let last = self.devices.len() - 1;
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some((i + 1).min(last)));
    }
}

/// Renders the device selection screen into the given frame.
pub fn render(frame: &mut Frame, screen: &mut DeviceScreen) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(frame.area());

    let items: Vec<ListItem> = if screen.devices.is_empty() {
        vec![ListItem::new(Line::from(
            "No compatible input devices found.",
        ))]
    } else {
        screen
            .devices
            .iter()
            .map(|(idx, name, rate, ch)| {
                ListItem::new(Line::from(format!("[{idx}]  {name}  ({rate} Hz, {ch}ch)")))
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Phase4 - Select input device "),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let hints = Paragraph::new(Line::from(
        "  Up/Down: select   Enter: start   Q / Ctrl+C: quit",
    ))
    .block(Block::default().borders(Borders::ALL));

    frame.render_stateful_widget(list, chunks[0], &mut screen.list_state);
    frame.render_widget(hints, chunks[1]);
}
