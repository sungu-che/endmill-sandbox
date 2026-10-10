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
            commands::profile_detail,
            commands::profile_set_tool,
            commands::profile_set_workpiece,
            commands::profile_set_coolant,
            commands::calc_context,
            commands::calc_run,
            commands::calc_apply,
            commands::tool_catalog,
            commands::tool_preview,
            commands::tool_save,
            commands::tool_delete,
            commands::workpiece_catalog,
            commands::workpiece_save,
            commands::workpiece_delete,
            commands::preset_catalog,
            commands::preset_create,
            commands::preset_delete,
            commands::profile_from_preset,
            commands::profile_duplicate,
            commands::profile_delete,
            commands::viewport_sim,
            commands::settings_get,
            commands::settings_set_environment,
            commands::settings_set_models,
            commands::settings_set_ui,
            commands::settings_set_comp,
            commands::settings_set_viz,
            commands::settings_set_dry_run,
            commands::tool_state,
            commands::tool_state_reset,
            commands::tool_instances,
            commands::job_records,
            commands::job_record,
            commands::sim_job_start,
            commands::sim_job_replan,
            commands::sim_job_status,
            commands::sim_job_result,
            commands::sim_job_cancel,
            commands::models_catalog,
            commands::models_download,
            commands::models_cancel,
            commands::models_unload,
            commands::models_delete,
            commands::models_ensure,
            commands::ingest_needs,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = &event {
                // 종료 정리가 어디선가 멈춰도 30초 뒤에는 프로세스를 끝냅니다.
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    eprintln!("[exit] 종료 정리가 30초 안에 끝나지 않아 프로세스를 끝냅니다");
                    hard_exit(1);
                });
                if let Some(state) = app_handle.try_state::<commands::AppState>() {
                    state.shutdown();
                }
                // 백그라운드 작업을 멈추고 데이터 저장까지 끝났습니다. 이후의 런타임·GPU 드라이버 정리 단계에서
                // 프로세스가 창 없이 남는 일이 있어 (남은 프로세스가 exe 를 잠가 다음 실행의 rename 이 실패),
                // 여기서 바로 끝냅니다.
                hard_exit(0);
            }
        });
}

/// 프로세스를 즉시 끝냅니다. Windows 에서는 ExitProcess 의 DLL 정리 단계를 거치지 않습니다.
fn hard_exit(code: i32) -> ! {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn TerminateProcess(process: *mut std::ffi::c_void, exit_code: u32) -> i32;
        }
        unsafe {
            TerminateProcess(GetCurrentProcess(), code as u32);
        }
    }
    std::process::exit(code)
}

