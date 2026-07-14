# Greenfield Backup Sync Tool Rebuild

## Goal

Rebuild the desktop application around the smooth `main`-branch experience while retaining S3 as the only transport. Replace shared orchestration, pairing, sync state, transfer progress, secret activation, logging, and the Windows/macOS native interfaces.

Build order: shared headless core, transfer engine, Windows reference UI, native macOS parity.

## Architecture

- Add one single-owner `AppController` driven by `AppCommand`, emitting typed `AppEvent` and `AppSnapshot` values over standard threads and channels. Do not add an async runtime.
- Keep connection, pairing, scanning, uploading, restoring, paused, and failure states separate.
- Replace log parsing with typed scan, transfer, progress, completion, failure, batch, and authentication events.
- Maintain at most two upload workers and a current-run activity list capped at 200 rows. Logs remain durable but never repopulate the UI.
- Make `BackupTransport` upload and download operations cancellable and byte-progress aware. Report real progress for streamed PUT, resumed multipart uploads, and restore downloads.
- Rebuild pairing as a cancellable typed client returning detailed errors. Validate a complete S3 approval and the issued credentials before locally replacing an existing connection.
- Stage candidate secrets independently and activate config atomically. On macOS use unique candidate Keychain accounts; on Windows stage DPAPI ciphertext.
- Serialize log writes process-wide and never log secrets, device tokens, or polling tokens.

## Product Behaviour

- The main window shows local folder and server status, Open, Choose, Connect/Reconnect, current-run activity, overall progress, contextual retry, Start at login, and Auto-update.
- Connect starts immediately with the saved control-plane URL and shows the QR/code. Change Server cancels local polling, validates and persists the new URL, then starts a new request.
- Pair approval closes the panel and immediately starts the initial upload.
- Cancelling, rejecting, or timing out a reconnect leaves the previous local connection untouched.
- A first backup without a local v2 manifest uploads every local file. Manifest entries change only after verified remote upload.
- Authentication and policy failures pause sync and require reconnect. Ordinary failures remain retryable.
- Restore lives in the secondary tray/menu, focuses the main window, and reports live progress with cancellation.
- Windows remains raw Win32. macOS remains native AppKit and removes the glance popover; a primary menu-bar click opens the full window.
- Manual launches show the window. Start-at-login or daemon launches stay hidden.
- Preserve schema v2, S3-only pairing, DPAPI entropy `webdavsync-v1`, macOS Keychain `security ... -A`, one-way uploads, persistent multipart resume, safe whole-customer restore, and two-file concurrency.

## Implementation Sequence

1. Introduce shared controller contracts, typed pairing, atomic secret/config activation, serialized logging, and fake implementations for tests.
2. Rebuild S3 PUT, multipart, verification, manifest, watcher, retry, restore, and typed progress.
3. Replace Windows orchestration and UI against the shared controller.
4. Replace macOS orchestration and UI against the same controller; delete the popover and duplicated pairing flow.
5. Remove compatibility code only after both platforms compile and shared behaviour tests pass.
6. Update `SPEC.md`. Leave `proxmox/` and `license-inspector/` untouched.

## Tests and Acceptance

- Cover pairing request fields, URL mismatch, cancellation, polling states, malformed/error responses, and strict S3 approval.
- Cover reconnect success, cancellation, rejection, activation failure, candidate cleanup, and local rollback.
- Cover first backup, new and modified files, no remote delete on local deletion, two-upload maximum, monotonic progress, multipart resume, verified manifest updates, retry/auth pause, safe restore, serialized logs, and controller reduction.
- Run `cargo test --all-targets`, a Windows cross-target type check, and `./build-macos.sh` after every application-affecting commit.
- Smoke macOS pairing, initial upload, live percentages, file modification, retry, cancelled reconnect, restore, relaunch, and no-Keychain-prompt behaviour.
- Push the completed branch. Windows operator then runs `git pull` and `.\build-windows.ps1` and reports any `control_plane_url mismatch`.

## Execution Notes

- Starting branch: `s3-multipart-implementation` at `e4d3f6f`.
- The initial worktree contained overlapping uncommitted WIP in `SPEC.md`, `build-macos.sh`, core sync/pairing/secret/logging/S3 files, and macOS UI files. A binary safety patch was saved outside the repository at `/tmp/backup-sync-tool-pre-greenfield.patch` before this plan commit.
- Preserve or deliberately transplant sealed-app log placement, strict approval fields, manual-folder pairing hints, authentication-stop behaviour, and URL-decoded S3 listing keys from that WIP.
- This file is explicitly requested project documentation. `SPEC.md` remains the authoritative product and technical contract.
- Do not access Forge, production configuration, or deployment credentials. Do not run `release.sh`.

## Laravel Follow-up Boundary

The current Laravel start/status payload supports the ordinary desktop pairing flow, but it cannot guarantee full transactional reconnect: approval revokes the old key before the replacement is validated, the approved payload is consumed before delivery is confirmed, and there are no complete/cancel endpoints.

This desktop rebuild keeps old local config until candidate validation succeeds. Cancellation, rejection, and timeout preserve it. If validation fails after server approval, show **Reconnect required** instead of claiming the revoked key still works.

After the desktop rebuild, audit `box-rui-cam` read-only and propose a separate two-phase API plan: staged approval, repeatable approved polling, idempotent complete/cancel, deferred old-key revocation, canonical `APP_URL`, explicit expiry, stable public states, and feature tests. Do not change Laravel as part of this implementation.
