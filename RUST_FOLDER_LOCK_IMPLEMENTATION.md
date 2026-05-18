# Rust Folder Lock Implementation

This document describes the desktop-side changes required for Laravel-approved customer folders.

## Core Rule

The Windows app may detect a proposed customer name before pairing, but Laravel decides the final remote folder during admin approval.

After pairing, the approved `remote_folder` is immutable in the desktop app. The user must not be able to browse up a level, select another customer folder, or save a different destination folder.

## Pairing Request

When the user clicks Pair, send local context only as a hint:

```json
{
  "machine_name": "RECEPTION-PC",
  "windows_user": "office",
  "app_version": "2026.0.3",
  "detected_customer": "Palmeira Minimercado"
}
```

Do not send the currently editable destination field as trusted customer identity.

Use XD/license detection for `detected_customer` where possible. If detection fails, send no detected customer and let the Laravel admin choose.

## Pairing Response

Laravel returns the approved folder:

```json
{
  "status": "approved",
  "device_token": "...",
  "webdav_url": "...",
  "username": "...",
  "password": "...",
  "remote_folder": "XDPT.59655-Palmeira-Minimercado",
  "credential_version": 1
}
```

The app must reject pairing if:

- `device_token` is missing
- `remote_folder` is missing
- `remote_folder` is empty
- `remote_folder` is `/` or `\`
- `remote_folder` contains `..`
- `remote_folder` starts with `/` or `\`

## Local Storage

Store with DPAPI:

- raw `device_token`
- raw WebDAV password

Store normally in config:

- `webdav_url`
- `username`
- approved `remote_folder`
- `credential_version`

The approved folder should be treated as server-owned config, not user-owned settings.

## UI Changes

After a successful pairing:

- destination folder textbox is read-only, disabled, or replaced by static text
- remote folder browse button is hidden or disabled
- server URL, username, and password should also remain locked unless explicit re-pair/refresh flow is added
- save must not overwrite `remote_folder` from the destination textbox

Before pairing:

- the UI may show the locally detected customer hint
- the UI should not imply the user is choosing the final server folder

## Sync Behavior

Uploads must always use the approved folder stored from Laravel.

If config is paired, sync URL construction should use:

```text
webdav_url / approved_remote_folder / relative_file_path
```

The app must not allow remote folder picker navigation to change `approved_remote_folder`.

## Credential Refresh

Start refresh only when needed:

```text
POST /api/device/credential-refresh/start
Authorization: Bearer <device_token>
```

Poll only while that refresh request is pending:

```text
GET /api/device/credential-refresh/status/{request_token}
```

On approved refresh, Laravel may return `remote_folder`.

The desktop must:

- accept it only if it exactly matches the already paired folder
- reject refresh if it is empty, `/`, or different
- DPAPI-save the new WebDAV password
- keep the existing approved folder unchanged
- restart or reconfigure sync before next upload

## Existing Code Areas To Change

| File | Change |
| --- | --- |
| `src/ui.rs` | Disable/hide destination browse after pairing; make destination field read-only after pairing |
| `src/ui.rs` | Stop `do_save` from overwriting paired `remote_folder` from UI text |
| `src/ui.rs` | Validate pairing `remote_folder` before saving |
| `src/ui.rs` | Stop using editable destination field as `detected_customer` during pair start |
| `src/sync.rs` | Ensure upload base always uses approved stored folder |
| `src/config.rs` | Consider adding explicit paired/approved-folder state if needed |
| `src/pairing.rs` | Parse/keep `credential_version`; validate approved folder before applying response |

## Important Security Note

Current shared WebDAV credentials are rooted at `XD-BACKUPS`, so this desktop lock prevents accidents but is not hard tenant isolation against a tampered client.

Hard isolation later requires Laravel to issue customer-scoped WebDAV credentials, where each credential can only access its own customer folder.

The Rust app should still implement the lock now because it prevents normal users from accidentally uploading into the wrong customer folder.
