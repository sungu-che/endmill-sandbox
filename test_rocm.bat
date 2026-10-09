@echo off
chcp 65001 > nul
call "%~dp0_env_rocm.bat"
if errorlevel 1 exit /b 1

cd /d "%~dp0src-tauri"
set "PATH=%CD%\dlls;%PATH%"

echo [TEST] cargo test (ROCm / HIP %ROCM_VER%: %ROCM_PATH%)...
cd /d "%~dp0"
cargo test --no-default-features --features rocm %*
