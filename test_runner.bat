@echo off
rem ============================================================
rem  End Mill Sandbox - sequential test runner shared by every backend
rem    test_cuda.bat   NVIDIA CUDA        test_rocm.bat  AMD ROCm / HIP
rem    test_vulkan.bat Vulkan             test_cpu.bat   CPU only
rem  The wrapper sets up the toolchain, BACKEND (cuda/rocm/vulkan/cpu), FEAT (cargo feature flags) and
rem  ENDMILL_DEVICE, then calls this file. Do not run this file directly.
rem
rem  The cutting simulation (forces, heat, wear, tool life, tool x material N:N matrix) runs on the CPU for
rem  every backend, so steps 02-14 and 17 must give the same results everywhere. The backend changes only the ML
rem  parts (SigLIP2 wear images, ModernBERT/laya, TTM forecasts); steps 15-16 compare those with the CPU.
rem
rem  Every step writes its own log:  test_logs\<timestamp>_<backend>\NN_name.log
rem  Failures do not stop the run. At the end:
rem    summary.log     exit code and duration of every step, test results, cross-check counts
rem    highlights.log  every cross-check line ([EOL] [MATRIX] [MATRIX-CHECK] [AUDIT] [UNKNOWN] [GPU] [BENCH] ...)
rem    wear_logs\      CSV/JSON job logs written by this run (also mirrored to the app's wear_logs folder)
rem    matrix\         tool x material N:N matrix CSV/JSON per path and the physical check results
rem    manifest.txt    every file this run produced
rem
rem  Options (set before running the wrapper):
rem    NOPAUSE=1          skip the final pause
rem    QUICK=1            skip the long steps (03 tool_state, 13 eol_probe, 14 nn_matrix)
rem    ONLY=04,11         run only these step numbers (step 01 build always runs)
rem    LEAN_LINK=1        link this crate's test binaries without PDB debug info (far less link memory and disk)
rem    CARGO_BUILD_JOBS=2 parallel rustc/link jobs (default 2 for a 7 GB machine)
rem    MATRIX_PATTERNS=rect_profile,pocket_zigzag,helical_pocket   paths of the N:N matrix in step 14
rem    APP_WEAR_LOG_DIR=<dir>   where wear logs are mirrored (default: the app's wear_logs folder)
rem ============================================================
chcp 65001 > nul
setlocal EnableExtensions
if not defined BACKEND set "BACKEND=cpu"
set "RUST_BACKTRACE=1"
rem Limit parallel rustc/link jobs. With every test binary linked at once on a 7 GB machine, step 01 failed with
rem "os error 1455" (paging file too small); a single link.exe of a test binary peaks near 2.6 GB.
if not defined CARGO_BUILD_JOBS set "CARGO_BUILD_JOBS=2"
set "CARGO_EXTRA="
if defined LEAN_LINK set "CARGO_EXTRA=--config profile.dev.package.endmill_sandbox.debug=0 --config profile.dev.package.endmill_sandbox.strip=true"
if not defined MATRIX_PATTERNS set "MATRIX_PATTERNS=rect_profile,pocket_zigzag,helical_pocket"
set "CT=cargo test %FEAT% %CARGO_EXTRA%"
set "CR=cargo run %FEAT% %CARGO_EXTRA%"

if not defined PROTOC (
    where protoc >nul 2>nul
    if errorlevel 1 (
        if not exist "%~dp0tools\protoc\bin\protoc.exe" (
            echo [SETUP] protoc not found - downloading for LanceDB build...
            powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference='Stop'; New-Item -ItemType Directory -Force -Path '%~dp0tools' | Out-Null; Invoke-WebRequest -Uri 'https://github.com/protocolbuffers/protobuf/releases/download/v29.3/protoc-29.3-win64.zip' -OutFile '%~dp0tools\protoc.zip' -UseBasicParsing; Expand-Archive -Force '%~dp0tools\protoc.zip' '%~dp0tools\protoc'"
        )
        set "PROTOC=%~dp0tools\protoc\bin\protoc.exe"
    )
)

cd /d "%~dp0"
set "PATH=%CD%\src-tauri\dlls;%PATH%"

for /f %%i in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd_HHmmss"') do set "TS=%%i"
set "LOGDIR=%CD%\test_logs\%TS%_%BACKEND%"
mkdir "%LOGDIR%"
mkdir "%LOGDIR%\wear_logs"
mkdir "%LOGDIR%\matrix"
set "SUMMARY=%LOGDIR%\summary.log"
set "FAILED=0"
rem wear_compensation saves its CSV/JSON job logs into this run's folder (mirrored to the app folder at the end);
rem tool_life_matrix and step 14 save the N:N matrix next to them
if not defined APP_WEAR_LOG_DIR set "APP_WEAR_LOG_DIR=%APPDATA%\com.qntology.endmill-sandbox\wear_logs"
set "ENDMILL_WEAR_LOG_DIR=%LOGDIR%\wear_logs"
set "ENDMILL_MATRIX_DIR=%LOGDIR%\matrix"

>"%SUMMARY%" echo ========================================
>>"%SUMMARY%" echo  End Mill Sandbox test run %TS%  backend=%BACKEND%  features=%FEAT%
>>"%SUMMARY%" echo  options: QUICK=%QUICK% ONLY=%ONLY% LEAN_LINK=%LEAN_LINK% CARGO_BUILD_JOBS=%CARGO_BUILD_JOBS% MATRIX_PATTERNS=%MATRIX_PATTERNS%
>>"%SUMMARY%" echo ========================================
echo Logs: %LOGDIR%
echo.

rem ---- 00: environment snapshot (toolchain, backend device, CPU, memory, disk) ----
set "ENVLOG=%LOGDIR%\00_env.log"
echo [00] environment snapshot
(
  echo --- date ---
  echo %DATE% %TIME%
  echo --- git ---
  git rev-parse HEAD
  git status --short
  git log -1 --format=%%s
  echo --- rustc / cargo ---
  rustc -Vv
  cargo -V
  echo --- backend %BACKEND% ^(%FEAT%^) ---
  if /i "%BACKEND%"=="cuda" (
    nvcc --version
    nvidia-smi
  )
  if /i "%BACKEND%"=="rocm" (
    "%ROCM_PATH%\bin\hipcc.exe" --version
    if exist "%ROCM_PATH%\bin\hipInfo.exe" "%ROCM_PATH%\bin\hipInfo.exe"
  )
  if /i "%BACKEND%"=="vulkan" (
    if exist "%SystemRoot%\System32\vulkan-1.dll" (echo vulkan-1.dll present) else (echo vulkan-1.dll MISSING - the ML parts fall back to CPU)
    where vulkaninfo >nul 2>nul && vulkaninfo --summary
  )
  echo --- cpu / memory / disk ---
  powershell -NoProfile -Command "$c = Get-CimInstance Win32_Processor | Select-Object -First 1; '{0} - {1} cores / {2} threads' -f $c.Name.Trim(), $c.NumberOfCores, $c.NumberOfLogicalProcessors; $o = Get-CimInstance Win32_OperatingSystem; 'RAM total {0:N1} GB, free {1:N1} GB' -f ($o.TotalVisibleMemorySize/1MB), ($o.FreePhysicalMemory/1MB); 'Disk C: free {0:N1} GB' -f ((Get-PSDrive C).Free/1GB)"
  echo --- env ---
  echo BACKEND=%BACKEND%
  echo FEAT=%FEAT%
  echo ENDMILL_DEVICE=%ENDMILL_DEVICE%
  echo PROTOC=%PROTOC%
  echo CARGO_BUILD_JOBS=%CARGO_BUILD_JOBS%
  echo CARGO_EXTRA=%CARGO_EXTRA%
  echo CARGO_TARGET_DIR=%CARGO_TARGET_DIR%
  echo ENDMILL_WEAR_LOG_DIR=%ENDMILL_WEAR_LOG_DIR%
  echo ENDMILL_MATRIX_DIR=%ENDMILL_MATRIX_DIR%
) > "%ENVLOG%" 2>&1

rem ---- 01: build all test binaries and examples first (separates compile errors from test failures) ----
call :run 01 build_tests "%CT% --no-run"

rem ---- 02..08: integration tests, in dependency order (preset/material list first) ----
call :run 02 environment_tools   "%CT% --test environment_tools --no-fail-fast -- --nocapture --test-threads=1"
call :run 03 tool_state          "%CT% --test tool_state --no-fail-fast -- --nocapture --test-threads=1" long
call :run 04 wear_compensation   "%CT% --test wear_compensation --no-fail-fast -- --nocapture --test-threads=1"
call :run 05 store_flow          "%CT% --test store_flow --no-fail-fast -- --nocapture --test-threads=1"
call :run 06 pipeline_flow       "%CT% --test pipeline_flow --no-fail-fast -- --nocapture --test-threads=1"
call :run 07 interaction_physics "%CT% --test interaction_physics --no-fail-fast -- --nocapture --test-threads=1"
call :run 08 physcal_flow        "%CT% --test physcal_flow --no-fail-fast -- --nocapture --test-threads=1"

rem ---- 09: library unit tests ----
call :run 09 lib_unit            "%CT% --lib --no-fail-fast -- --nocapture --test-threads=1"

rem ---- 10: MPM solver crate (release, backend independent) ----
pushd crates\endmill_mpm
call :run 10 endmill_mpm         "cargo test --release --no-fail-fast -- --nocapture"
popd

rem ---- 11: tool end of life (stop / change / finish part / failure), coating break-through, N:N matrix tests ----
call :run 11 tool_life_matrix    "%CT% --test tool_life_matrix --no-fail-fast -- --nocapture --test-threads=1"

rem ---- 12: full pipeline audit of every built-in preset (rect_profile): [AUDIT] [EOL] [UNKNOWN] lines ----
call :run 12 sim_audit_presets   "call :audit_all"

rem ---- 13: end-of-life behaviour of every preset on the pocket path under all four tool-life policies ----
call :run 13 eol_probe           "%CR% --example eol_probe -- pocket_zigzag stop,change_tool,finish_part,run_to_failure" long

rem ---- 14: tool x material N:N matrix (8 built-in tools x 8 use cases) on several paths, with the physical check ----
rem      ([MATRIX-CHECK]: finite values, no mm/min wear after coating break-through, temperature and VB bounds,
rem       carbon coatings on Fe/Ti/Ni and HSS on 58 HRC judged unsuitable, intended pairs not unsuitable)
call :run 14 nn_matrix           "%CR% --example pair_matrix -- - %MATRIX_PATTERNS% --check" long

rem ---- 15-16: ML on the GPU backend compared with the CPU (skipped on the cpu backend) ----
if /i "%BACKEND%"=="cpu" (
    >>"%SUMMARY%" echo 15_gpu_parity: skipped - cpu backend
) else (
    call :run 15 gpu_parity "%CT% --test gpu_backend --no-fail-fast -- --nocapture --test-threads=1"
)
if /i "%BACKEND%"=="vulkan" (
    call :run 16 vulkan_backend "%CT% --test vulkan_backend --no-fail-fast -- --nocapture --test-threads=1"
) else (
    >>"%SUMMARY%" echo 16_vulkan_backend: skipped - vulkan only
)

rem ---- 17: literature benchmark - the tools, materials and cutting conditions of the cited papers moved into the sandbox ----
rem      data (21 paper tools + 8 built-in tools x 35 paper conditions) and compared with what the papers measured
rem      ([BENCH] comparison lines, [BENCH-SUM] per quantity, [BENCH-ANCHOR] per-material life diagnostic, [BENCH-GRID] life grid;
rem      see docs\literature-benchmark.md)
call :run 17 literature_bench    "%CT% --test literature_benchmark --no-fail-fast -- --nocapture --test-threads=1"

rem ---- mirror this run's wear logs into the app folder (the app lists them there) ----
if exist "%LOGDIR%\wear_logs\*" (
    if not exist "%APP_WEAR_LOG_DIR%" mkdir "%APP_WEAR_LOG_DIR%"
    copy /y "%LOGDIR%\wear_logs\*.*" "%APP_WEAR_LOG_DIR%\" >nul
    >>"%SUMMARY%" echo wear logs: %LOGDIR%\wear_logs  ^(mirrored to %APP_WEAR_LOG_DIR%^)
)

echo.
>>"%SUMMARY%" echo ========================================
if "%FAILED%"=="0" (
    >>"%SUMMARY%" echo ALL STEPS PASSED
) else (
    >>"%SUMMARY%" echo STEPS FAILED: %FAILED%
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0test_summary.ps1" -LogDir "%LOGDIR%" -Backend "%BACKEND%"
echo.
type "%SUMMARY%"
echo.
echo Logs saved in: %LOGDIR%
echo Cross-check lines: %LOGDIR%\highlights.log
if not defined NOPAUSE pause
endlocal & exit /b %FAILED%

rem ------------------------------------------------------------
rem  :run <NN> <name> "<command>" [long]
rem ------------------------------------------------------------
:run
set "ID=%~1"
set "STEP=%~1_%~2"
set "CMDLINE=%~3"
set "IS_LONG=%~4"
set "STEPLOG=%LOGDIR%\%STEP%.log"
if not defined ONLY goto :run_quick
if "%ID%"=="01" goto :run_quick
echo ,%ONLY%, | findstr /C:",%ID%," >nul
if errorlevel 1 (
    echo [%STEP%] skipped - not in ONLY=%ONLY%
    >>"%SUMMARY%" echo %STEP%: skipped - ONLY
    exit /b 0
)
:run_quick
if not defined QUICK goto :run_go
if /i not "%IS_LONG%"=="long" goto :run_go
echo [%STEP%] skipped - QUICK
>>"%SUMMARY%" echo %STEP%: skipped - QUICK
exit /b 0
:run_go
echo [%STEP%] %CMDLINE%
for /f %%i in ('powershell -NoProfile -Command "[DateTimeOffset]::Now.ToUnixTimeSeconds()"') do set "T0=%%i"
>"%STEPLOG%" echo [%TIME%] start %STEP%  backend=%BACKEND%
>>"%STEPLOG%" echo $ %CMDLINE%
>>"%STEPLOG%" echo.
%CMDLINE% >> "%STEPLOG%" 2>&1
set "RC=%ERRORLEVEL%"
for /f %%i in ('powershell -NoProfile -Command "[DateTimeOffset]::Now.ToUnixTimeSeconds()"') do set "T1=%%i"
set /a DUR=T1-T0
>>"%STEPLOG%" echo.
>>"%STEPLOG%" echo [%TIME%] end %STEP% exit=%RC% (%DUR% s)
if not "%RC%"=="0" set /a FAILED+=1
>>"%SUMMARY%" echo %STEP%: exit=%RC%  %DUR% s
findstr /B /C:"test result:" /C:"error" "%STEPLOG%" >> "%SUMMARY%" 2>nul
findstr /C:"FAILED" /C:"panicked at" "%STEPLOG%" >> "%SUMMARY%" 2>nul
echo   -^> exit=%RC%  %DUR% s  (%STEPLOG%)
exit /b 0

rem ------------------------------------------------------------
rem  :audit_all - sim_audit example for every built-in preset (0..7) on rect_profile
rem ------------------------------------------------------------
:audit_all
set "AUDIT_RC=0"
for /L %%i in (0,1,7) do (
    echo ===== preset %%i rect_profile =====
    %CR% --example sim_audit -- %%i rect_profile
    if errorlevel 1 set "AUDIT_RC=1"
)
exit /b %AUDIT_RC%
