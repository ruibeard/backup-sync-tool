# Rust Licence Detection Integration Tasks

Goal: implement the fast XD licence detection directly in the Rust app. Do not use PowerShell/reflection on the pairing path, and do not depend on spawning `license-inspector.exe` for the normal app flow.

## Current Situation

- `license-inspector.exe` in the repo root is now a fast NativeAOT helper.
- `license-inspector.exe --remote-folder` returns the folder hint in about 18-20ms on this machine.
- Running `license-inspector.exe` without arguments prints all available licence details.
- Running `license-inspector.exe --json` prints all available licence details as JSON.
- The helper proves the fast algorithm:
  - parse `xd.lic` JSON
  - read `xd.pem`
  - decrypt `Number` and `ClientComercialName`
  - slugify into the remote folder hint
- The Rust app still has slow XD detection logic in `src/xd.rs` that starts PowerShell and loads XD DLLs directly.

## Required Rust Change

Update `src/xd.rs` so `detect_customer_hint()` implements the fast detection natively in Rust.

Recommended behavior:

1. Keep lightweight XD path checks:
   - `C:\XDSoftware`
   - `C:\XDSoftware\cfg\xd.lic`
   - `C:\XDSoftware\cfg\xd.pem`
2. Read `xd.lic` as JSON.
3. Read `xd.pem` as an RSA public key.
4. Decrypt only:
   - `Number`
   - `ClientComercialName`
5. Build `DetectedCustomer.folder` as:

   ```text
   Number + "-" + slugified(ClientComercialName)
   ```

6. Set `DetectedCustomer.customer` to the decrypted `ClientComercialName`.
7. If native detection fails, optionally fall back to `license-inspector.exe --remote-folder` as a diagnostic/compatibility fallback.
8. Remove the PowerShell/XD DLL reflection path once native detection is verified on a real XD install.

## Algorithm Reference

Use the standalone helper as the reference implementation:

- source: `license-inspector/Program.cs`
- command: `license-inspector.exe --remote-folder`

Expected output on this machine:

```text
XDPT.59655-Palmeira-Minimercado
```

The XD encrypted fields are base64 chunks separated by `=` characters. Match the helper behavior:

1. Split the encrypted string on `=`.
2. Re-append `=` to each non-empty chunk.
3. Base64-decode each chunk.
4. Apply raw RSA public-key operation with the key from `xd.pem`.
5. Concatenate decoded blocks.
6. Decode the bytes as UTF-8.

## Dependency Choice

Preferred approach: use small Rust crypto/parsing crates for clarity.

Already available:

- `serde_json`
- `base64`

Likely needed:

- PEM/RSA big integer support through a small maintained crate, or Windows CNG APIs.

Tradeoff:

- Rust crypto crate: simpler, easier to test, less Win32 code.
- Windows CNG APIs: avoids a new dependency, but is more verbose and easier to get wrong.

Choose the crate path unless there is a strong release-size or policy reason not to.

## Pairing Flow Impact

The Pair button must not wait for licence detection before showing the popup.

Expected flow:

1. User clicks Pair.
2. Pairing popup opens immediately.
3. Background worker gets optional detected folder using `detect_customer_hint()`.
4. Background worker posts `/api/pair/start`.
5. Popup updates with QR/code when the server responds.

This preserves the immediate-popup UX while making the optional folder hint in-process and avoiding helper process startup cost.

## Validation

After changing Rust, run from repo root:

```powershell
.\build-local.ps1
```

Confirm:

- release build succeeds with 0 errors
- root `backupsynctool.exe` is copied
- app is running from the repo root
- Pair popup opens immediately
- QR/code appears after the server responds
- native Rust detection output matches `license-inspector.exe --remote-folder`
- pairing still works if native detection fails, because the folder hint is optional and server approval owns the final customer folder

## Notes

- Do not call default `license-inspector.exe` mode from the app; it prints many fields and is unnecessary for pairing.
- If a fallback helper call is kept, call only `license-inspector.exe --remote-folder`.
- Do not make Pair popup visibility depend on licence detection.
- The helper exe should remain useful for manual diagnostics, but the app should not depend on it for the normal fast path.
