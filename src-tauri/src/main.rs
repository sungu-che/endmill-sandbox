#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app
                .path_resolver()
                .app_data_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("endmill-sandbox"));
            app.manage(commands::AppState::new(dir));
            Ok(())
        })
        .on_window_event(|event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event.event() {
                if let Some(state) = event.window().try_state::<commands::AppState>() {
                    state.flush();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_brands,
            commands::get_application_profiles,
            commands::calculate_cutting_conditions,
            commands::get_sample_endmill,
            commands::generate_gcode_preview,
            commands::save_gcode_to_file,
            commands::list_mockup_settings,
            commands::list_presets,
            commands::list_profiles,
            commands::save_profile_to_json,
            commands::load_profile_from_json,
            commands::list_workpiece_setups,
            commands::build_custom_workpiece,
            commands::list_coolant_options,
            commands::apply_coolant_to_profile,
            commands::get_toolpath_segments,
            commands::create_custom_endmill,
            commands::create_custom_profile,
            commands::generate_gcode_with_pattern,
            commands::get_toolpath_with_pattern,
            commands::get_coolant_visual_info,
            commands::save_gcode_with_pattern,
            commands::save_text_file,
            commands::ws_summary,
            commands::ws_select_profile,
            commands::ws_set_options,
            commands::ws_set_frf,
            commands::ws_ingest,
            commands::ws_run_process,
            commands::ws_recommend,
            commands::ws_forecast,
            commands::ws_decide,
            commands::ws_apply_decision,
            commands::ws_adaptive_program,
            commands::models_status,
            commands::models_set_root,
            commands::models_load,
            commands::sds_status,
            commands::sds_flush,
            commands::sds_purge,
            commands::lib_overview,
            commands::lib_create_project,
            commands::lib_use_project,
            commands::lib_save_profile,
            commands::lib_save_preset,
            commands::lib_profiles_by_preset,
            commands::lib_runs,
            commands::lib_curate,
            commands::lib_compare_endmills,
            commands::lib_relay_endmill,
            commands::lib_apply_endmill,
            commands::lib_flush,
            commands::lib_reindex,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}