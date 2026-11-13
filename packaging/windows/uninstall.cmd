@echo off
rem Areko GPUPinWatch: double-click to remove (asks for administrator rights).
powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -NoExit -File ""%~dp0install.ps1"" -Uninstall'"
