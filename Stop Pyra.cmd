@echo off
rem Stop the tray process and its worker, including an unresponsive instance.
taskkill /F /T /IM Pyra.exe >nul 2>&1
rem Already stopped is also a successful outcome.
tasklist /FI "IMAGENAME eq Pyra.exe" /NH 2>nul | find /I "Pyra.exe" >nul
if errorlevel 1 exit /b 0
echo Pyra could not be stopped. Check that it is running under your user account.
echo.
pause
exit /b 1
