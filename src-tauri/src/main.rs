#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

fn main() {
    tauri::Builder::default()
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}