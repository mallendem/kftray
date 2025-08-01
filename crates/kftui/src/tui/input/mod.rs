pub mod file_explorer;
pub mod navigation;
mod popup;

use std::collections::HashSet;
use std::io;
use std::sync::atomic::{
    AtomicBool,
    Ordering,
};
use std::sync::Arc;

use crossterm::event::{
    self,
    Event,
    KeyCode,
    KeyModifiers,
};
use crossterm::terminal::size;
pub use file_explorer::*;
use kftray_commons::models::{
    config_model::Config,
    config_state_model::ConfigState,
};
use kftray_commons::utils::db_mode::DatabaseMode;
pub use popup::*;
use ratatui::widgets::ListState;
use ratatui::widgets::TableState;
use ratatui_explorer::{
    FileExplorer,
    Theme,
};
use tui_logger::TuiWidgetEvent;
use tui_logger::TuiWidgetState;

use crate::core::port_forward::stop_all_port_forward_and_exit;
use crate::logging::LoggerState;
use crate::tui::input::navigation::handle_auto_add_configs;
use crate::tui::input::navigation::handle_context_selection;
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum DeleteButton {
    Confirm,
    Close,
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum ActiveComponent {
    Menu,
    StoppedTable,
    RunningTable,
    Details,
    Logs,
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum ActiveTable {
    Stopped,
    Running,
}

#[derive(PartialEq, Debug)]
pub enum AppState {
    Normal,
    ShowErrorPopup,
    ShowConfirmationPopup,
    ImportFileExplorerOpen,
    ExportFileExplorerOpen,
    ShowInputPrompt,
    ShowHelp,
    ShowAbout,
    ShowDeleteConfirmation,
    ShowContextSelection,
    ShowSettings,
}

pub struct App {
    pub details_scroll_offset: usize,
    pub details_scroll_max_offset: usize,
    pub selected_rows_stopped: HashSet<usize>,
    pub selected_rows_running: HashSet<usize>,
    pub import_file_explorer: FileExplorer,
    pub export_file_explorer: FileExplorer,
    pub state: AppState,
    pub selected_row_stopped: usize,
    pub selected_row_running: usize,
    pub active_table: ActiveTable,
    pub import_export_message: Option<String>,
    pub input_buffer: String,
    pub selected_file_path: Option<std::path::PathBuf>,
    pub file_content: Option<String>,
    pub stopped_configs: Vec<Config>,
    pub running_configs: Vec<Config>,
    pub error_message: Option<String>,
    pub active_component: ActiveComponent,
    pub selected_menu_item: usize,
    pub delete_confirmation_message: Option<String>,
    pub selected_delete_button: DeleteButton,
    pub visible_rows: usize,
    pub table_state_stopped: TableState,
    pub table_state_running: TableState,
    pub contexts: Vec<String>,
    pub selected_context_index: usize,
    pub context_list_state: ListState,
    pub tui_logger_state: TuiWidgetState,
    pub logger_state: LoggerState,
    pub settings_timeout_input: String,
    pub settings_editing: bool,
    pub settings_network_monitor: bool,
    pub settings_selected_option: usize,
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    pub configs_being_processed:
        std::collections::HashMap<i64, (Arc<AtomicBool>, std::time::Instant)>,
    pub error_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    pub error_sender: Option<tokio::sync::mpsc::UnboundedSender<String>>,
}

impl Default for App {
    fn default() -> Self {
        panic!("App::default() should not be used. Use App::new(LoggerState) instead.");
    }
}

impl App {
    pub fn new(logger_state: LoggerState) -> Self {
        let theme = Theme::default().add_default_title();
        let import_file_explorer = FileExplorer::with_theme(theme.clone()).unwrap();
        let export_file_explorer = FileExplorer::with_theme(theme).unwrap();
        let tui_logger_state = TuiWidgetState::new();
        let (error_sender, error_receiver) = tokio::sync::mpsc::unbounded_channel();

        let mut app = Self {
            details_scroll_offset: 0,
            details_scroll_max_offset: 0,
            import_file_explorer,
            export_file_explorer,
            state: AppState::Normal,
            selected_row_stopped: 0,
            selected_row_running: 0,
            active_table: ActiveTable::Stopped,
            selected_rows_stopped: HashSet::new(),
            selected_rows_running: HashSet::new(),
            import_export_message: None,
            input_buffer: String::new(),
            selected_file_path: None,
            file_content: None,
            stopped_configs: Vec::new(),
            running_configs: Vec::new(),
            error_message: None,
            active_component: ActiveComponent::StoppedTable,
            selected_menu_item: 0,
            delete_confirmation_message: None,
            selected_delete_button: DeleteButton::Confirm,
            visible_rows: 0,
            table_state_stopped: TableState::default(),
            table_state_running: TableState::default(),
            contexts: Vec::new(),
            selected_context_index: 0,
            context_list_state: ListState::default(),
            tui_logger_state,
            logger_state,
            settings_timeout_input: String::new(),
            settings_editing: false,
            settings_network_monitor: true,
            settings_selected_option: 0,
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            configs_being_processed: std::collections::HashMap::new(),
            error_receiver: Some(error_receiver),
            error_sender: Some(error_sender),
        };

        if let Ok((_, height)) = size() {
            app.update_visible_rows(height);
        }

        app
    }

    pub fn update_visible_rows(&mut self, terminal_height: u16) {
        self.visible_rows = (terminal_height.saturating_sub(19)) as usize;
    }

    pub fn update_configs(&mut self, configs: &[Config], config_states: &[ConfigState]) {
        self.stopped_configs = configs
            .iter()
            .filter(|config| {
                config_states
                    .iter()
                    .find(|state| state.config_id == config.id.unwrap_or_default())
                    .map(|state| !state.is_running)
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        self.running_configs = configs
            .iter()
            .filter(|config| {
                config_states
                    .iter()
                    .find(|state| state.config_id == config.id.unwrap_or_default())
                    .map(|state| state.is_running)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        let now = std::time::Instant::now();
        self.configs_being_processed
            .retain(|&_config_id, (completion_flag, start_time)| {
                if completion_flag.load(Ordering::Relaxed) {
                    return false;
                }

                if now.duration_since(*start_time) > std::time::Duration::from_secs(30) {
                    return false;
                }

                true
            });

        if let Some(ref mut receiver) = self.error_receiver {
            if let Ok(error_msg) = receiver.try_recv() {
                self.error_message = Some(error_msg);
                self.state = AppState::ShowErrorPopup;
            }
        }
    }

    pub fn scroll_up(&mut self) {
        match self.active_table {
            ActiveTable::Stopped => {
                if !self.stopped_configs.is_empty() {
                    if let Some(selected) = self.table_state_stopped.selected() {
                        if selected > 0 {
                            self.table_state_stopped.select(Some(selected - 1));
                            self.selected_row_stopped = selected - 1;
                        }
                    }
                }
            }
            ActiveTable::Running => {
                if !self.running_configs.is_empty() {
                    if let Some(selected) = self.table_state_running.selected() {
                        if selected > 0 {
                            self.table_state_running.select(Some(selected - 1));
                            self.selected_row_running = selected - 1;
                        }
                    }
                }
            }
        }
    }

    pub fn scroll_down(&mut self) {
        match self.active_table {
            ActiveTable::Stopped => {
                if !self.stopped_configs.is_empty() {
                    if let Some(selected) = self.table_state_stopped.selected() {
                        if selected < self.stopped_configs.len() - 1 {
                            self.table_state_stopped.select(Some(selected + 1));
                            self.selected_row_stopped = selected + 1;
                        }
                    } else {
                        self.table_state_stopped.select(Some(0));
                        self.selected_row_stopped = 0;
                    }
                }
            }
            ActiveTable::Running => {
                if !self.running_configs.is_empty() {
                    if let Some(selected) = self.table_state_running.selected() {
                        if selected < self.running_configs.len() - 1 {
                            self.table_state_running.select(Some(selected + 1));
                            self.selected_row_running = selected + 1;
                        }
                    } else {
                        self.table_state_running.select(Some(0));
                        self.selected_row_running = 0;
                    }
                }
            }
        }
    }
}

pub fn toggle_select_all(app: &mut App) {
    let (selected_rows, configs) = match app.active_table {
        ActiveTable::Stopped => (&mut app.selected_rows_stopped, &app.stopped_configs),
        ActiveTable::Running => (&mut app.selected_rows_running, &app.running_configs),
    };

    if selected_rows.len() == configs.len() {
        selected_rows.clear();
    } else {
        selected_rows.clear();
        for i in 0..configs.len() {
            selected_rows.insert(i);
        }
    }
}

pub async fn handle_input(app: &mut App, mode: DatabaseMode) -> io::Result<bool> {
    if event::poll(std::time::Duration::from_millis(100))? {
        if let Event::Key(key) = event::read()? {
            log::debug!("Key pressed: {key:?}");

            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                stop_all_port_forward_and_exit(app, mode).await;
            }

            match app.state {
                AppState::ShowErrorPopup => {
                    log::debug!("Handling ShowErrorPopup state");
                    handle_error_popup_input(app, key.code)?;
                }
                AppState::ShowConfirmationPopup => {
                    log::debug!("Handling ShowConfirmationPopup state");
                    handle_confirmation_popup_input(app, key.code).await?;
                }
                AppState::ImportFileExplorerOpen => {
                    log::debug!("Handling ImportFileExplorerOpen state");
                    handle_import_file_explorer_input(app, key.code, mode).await?;
                }
                AppState::ExportFileExplorerOpen => {
                    log::debug!("Handling ExportFileExplorerOpen state");
                    handle_export_file_explorer_input(app, key.code, mode).await?;
                }
                AppState::ShowInputPrompt => {
                    log::debug!("Handling ShowInputPrompt state");
                    handle_export_input_prompt(app, key.code, mode).await?;
                }
                AppState::ShowHelp => {
                    log::debug!("Handling ShowHelp state");
                    handle_help_input(app, key.code)?;
                }
                AppState::ShowAbout => {
                    log::debug!("Handling ShowAbout state");
                    handle_about_input(app, key.code)?;
                }
                AppState::ShowDeleteConfirmation => {
                    log::debug!("Handling ShowDeleteConfirmation state");
                    handle_delete_confirmation_input(app, key.code, mode).await?;
                }
                AppState::ShowContextSelection => {
                    log::debug!("Handling ShowContextSelection state");
                    handle_context_selection_input(app, key.code, mode).await?;
                }
                AppState::ShowSettings => {
                    log::debug!("Handling ShowSettings state");
                    handle_settings_input(app, key.code, mode).await?;
                }
                AppState::Normal => {
                    log::debug!("Handling Normal state");
                    handle_normal_input(app, key.code, mode).await?;
                }
            }
        } else if let Event::Resize(_, height) = event::read()? {
            log::debug!("Handling Resize event");
            app.update_visible_rows(height);
        }
    }
    Ok(false)
}

pub async fn handle_normal_input(
    app: &mut App, key: KeyCode, mode: DatabaseMode,
) -> io::Result<()> {
    if handle_common_hotkeys(app, key, mode).await? {
        return Ok(());
    }

    match key {
        KeyCode::Tab => {
            app.active_component = match app.active_component {
                ActiveComponent::Menu => ActiveComponent::StoppedTable,
                ActiveComponent::StoppedTable => ActiveComponent::Details,
                ActiveComponent::Details => ActiveComponent::Menu,
                _ => ActiveComponent::Menu,
            };

            app.active_table = match app.active_component {
                ActiveComponent::StoppedTable => ActiveTable::Stopped,
                _ => app.active_table,
            };
        }
        KeyCode::PageUp | KeyCode::PageDown => match app.active_component {
            ActiveComponent::Logs => handle_logs_input(app, key).await?,
            ActiveComponent::Details => handle_details_input(app, key, mode).await?,
            _ => {
                if key == KeyCode::PageUp {
                    scroll_page_up(app);
                } else {
                    scroll_page_down(app);
                }
            }
        },
        _ => match app.active_component {
            ActiveComponent::Menu => handle_menu_input(app, key, mode).await?,
            ActiveComponent::StoppedTable => handle_stopped_table_input(app, key, mode).await?,
            ActiveComponent::RunningTable => handle_running_table_input(app, key, mode).await?,
            ActiveComponent::Details => handle_details_input(app, key, mode).await?,
            ActiveComponent::Logs => handle_logs_input(app, key).await?,
        },
    }
    Ok(())
}

pub fn scroll_page_up(app: &mut App) {
    match app.active_component {
        ActiveComponent::StoppedTable => {
            let rows_to_scroll = app.visible_rows;
            if app.selected_row_stopped >= rows_to_scroll {
                app.selected_row_stopped -= rows_to_scroll;
            } else {
                app.selected_row_stopped = 0;
            }
            app.table_state_stopped
                .select(Some(app.selected_row_stopped));
        }
        ActiveComponent::RunningTable => {
            let rows_to_scroll = app.visible_rows;
            if app.selected_row_running >= rows_to_scroll {
                app.selected_row_running -= rows_to_scroll;
            } else {
                app.selected_row_running = 0;
            }
            app.table_state_running
                .select(Some(app.selected_row_running));
        }
        ActiveComponent::Details => {
            if app.details_scroll_offset >= app.visible_rows {
                app.details_scroll_offset -= app.visible_rows;
            } else {
                app.details_scroll_offset = 0;
            }
        }
        _ => {}
    }
}

pub fn scroll_page_down(app: &mut App) {
    match app.active_component {
        ActiveComponent::StoppedTable => {
            let rows_to_scroll = app.visible_rows;
            if app.selected_row_stopped + rows_to_scroll < app.stopped_configs.len() {
                app.selected_row_stopped += rows_to_scroll;
            } else {
                app.selected_row_stopped = app.stopped_configs.len() - 1;
            }
            app.table_state_stopped
                .select(Some(app.selected_row_stopped));
        }
        ActiveComponent::RunningTable => {
            let rows_to_scroll = app.visible_rows;
            if app.selected_row_running + rows_to_scroll < app.running_configs.len() {
                app.selected_row_running += rows_to_scroll;
            } else {
                app.selected_row_running = app.running_configs.len() - 1;
            }
            app.table_state_running
                .select(Some(app.selected_row_running));
        }
        ActiveComponent::Details => {
            if app.details_scroll_offset + app.visible_rows < app.details_scroll_max_offset {
                app.details_scroll_offset += app.visible_rows;
            } else {
                app.details_scroll_offset = app.details_scroll_max_offset;
            }
        }
        _ => {}
    }
}

pub fn select_first_row(app: &mut App) {
    match app.active_table {
        ActiveTable::Stopped => {
            if !app.stopped_configs.is_empty() {
                app.table_state_stopped.select(Some(0));
                app.selected_row_stopped = 0;
            }
        }
        ActiveTable::Running => {
            if !app.running_configs.is_empty() {
                app.table_state_running.select(Some(0));
                app.selected_row_running = 0;
            }
        }
    }
}

pub fn clear_selection(app: &mut App) {
    match app.active_table {
        ActiveTable::Stopped => {
            app.selected_rows_stopped.clear();
            app.selected_rows_running.clear();
            app.table_state_stopped.select(None);
            app.selected_row_stopped = 0;
            app.table_state_running.select(None);
            app.selected_row_running = 0;
        }
        ActiveTable::Running => {
            app.selected_rows_running.clear();
            app.selected_rows_stopped.clear();
            app.table_state_running.select(None);
            app.selected_row_running = 0;
            app.table_state_stopped.select(None);
            app.selected_row_stopped = 0;
        }
    }
}

pub async fn handle_menu_input(app: &mut App, key: KeyCode, mode: DatabaseMode) -> io::Result<()> {
    if handle_common_hotkeys(app, key, mode).await? {
        return Ok(());
    }

    match key {
        KeyCode::Left => {
            if app.selected_menu_item > 0 {
                app.selected_menu_item -= 1
            }
        }
        KeyCode::Right => {
            if app.selected_menu_item < 6 {
                app.selected_menu_item += 1
            }
        }
        KeyCode::Down => {
            app.active_component = ActiveComponent::StoppedTable;
            app.active_table = ActiveTable::Stopped;
            clear_selection(app);
            select_first_row(app);
        }
        KeyCode::Enter => match app.selected_menu_item {
            0 => app.state = AppState::ShowHelp,
            1 => handle_auto_add_configs(app).await,
            2 => open_import_file_explorer(app),
            3 => open_export_file_explorer(app),
            4 => {
                app.state = AppState::ShowSettings;
                if let Ok(timeout) =
                    kftray_commons::utils::settings::get_disconnect_timeout_with_mode(mode).await
                {
                    app.settings_timeout_input = timeout.unwrap_or(0).to_string();
                }
                if let Ok(network_monitor) =
                    kftray_commons::utils::settings::get_network_monitor_with_mode(mode).await
                {
                    app.settings_network_monitor = network_monitor;
                }
                app.settings_editing = false;
                app.settings_selected_option = 0;
            }
            5 => app.state = AppState::ShowAbout,
            6 => stop_all_port_forward_and_exit(app, mode).await,
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

pub async fn handle_stopped_table_input(
    app: &mut App, key: KeyCode, mode: DatabaseMode,
) -> io::Result<()> {
    if handle_common_hotkeys(app, key, mode).await? {
        return Ok(());
    }

    match key {
        KeyCode::Right => {
            app.active_component = ActiveComponent::RunningTable;
            app.active_table = ActiveTable::Running;
            clear_selection(app);
            select_first_row(app);
        }
        KeyCode::Up => {
            if app.table_state_stopped.selected() == Some(0) {
                app.active_component = ActiveComponent::Menu;
                app.table_state_running.select(None);
                app.selected_rows_stopped.clear();
                app.table_state_stopped.select(None);
            } else {
                app.scroll_up();
            }
        }
        KeyCode::Down => {
            if app.stopped_configs.is_empty()
                || app.table_state_stopped.selected() == Some(app.stopped_configs.len() - 1)
            {
                app.active_component = ActiveComponent::Details;
                app.table_state_running.select(None);
                app.selected_rows_stopped.clear();
                app.table_state_stopped.select(None);
            } else {
                app.scroll_down();
            }
        }
        KeyCode::Char(' ') => toggle_row_selection(app),
        KeyCode::Char('f') => handle_port_forwarding(app, mode).await?,
        KeyCode::Char('d') => show_delete_confirmation(app),
        KeyCode::Char('a') => toggle_select_all(app),
        _ => {}
    }
    Ok(())
}

pub async fn handle_running_table_input(
    app: &mut App, key: KeyCode, mode: DatabaseMode,
) -> io::Result<()> {
    if handle_common_hotkeys(app, key, mode).await? {
        return Ok(());
    }

    match key {
        KeyCode::Left => {
            app.active_component = ActiveComponent::StoppedTable;
            app.active_table = ActiveTable::Stopped;
            clear_selection(app);
            select_first_row(app);
        }
        KeyCode::Up => {
            if app.running_configs.is_empty() || app.table_state_running.selected() == Some(0) {
                app.active_component = ActiveComponent::Menu;
                app.table_state_running.select(None);
                app.selected_rows_stopped.clear();
                app.table_state_stopped.select(None);
            } else {
                app.scroll_up();
            }
        }
        KeyCode::Down => {
            if app.running_configs.is_empty()
                || app.table_state_running.selected() == Some(app.running_configs.len() - 1)
            {
                app.active_component = ActiveComponent::Logs;
                app.table_state_running.select(None);
                app.selected_rows_stopped.clear();
                app.table_state_stopped.select(None);
            } else {
                app.scroll_down();
            }
        }
        KeyCode::Char(' ') => toggle_row_selection(app),
        KeyCode::Char('f') => handle_port_forwarding(app, mode).await?,
        KeyCode::Char('d') => show_delete_confirmation(app),
        KeyCode::Char('a') => toggle_select_all(app),
        _ => {}
    }
    Ok(())
}

pub async fn handle_details_input(
    app: &mut App, key: KeyCode, mode: DatabaseMode,
) -> io::Result<()> {
    if handle_common_hotkeys(app, key, mode).await? {
        return Ok(());
    }

    match key {
        KeyCode::Right => app.active_component = ActiveComponent::Logs,
        KeyCode::Up => {
            app.active_component = ActiveComponent::StoppedTable;
            app.active_table = ActiveTable::Stopped;
            clear_selection(app);
            if !app.stopped_configs.is_empty() {
                app.table_state_stopped.select(Some(0));
                app.selected_row_stopped = 0;
            }
        }
        KeyCode::PageUp => {
            if app.details_scroll_offset >= app.visible_rows {
                app.details_scroll_offset -= app.visible_rows;
            } else {
                app.details_scroll_offset = 0;
            }
        }
        KeyCode::PageDown => {
            if app.details_scroll_offset + app.visible_rows < app.details_scroll_max_offset {
                app.details_scroll_offset += app.visible_rows;
            } else {
                app.details_scroll_offset = app.details_scroll_max_offset;
            }
        }
        _ => {}
    }
    Ok(())
}

pub async fn handle_logs_input(app: &mut App, key: KeyCode) -> io::Result<()> {
    match key {
        KeyCode::Left => app.active_component = ActiveComponent::Details,
        KeyCode::Up => {
            app.active_component = ActiveComponent::RunningTable;
            app.active_table = ActiveTable::Running;
            clear_selection(app);
            select_first_row(app);
        }
        KeyCode::PageUp => app.tui_logger_state.transition(TuiWidgetEvent::PrevPageKey),
        KeyCode::PageDown => app.tui_logger_state.transition(TuiWidgetEvent::NextPageKey),
        _ => {}
    }
    Ok(())
}

pub async fn handle_common_hotkeys(
    app: &mut App, key: KeyCode, mode: DatabaseMode,
) -> io::Result<bool> {
    match key {
        KeyCode::Char('q') => {
            app.state = AppState::ShowAbout;
            Ok(true)
        }
        KeyCode::Char('i') => {
            open_import_file_explorer(app);
            Ok(true)
        }
        KeyCode::Char('e') => {
            open_export_file_explorer(app);
            Ok(true)
        }
        KeyCode::Char('h') => {
            app.state = AppState::ShowHelp;
            Ok(true)
        }
        KeyCode::Char('s') => {
            app.state = AppState::ShowSettings;
            if let Ok(timeout) =
                kftray_commons::utils::settings::get_disconnect_timeout_with_mode(mode).await
            {
                app.settings_timeout_input = timeout.unwrap_or(0).to_string();
            }
            if let Ok(network_monitor) =
                kftray_commons::utils::settings::get_network_monitor_with_mode(mode).await
            {
                app.settings_network_monitor = network_monitor;
            }
            app.settings_editing = false;
            app.settings_selected_option = 0;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub fn toggle_row_selection(app: &mut App) {
    match app.active_table {
        ActiveTable::Running => {
            if let Some(selected) = app.table_state_running.selected() {
                if app.selected_rows_running.contains(&selected) {
                    app.selected_rows_running.retain(|&x| x != selected);
                } else {
                    app.selected_rows_running.insert(selected);
                }
                app.selected_row_running = selected;
            }
        }
        ActiveTable::Stopped => {
            if let Some(selected) = app.table_state_stopped.selected() {
                if app.selected_rows_stopped.contains(&selected) {
                    app.selected_rows_stopped.retain(|&x| x != selected);
                } else {
                    app.selected_rows_stopped.insert(selected);
                }
                app.selected_row_stopped = selected;
            }
        }
    }
}

pub async fn handle_port_forwarding(app: &mut App, mode: DatabaseMode) -> io::Result<()> {
    let (selected_rows, configs, selected_row) = match app.active_table {
        ActiveTable::Stopped => (
            &mut app.selected_rows_stopped,
            &app.stopped_configs,
            app.selected_row_stopped,
        ),
        ActiveTable::Running => (
            &mut app.selected_rows_running,
            &app.running_configs,
            app.selected_row_running,
        ),
    };

    if configs.is_empty() {
        return Ok(());
    }

    if selected_rows.is_empty() {
        selected_rows.insert(selected_row);
    }

    let selected_configs: Vec<Config> = selected_rows
        .iter()
        .filter_map(|&row| configs.get(row).cloned())
        .collect();

    let start_time = std::time::Instant::now();
    for config in &selected_configs {
        if let Some(id) = config.id {
            let completion_flag = Arc::new(AtomicBool::new(false));
            app.configs_being_processed
                .insert(id, (completion_flag.clone(), start_time));
        }
    }

    if app.active_table == ActiveTable::Stopped {
        app.running_configs.extend(selected_configs.clone());
        app.stopped_configs
            .retain(|config| !selected_configs.contains(config));
    } else {
        app.stopped_configs.extend(selected_configs.clone());
        app.running_configs
            .retain(|config| !selected_configs.contains(config));
    }

    let error_sender = app.error_sender.clone();
    let active_table = app.active_table;
    let logger_state_clone = app.logger_state.clone();
    for config in selected_configs.clone() {
        if let Some(id) = config.id {
            let completion_flag = app
                .configs_being_processed
                .get(&id)
                .map(|(flag, _)| flag.clone());
            let sender = error_sender.clone();
            let logger_state_for_task = logger_state_clone.clone();
            if let Some(flag) = completion_flag {
                tokio::spawn(async move {
                    use crate::core::port_forward::{
                        start_port_forwarding,
                        stop_port_forwarding,
                    };
                    use crate::tui::input::{
                        ActiveTable,
                        App,
                    };

                    let mut temp_app = App::new(logger_state_for_task);

                    let is_starting = active_table == ActiveTable::Stopped;

                    if is_starting {
                        start_port_forwarding(&mut temp_app, config, mode).await;
                    } else {
                        stop_port_forwarding(&mut temp_app, config, mode).await;
                    }

                    if let Some(error_msg) = temp_app.error_message {
                        if let Some(sender) = sender {
                            let _ = sender.send(error_msg);
                        }
                    }

                    flag.store(true, Ordering::Relaxed);
                });
            }
        }
    }

    match app.active_table {
        ActiveTable::Stopped => app.selected_rows_stopped.clear(),
        ActiveTable::Running => app.selected_rows_running.clear(),
    }

    Ok(())
}

pub fn show_delete_confirmation(app: &mut App) {
    if !app.selected_rows_stopped.is_empty() {
        app.state = AppState::ShowDeleteConfirmation;
        app.delete_confirmation_message =
            Some("Are you sure you want to delete the selected configs?".to_string());
    }
}

pub async fn handle_delete_confirmation_input(
    app: &mut App, key: KeyCode, mode: DatabaseMode,
) -> io::Result<()> {
    match key {
        KeyCode::Left | KeyCode::Right => {
            app.selected_delete_button = match app.selected_delete_button {
                DeleteButton::Confirm => DeleteButton::Close,
                DeleteButton::Close => DeleteButton::Confirm,
            };
        }
        KeyCode::Enter => {
            if app.selected_delete_button == DeleteButton::Confirm {
                let ids_to_delete: Vec<i64> = app
                    .selected_rows_stopped
                    .iter()
                    .filter_map(|&row| app.stopped_configs.get(row).and_then(|config| config.id))
                    .collect();

                match kftray_commons::utils::config::delete_configs_with_mode(
                    ids_to_delete.clone(),
                    mode,
                )
                .await
                {
                    Ok(_) => {
                        app.delete_confirmation_message =
                            Some("Configs deleted successfully.".to_string());
                        app.stopped_configs.retain(|config| {
                            !ids_to_delete.contains(&config.id.unwrap_or_default())
                        });
                    }
                    Err(e) => {
                        app.delete_confirmation_message =
                            Some(format!("Failed to delete configs: {e}"));
                    }
                }
            }
            app.selected_rows_stopped.clear();
            app.state = AppState::Normal;
        }
        KeyCode::Esc => app.state = AppState::Normal,
        _ => {}
    }
    Ok(())
}

pub fn open_import_file_explorer(app: &mut App) {
    app.state = AppState::ImportFileExplorerOpen;
    app.selected_file_path = std::env::current_dir().ok();
}

pub fn open_export_file_explorer(app: &mut App) {
    app.state = AppState::ExportFileExplorerOpen;
    app.selected_file_path = std::env::current_dir().ok();
}

pub async fn handle_context_selection_input(
    app: &mut App, key: KeyCode, mode: DatabaseMode,
) -> io::Result<()> {
    if let KeyCode::Enter = key {
        if let Some(selected_context) = app.contexts.get(app.selected_context_index).cloned() {
            handle_context_selection(app, &selected_context, mode).await;
        }
    } else if let KeyCode::Up = key {
        if app.selected_context_index > 0 {
            app.selected_context_index -= 1;
            app.context_list_state
                .select(Some(app.selected_context_index));
        }
    } else if let KeyCode::Down = key {
        if app.selected_context_index < app.contexts.len() - 1 {
            app.selected_context_index += 1;
            app.context_list_state
                .select(Some(app.selected_context_index));
        }
    }
    Ok(())
}

pub async fn handle_settings_input(
    app: &mut App, key: KeyCode, mode: DatabaseMode,
) -> io::Result<()> {
    match key {
        KeyCode::Esc => {
            app.state = AppState::Normal;
            app.settings_editing = false;
        }
        KeyCode::Up => {
            if app.settings_selected_option > 0 {
                app.settings_selected_option -= 1;
            }
        }
        KeyCode::Down => {
            if app.settings_selected_option < 1 {
                app.settings_selected_option += 1;
            }
        }
        KeyCode::Enter => {
            match app.settings_selected_option {
                0 => {
                    // Handle timeout setting
                    if app.settings_editing {
                        if let Ok(timeout_value) = app.settings_timeout_input.parse::<u32>() {
                            if (kftray_commons::utils::settings::set_disconnect_timeout_with_mode(
                                timeout_value,
                                mode,
                            )
                            .await)
                                .is_err()
                            {
                                app.error_message =
                                    Some("Failed to save timeout setting".to_string());
                                app.state = AppState::ShowErrorPopup;
                            } else {
                                app.settings_editing = false;
                            }
                        } else {
                            app.error_message =
                                Some("Invalid timeout value. Please enter a number.".to_string());
                            app.state = AppState::ShowErrorPopup;
                        }
                    } else {
                        app.settings_editing = true;
                    }
                }
                1 => {
                    // Handle network monitor toggle
                    app.settings_network_monitor = !app.settings_network_monitor;
                    if let Err(e) = kftray_commons::utils::settings::set_network_monitor_with_mode(
                        app.settings_network_monitor,
                        mode,
                    )
                    .await
                    {
                        app.error_message =
                            Some(format!("Failed to save network monitor setting: {e}"));
                        app.state = AppState::ShowErrorPopup;
                    } else {
                        // Control network monitor at runtime
                        if app.settings_network_monitor {
                            if let Err(e) = kftray_network_monitor::restart().await {
                                app.error_message =
                                    Some(format!("Failed to start network monitor: {e}"));
                                app.state = AppState::ShowErrorPopup;
                            }
                        } else if let Err(e) = kftray_network_monitor::stop().await {
                            app.error_message =
                                Some(format!("Failed to stop network monitor: {e}"));
                            app.state = AppState::ShowErrorPopup;
                        }
                    }
                }
                _ => {}
            }
        }
        KeyCode::Char(c) => {
            if app.settings_editing && app.settings_selected_option == 0 && c.is_ascii_digit() {
                app.settings_timeout_input.push(c);
            }
        }
        KeyCode::Backspace => {
            if app.settings_editing && app.settings_selected_option == 0 {
                app.settings_timeout_input.pop();
            }
        }
        _ => {}
    }
    Ok(())
}
