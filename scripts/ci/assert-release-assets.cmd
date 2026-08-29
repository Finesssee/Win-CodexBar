@echo on
setlocal

set "VERSION="
for /f "tokens=3" %%A in ('findstr /b /c:"version = " rust\Cargo.toml') do (
  if not defined VERSION set "VERSION=%%~A"
)

if not defined VERSION (
  echo Failed to determine version from rust\Cargo.toml
  exit /b 1
)

set "ASSETS_DIR=C:\code\Win-CodexBar-release\assets"
set "MISSING=0"

for %%F in (
  "CodexBar-%VERSION%-Setup.exe"
  "CodexBar-%VERSION%-Setup.exe.sha256"
  "CodexBar-%VERSION%-portable.exe"
  "CodexBar-%VERSION%-portable.exe.sha256"
  "CodexBarCLI-v%VERSION%-windows-x64.zip"
  "CodexBarCLI-v%VERSION%-windows-x64.zip.sha256"
) do (
  if not exist "%ASSETS_DIR%\%%~F" (
    echo Missing release artifact: %ASSETS_DIR%\%%~F
    set "MISSING=1"
  ) else (
    echo Found %ASSETS_DIR%\%%~F
  )
)

if "%MISSING%"=="1" exit /b 1
exit /b 0
