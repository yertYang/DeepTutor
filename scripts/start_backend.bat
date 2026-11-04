@echo off
setlocal
cd /d "%~dp0.."
if not defined DEEPTUTOR_RUST_SERVER_BIN set "DEEPTUTOR_RUST_SERVER_BIN=%CD%\target\release\deeptutor.exe"
if not exist "%DEEPTUTOR_RUST_SERVER_BIN%" (
  echo Build first: cargo build --release --locked -p deeptutor-server
  exit /b 1
)
if not defined DEEPTUTOR_HOME set "DEEPTUTOR_HOME=%CD%"
"%DEEPTUTOR_RUST_SERVER_BIN%" --home "%DEEPTUTOR_HOME%" serve --port 8001 %*
exit /b %errorlevel%
