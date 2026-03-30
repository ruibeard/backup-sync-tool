# WebDavSync — UI Design Prompt

## What the application does

WebDavSync is a Windows desktop tray utility. It continuously syncs a local folder on the user's machine to a remote folder on a WebDAV server (e.g. a NAS or cloud storage box). The user configures it once, saves, and it runs silently in the system tray — uploading local changes and optionally downloading remote changes automatically.

The main window is the only settings and status panel. The user opens it by double-clicking the tray icon. Closing the window does not exit the app — it just hides it. The only way to exit is via the system tray context menu.

---

## Sections and fields

The UI is divided into logical sections. Each section has a heading that visually separates it from the next.

### 1. Local Folder

The local directory on the user's machine that will be synced.

- **Folder** — text field for the local folder path, with a **Browse** button next to it that opens a folder picker

### 2. Server

Connection details for the remote WebDAV server.

- **URL** — text field for the WebDAV server URL, with a small button next to it that opens the URL in the default browser
- **Username** — text field
- **Password** — password field (input is masked)
- **Remote folder** — text field for the path on the server to sync to/from, with a **Browse** button that connects to the server and lets the user pick a remote folder from a tree
- **Connect** button — tests the connection using the entered credentials; shows an inline status message next to it indicating whether the connection succeeded or failed (e.g. "Not connected" or "Connected since 14:32")

### 3. Options

Behavioural toggles.

- **Start with Windows** — checkbox; registers or unregisters the app in Windows startup
- **Sync remote changes** — checkbox; when enabled, the app also downloads files that were changed on the server

### 4. Sync status

A slim persistent strip that always shows the current state of the sync engine.

- A single line of status text (e.g. "Not configured", "Watching for changes", "Syncing 3 of 12 files…", "Error: connection refused")
- A thin horizontal progress bar beneath it, which fills during active sync and is empty when idle

### 5. Recent activity

A scrollable log of recent sync events.

- A read-only list where each line is a timestamped entry (e.g. "14:32:05 Uploaded report.pdf")
- Newest entries appear at the bottom and scroll into view automatically
- This area should expand to fill all remaining vertical space in the window

---

## Actions / buttons

| Button | What it does |
|---|---|
| Browse (local folder) | Opens a system folder picker dialog |
| Browse (remote folder) | Connects to the server and shows a folder tree to pick from |
| Connect | Tests the WebDAV connection and updates the inline status next to it |
| Save | Validates all fields, tests connection, saves settings, and (re)starts the sync engine |
| Close | Hides the window; the app keeps running in the tray |

---

## Application states

| State | Status text | Progress bar |
|---|---|---|
| Not configured | "Not configured" | empty |
| Connecting | "Connecting to server..." | empty |
| Idle / watching | "Watching for changes" | empty |
| Syncing | "Syncing X of Y files..." | filling |
| Error | "Error: [reason]" | empty |

---

## Design goals

- Clean, minimal, professional — the feel of a modern utility like Dropbox or OneDrive settings
- No decorative borders or group boxes — use spacing and subtle dividers to separate sections
- Form layout: labels on the left, fields on the right, consistent alignment across all rows
- The sync status strip and the Save / Close buttons must always be visible — they should never be hidden by scrolling
- The activity log area is the only part that grows; everything else has a fixed height
- Compact but not cramped — enough breathing room between sections to read comfortably
