# WebDavSync

WebDavSync is a small native Windows tray application for syncing one local folder to one WebDAV destination.

The product goal is simple:

1. User copies `WebDavSync.exe` into a folder.
2. User double-clicks the exe.
3. User enters a local folder, WebDAV URL, username, and password.
4. The app runs quietly in the tray and keeps the local folder pushed to the WebDAV server.

This project is intentionally focused on a portable, low-friction internal tool. No installer is required for the core product.

## Product Summary

This application is being built as a:

- Native C++ Win32 application
- Portable single executable
- Tray-based background sync tool
- One-way sync client: local folder -> WebDAV server

The current design avoids unnecessary complexity:

- no service for v1
- no multi-folder sync for v1
- no two-way sync for v1
- no dependency on .NET
- no installer requirement for normal use

## User Experience

### First Launch

When the user double-clicks `WebDavSync.exe`, the app opens a small settings window with:

- Folder to watch
- WebDAV URL
- Username
- Password
- Start with Windows
- Delete remote files too
- Save
- Test Connection
- Sync Now

### Normal Use

After setup, the app hides to the Windows tray and runs in the background.

Tray menu:

- Open Settings
- Sync Now
- Open Logs
- Exit

The settings window can be reopened later from the tray icon.

## Core Requirements

The app must:

- run as a single portable `.exe`
- work as a normal user without admin rights
- store configuration locally
- protect the password using Windows DPAPI
- monitor one local folder
- upload new and changed files to WebDAV
- optionally mirror local deletions to WebDAV
- show a tray icon and basic status
- write simple log files for troubleshooting
- support manual `Sync Now`
- support `Start with Windows`

## Non-Goals For V1

The app will not do these things in the first version:

- two-way sync
- conflict resolution
- version history
- multiple watched folders
- service mode
- automatic updates
- complex installer flow
- advanced admin console

These features can be added later if the simple version proves useful.

## Security Requirements

The minimum security rules for this app are:

- only allow `https://` WebDAV endpoints in production use
- validate TLS certificates and hostnames
- do not ignore certificate errors by default
- store passwords with Windows DPAPI
- never store passwords as plain text
- never hash the password for storage, because the app must recover it to authenticate
- never log passwords or authorization headers
- keep the sync credentials limited in scope if possible
- make remote delete opt-in

Important note:

WebDAV over plain `http://` is not acceptable for sensitive or normal business use. The intended deployment is WebDAV over HTTPS.

## Runtime Files

The app is intended to be portable.

Typical folder layout:

```text
WebDavSync/
  WebDavSync.exe
  config.json
  logs/
```

Generated at runtime:

- `config.json`
- `logs/YYYY-MM-DD.log`

## Current Code Structure

Current source files:

- `src/main.cpp`
  - application entry point
- `src/app.h` / `src/app.cpp`
  - Win32 window, tray icon, settings UI, log handling, startup registration
- `src/config.h` / `src/config.cpp`
  - config structure, `config.json` load/save
- `src/dpapi.h` / `src/dpapi.cpp`
  - Windows DPAPI password protection
- `src/file_scanner.h` / `src/file_scanner.cpp`
  - folder scanning and snapshot building
- `src/sync_engine.h` / `src/sync_engine.cpp`
  - background sync loop and file comparison
- `src/webdav_client.h` / `src/webdav_client.cpp`
  - WebDAV requests using WinHTTP

## What Is Already Built

The current codebase already includes:

- CMake project for a native Win32 executable
- portable app structure
- small Win32 settings window
- tray icon with menu
- config persistence in `config.json`
- DPAPI password protection
- startup registration in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
- simple logging
- polling-based folder scanning
- upload of changed files to WebDAV
- remote folder creation using `MKCOL`
- optional remote delete mirroring
- connection test action

## What Still Needs To Be Built Or Hardened

The current code is a strong first cut, but it still needs validation and hardening before broad deployment.

### Build And Validation

- compile on Windows with MSVC
- verify Win32 and x64 builds as needed
- confirm behavior on Windows 7 SP1, Windows 10, and Windows 11
- verify TLS behavior against the real WebDAV server

### Product Hardening

- reject non-HTTPS URLs unless explicitly allowed for testing
- improve error messages for auth, certificate, and connectivity failures
- make status reporting more visible in the tray
- add path safety checks to prevent bad remote paths
- improve handling for locked or half-written files
- test large files and slow network behavior
- verify Unicode file names and special characters
- handle server-specific WebDAV quirks

### Operational Hardening

- better retry and backoff behavior
- safer upload timing so files are not uploaded mid-write
- optional startup flag for silent background launch
- better log detail for support use
- clearer first-run behavior if `config.json` does not exist

## Recommended V1 Release Scope

The recommended first release should be:

- one watched folder
- one-way sync only
- HTTPS WebDAV only
- basic auth over HTTPS
- optional delete sync
- tray UI
- portable exe
- no installer required

That is the smallest useful version with low user friction.

## Implementation Plan

### Phase 1: Make The Current Build Real

Goal:
Get the current code compiling and running on Windows.

Tasks:

- open in Visual Studio or build with CMake + MSVC
- fix any compile issues
- produce `WebDavSync.exe`
- test first launch
- test save/load config
- test tray behavior

### Phase 2: Validate Sync End-To-End

Goal:
Confirm that real file changes sync correctly to the real WebDAV server.

Tasks:

- test upload of new files
- test upload of changed files
- test nested folder creation
- test remote delete mirroring
- test invalid credentials
- test unavailable server
- test TLS failures

### Phase 3: Harden For Real Users

Goal:
Make the app safe enough for staff usage.

Tasks:

- enforce HTTPS-only mode
- improve status and error text
- handle files still being written
- improve retries and backoff
- add clearer logging
- test on real staff-style machines

### Phase 4: Package For Deployment

Goal:
Ship the app in the simplest possible way.

Tasks:

- deliver a portable `WebDavSync.exe`
- confirm it runs from a normal writable folder
- document where `config.json` and logs are stored
- optionally add a simple installer later if needed

## Build Requirements

To build this project on Windows, install:

- Visual Studio 2022 Community or Visual Studio Build Tools
- Desktop development with C++
- MSVC C++ build tools
- Windows SDK
- CMake tools

Build commands:

```powershell
cmake -S . -B build -A Win32
cmake --build build --config Release
```

Expected output:

```text
build/Release/WebDavSync.exe
```

## Design Decisions

Key decisions made for this app:

- native C++ instead of .NET
- portable exe instead of installer-first delivery
- tray app instead of service for v1
- DPAPI protection instead of hashing passwords
- polling-based scanning instead of a more complex file event engine for the first version

These choices keep the product small, understandable, and realistic to ship quickly.

## Success Definition

The app is successful when a non-technical user can:

1. Double-click `WebDavSync.exe`
2. Enter folder, URL, username, and password
3. Click `Save`
4. Close the window
5. Have files continue syncing quietly in the background

That is the bar for v1.
