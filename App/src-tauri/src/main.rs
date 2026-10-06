#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use sil_instrument_console::gui;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .manage(gui::GuiState::default())
        .invoke_handler(tauri::generate_handler![
            gui::host_connect,
            gui::host_call,
            gui::host_heartbeat,
            gui::host_result,
            gui::host_subscribe,
            gui::host_disconnect,
            gui::host_start,
            gui::host_preferences,
            gui::host_save_preferences,
            gui::choose_data_root,
            gui::export_archive,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !window
                    .state::<gui::GuiState>()
                    .exit_authorized
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    api.prevent_close();
                    gui::GuiState::close_requested(window.app_handle());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build Yang LAB INSTRUMENT CONSOLE")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, .. }
                if !app
                    .state::<gui::GuiState>()
                    .exit_authorized
                    .load(std::sync::atomic::Ordering::Acquire) =>
            {
                api.prevent_exit();
                gui::GuiState::close_requested(app);
            }
            tauri::RunEvent::Exit => app.state::<gui::GuiState>().detach(),
            _ => {}
        });
}
