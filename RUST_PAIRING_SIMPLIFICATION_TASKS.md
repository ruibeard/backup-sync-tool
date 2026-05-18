# Rust Pairing Simplification Tasks

Implement the desktop side of `BACKUP_SYNC_COMMUNICATION_SPEC.md`.

## Goal

The Windows app should get WebDAV credentials from Laravel pairing, save them locally with DPAPI, and upload files directly to WebDAV. Remove credential refresh from the active desktop protocol.

## Pair Start

Endpoint:

```text
POST {pair_api_base}/api/pair/start
```

Send:

```json
{
  "machine_name": "RECEPTION-PC",
  "windows_user": "office",
  "app_version": "2026.0.3",
  "detected_folder": "XDPT.59655-Palmeira-Minimercado"
}
```

Tasks:

- Keep `machine_name`, `windows_user`, and `app_version`.
- Keep `detected_folder`.
- Remove `detected_customer` from the API payload.
- Only send `detected_folder` when it came from XD/license detection.
- Do not send user-edited destination text as trusted folder identity.

XD detection should produce the folder hint:

```text
XDPT.59655-Palmeira-Minimercado
```

## Pair Polling

Endpoint:

```text
GET {pair_api_base}/api/pair/status/{poll_token}
```

Handle exact statuses:

| Status | Desktop behavior |
| --- | --- |
| `pending` | keep polling |
| `approved` | validate payload, save credentials, stop polling |
| `rejected` | show rejected message, stop polling |
| `expired` | show expired message, stop polling |
| `consumed` | show already consumed/re-pair message, stop polling |
| `failed` | show server payload failure, stop polling |

Tasks:

- Replace any `rejected`/`denied` mismatch with the spec value `rejected`.
- Stop polling immediately on terminal statuses.
- Keep timeout as a desktop-side safety limit.

## Approved Payload

Required response fields:

```json
{
  "status": "approved",
  "device_token": "raw-token-once",
  "webdav_url": "https://u561272-sub1.your-storagebox.de",
  "username": "u561272-sub1",
  "password": "raw-password-once",
  "remote_folder": "XDPT.59655-Palmeira-Minimercado"
}
```

Desktop validation:

- reject if `device_token` is missing or empty
- reject if `webdav_url` is missing or not HTTPS
- reject if `username` is missing or empty
- reject if `password` is missing or empty
- reject if `remote_folder` is missing or invalid

Invalid `remote_folder`:

- empty
- `/`
- `\`
- starts with `/` or `\`
- contains `/`
- contains `\`
- contains `..`

## Local Save

Save after approved pairing:

- `device_token_enc` = DPAPI encrypted raw `device_token`
- `password_enc` = DPAPI encrypted raw WebDAV password
- `webdav_url`
- `username`
- approved `remote_folder`
- existing local user settings like `watch_folder`, `start_with_windows`, `sync_remote_changes`, `parallel_uploads`

After pairing:

- server URL is read-only
- username is read-only
- password is read-only
- remote folder is read-only
- Save must not overwrite server-owned fields from UI controls

The only supported way to change customer folder or credentials is to pair again.

## Sync Behavior

Upload directly to WebDAV:

```text
PUT {webdav_url}/{remote_folder}/{relative_file_path}
Authorization: Basic base64(username:password)
```

Rules:

- `remote_folder` always comes from saved approved config.
- The manifest file remains `.backupsynctool-manifest.json`.
- Create child folders under the approved `remote_folder` with `MKCOL` as needed.
- Do not upload outside the approved `remote_folder`.

## Credential Failure

Remove credential refresh from the active behavior.

On WebDAV `401` or `403`:

- stop or pause automatic upload attempts
- log the authentication failure
- show a clear message: credentials are invalid, pair again
- do not call `/api/device/credential-refresh/*`

Files to delete or disconnect if implementation allows:

- `src/credential_refresh.rs`
- refresh structs/messages in `src/ui.rs`
- refresh worker and refresh result handling

If full deletion is too much churn, leave dead code temporarily but ensure no runtime path starts refresh.

## Tests / Manual Checks Later

When implementation starts:

- pair start sends `detected_folder`
- approved response saves DPAPI password/token
- rejected response stops polling immediately
- consumed response tells user to pair again
- upload URL is exactly `webdav_url/remote_folder/relative_path`
- WebDAV auth failure does not start refresh

