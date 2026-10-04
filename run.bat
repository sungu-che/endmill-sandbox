@echo off
chcp 65001 >nul
echo ========================================
echo   End Mill Sandbox - Build ^& Run
echo ========================================
echo.

echo [1/4] Rust 라이브러리 + CLI 빌드...
cargo build
if %ERRORLEVEL% NEQ 0 (
    echo [ERROR] Rust 빌드 실패! 에러를 확인하세요.
    pause
    exit /b 1
)
echo [OK] 빌드 완료
echo.

echo [2/4] CLI 데모 실행...
echo.
cargo run --bin endmill_demo
echo.

echo [3/4] 프론트엔드 의존성 확인...
if not exist "node_modules" (
    echo node_modules 없음 - npm install 실행...
    call npm install
    if %ERRORLEVEL% NEQ 0 (
        echo [ERROR] npm install 실패!
        pause
        exit /b 1
    )
)
echo [OK] 의존성 확인 완료
echo.

echo [4/4] Tauri 데스크톱 앱 실행...
call npm run tauri dev
pause