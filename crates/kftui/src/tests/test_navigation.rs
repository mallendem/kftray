use kftray_commons::models::config_model::Config;
use kftray_commons::utils::db_mode::DatabaseMode;

use crate::tui::input::navigation::{
    handle_auto_add_configs,
    handle_context_selection,
};
use crate::tui::input::{
    ActiveTable,
    App,
    AppState,
};

#[cfg(test)]
mod tests {
    use ratatui::widgets::ListState;

    use super::*;

    fn create_test_config() -> Config {
        Config {
            id: Some(1),
            service: Some("test-service".to_string()),
            namespace: "default".to_string(),
            local_port: Some(8080),
            remote_port: Some(80),
            context: Some("test-context".to_string()),
            workload_type: Some("service".to_string()),
            protocol: "tcp".to_string(),
            remote_address: Some("remote-address".to_string()),
            local_address: Some("127.0.0.1".to_string()),
            auto_loopback_address: false,
            alias: Some("test-alias".to_string()),
            domain_enabled: Some(false),
            kubeconfig: None,
            target: Some("test-target".to_string()),
        }
    }

    // Note: handle_port_forward has been removed as it's now handled
    // directly in the main input handler with proper error handling and throbber
    // support

    #[tokio::test]
    async fn test_handle_auto_add_configs_error() {
        let mut app = App::new();

        handle_auto_add_configs(&mut app).await;

        if app.state == AppState::ShowErrorPopup {
            assert!(
                app.error_message.is_some(),
                "Error message should be present in error state"
            );
            if let Some(error_msg) = &app.error_message {
                println!("Error message: {error_msg}");
            }
        } else {
            assert_eq!(
                app.state,
                AppState::ShowContextSelection,
                "App should be in context selection state"
            );
            assert_eq!(
                app.selected_context_index, 0,
                "First context should be selected"
            );
            assert!(
                app.context_list_state.selected().is_some(),
                "Context list should have a selection"
            );
        }
    }

    #[tokio::test]
    async fn test_handle_context_selection_success() {
        let mut app = App::new();
        app.state = AppState::ShowContextSelection;
        app.contexts = vec!["test-context".to_string()];
        app.selected_context_index = 0;
        app.context_list_state = ListState::default();
        app.context_list_state.select(Some(0));
        handle_context_selection(&mut app, "test-context", DatabaseMode::File).await;
        if app.state == AppState::ShowErrorPopup {
            assert!(app.error_message.is_some());
        } else {
            assert_eq!(
                app.state,
                AppState::Normal,
                "Expected app state to transition to Normal"
            );
        }
    }

    #[tokio::test]
    async fn test_handle_context_selection_error() {
        let mut app = App::new();
        app.state = AppState::ShowContextSelection;

        handle_context_selection(&mut app, "invalid-context", DatabaseMode::File).await;

        assert_eq!(app.state, AppState::ShowErrorPopup);
        assert!(app.error_message.is_some());

        if let Some(error_msg) = &app.error_message {
            assert!(error_msg.contains("Failed to retrieve service configs"));
        }
    }

    #[tokio::test]
    async fn test_handle_auto_add_configs_with_contexts() {
        let mut app = App::new();

        app.contexts = vec![];
        app.state = AppState::Normal;

        handle_auto_add_configs(&mut app).await;

        if app.state == AppState::ShowContextSelection {
            assert_eq!(app.selected_context_index, 0);
            assert!(app.context_list_state.selected().is_some());
        } else {
            assert_eq!(app.state, AppState::ShowErrorPopup);
            assert!(app.error_message.is_some());
        }
    }
}
