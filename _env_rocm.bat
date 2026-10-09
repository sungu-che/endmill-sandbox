@echo off

set "VS_SCRIPT="
set "VS_ARGS="
set "VS2022=%ProgramFiles%\Microsoft Visual Studio\2022"
for %%E in (Community Professional Enterprise BuildTools) do call :probe_vs "%VS2022%\%%E"
if not defined VS_SCRIPT call :probe_vswhere
if not defined VS_SCRIPT (
    echo [MSVC] Visual Studio C++ Build Tools not found. Install the "Desktop development with C++" workload.
    exit /b 1
)
if not defined VCVARS_VER set "VCVARS_VER=14.44"
call "%VS_SCRIPT%" %VS_ARGS% -vcvars_ver=%VCVARS_VER% > "%TEMP%\vcvars_out.txt" 2>&1
echo [MSVC] requested=%VCVARS_VER% active=%VCToolsVersion%
type "%TEMP%\vcvars_out.txt"
set "PYTHONIOENCODING=utf-8"

if not defined ROCM_VER set "ROCM_VER=7.2"
set "ROCM_PATH=%ProgramFiles%\AMD\ROCm\%ROCM_VER%"
if not exist "%ROCM_PATH%\bin\hipcc.exe" (
    echo [ROCm] HIP SDK %ROCM_VER% not found: %ROCM_PATH%
    echo        Installed versions:
    dir /b /ad "%ProgramFiles%\AMD\ROCm" 2>nul
    exit /b 1
)
set "HIP_PATH=%ROCM_PATH%"
set "HIPCC=%ROCM_PATH%\bin\hipcc.exe"
set "PATH=%ROCM_PATH%\bin;%PATH%"

rem LanceDB 빌드용 protoc (없으면 내려받기)
if not defined PROTOC (
    where protoc >nul 2>nul
    if errorlevel 1 (
        if not exist "%~dp0tools\protoc\bin\protoc.exe" (
            echo [SETUP] LanceDB 빌드용 protoc 내려받는 중...
            powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference='Stop'; New-Item -ItemType Directory -Force -Path '%~dp0tools' | Out-Null; Invoke-WebRequest -Uri 'https://github.com/protocolbuffers/protobuf/releases/download/v29.3/protoc-29.3-win64.zip' -OutFile '%~dp0tools\protoc.zip' -UseBasicParsing; Expand-Archive -Force '%~dp0tools\protoc.zip' '%~dp0tools\protoc'"
        )
        set "PROTOC=%~dp0tools\protoc\bin\protoc.exe"
    )
)

if not exist "%~dp0node_modules" (
    echo [SETUP] node_modules 없음 - npm install 실행...
    call npm install
    if errorlevel 1 exit /b 1
)

set "TAURI_CLI=cargo tauri"
if exist "%~dp0node_modules\.bin\tauri.cmd" set TAURI_CLI="%~dp0node_modules\.bin\tauri.cmd"
exit /b 0

:probe_vs
if defined VS_SCRIPT exit /b 0
if exist "%~1\VC\Auxiliary\Build\vcvars64.bat" (
    set "VS_SCRIPT=%~1\VC\Auxiliary\Build\vcvars64.bat"
    exit /b 0
)
if exist "%~1\Common7\Tools\VsDevCmd.bat" (
    set "VS_SCRIPT=%~1\Common7\Tools\VsDevCmd.bat"
    set "VS_ARGS=-arch=x64 -host_arch=x64"
)
exit /b 0

:probe_vswhere
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" exit /b 0
for /f "usebackq delims=" %%I in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do call :probe_vs "%%I"
exit /b 0
