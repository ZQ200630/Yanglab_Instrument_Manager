#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use sil_instrument_console::gui;
use tauri::Manager;

fn main() {
    let profile = match sil_instrument_console::profile::Profile::from_args(std::env::args().skip(1)) {
        Ok(profile) => profile,
        Err(error) => { eprintln!("{error}"); return; },
    };
    tauri::Builder::default()
        .manage(profile)
        .manage(gui::GuiState::default())
        .manage(sil_instrument_console::remote_gui::RemoteGuiState::default())
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
            sil_instrument_console::remote_gui::remote_peers,
            sil_instrument_console::remote_gui::remote_pair,
            sil_instrument_console::remote_gui::remote_pair_request,
            sil_instrument_console::remote_gui::remote_pair_status,
            sil_instrument_console::remote_gui::remote_pair_cancel,
            sil_instrument_console::remote_gui::remote_connect,
            sil_instrument_console::remote_gui::remote_call,
            sil_instrument_console::remote_gui::remote_subscribe,
            sil_instrument_console::remote_gui::remote_disconnect,
            sil_instrument_console::remote_gui::remote_forget,
            sil_instrument_console::remote_gui::remote_export,
            sil_instrument_console::profile::app_profile,
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
