# Client plan: S3 multipart

**Status:** Done in code. Live cutover still open (Forge env/deploy = operator only; Windows build + re-pair). Authority: `SPEC.md` / `AGENTS.md` / `docs/plans/2026-07-11-HANDOFF.md`.

## Three systems

| System | Host |
| --- | --- |
| Control plane | `backup.rui.cam` |
| Sync app | this repo |
| Object storage | `s3.rui.cam` (MinIO) |

## Locked decisions

- S3-only transport; empty/`webdav` config requires re-pair.
- Blocking `ureq` SigV4; no AWS SDK / async runtime.
- PutObject small files; persistent multipart large files (default 32 MiB parts, 2-file concurrency).
- Win7 target via `build-local.ps1` (`x86_64-win7-windows-msvc`).
- `s3_endpoint` from approve must be `https://s3.rui.cam` (never `backup.rui.cam`).

## Still open (ops)

See cutover checklist in `SPEC.md` and `docs/plans/2026-07-11-HANDOFF.md`. Agents never access Forge.