@echo off
setlocal
set "PYRA_LAUNCH_EXE=%~dp0Pyra.exe"
if exist "%PYRA_LAUNCH_EXE%" goto launch
set "PYRA_LAUNCH_EXE=%~dp0outputs\Pyra\Pyra.exe"
if exist "%PYRA_LAUNCH_EXE%" goto launch
set "PYRA_LAUNCH_EXE=%~dp0..\outputs\Pyra\Pyra.exe"
if exist "%PYRA_LAUNCH_EXE%" goto launch
echo Pyra.exe was not found.
echo Extract the portable package and keep this launcher beside Pyra.exe.
echo.
pause
exit /b 1

:launch
for %%I in ("%PYRA_LAUNCH_EXE%") do set "PYRA_LAUNCH_DIR=%%~dpI"
if not exist "%PYRA_LAUNCH_DIR%onnxruntime.dll" goto missing_runtime
if not exist "%PYRA_LAUNCH_DIR%onnxruntime_providers_shared.dll" goto missing_runtime
start "" /d "%PYRA_LAUNCH_DIR%" "%PYRA_LAUNCH_EXE%" --background
exit /b 0

:missing_runtime
echo Pyra's runtime files are missing.
echo Extract the entire portable package, including both ONNX Runtime DLLs.
echo.
pause
exit /b 1
