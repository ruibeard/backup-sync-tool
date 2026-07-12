# Ops: s3.rui.cam (object storage only)

**System:** Proxmox MinIO — not Laravel, not the sync app.

Authoritative host notes: `/Users/ruibeard/code/proxmox/s3-minio-ct.md`.

| Item | Value |
| --- | --- |
| Host | `balaco` / `192.168.0.46` |
| CT | `101` / `10.10.10.10` |
| Public API | `https://s3.rui.cam` (host Caddy + Let's Encrypt → CT `:9000`) |
| Bucket (shared / admin) | `xd-backups` |
| Creds | `/root/s3-minio-creds.txt` on Proxmox (gitignored copies elsewhere) |

## Edge / certs

Router forward **:443 only** to the Proxmox host. Host Caddy terminates TLS (Let's Encrypt for `s3.rui.cam`) and proxies to MinIO on the CT.

- Do **not** publish MinIO `:9000` or console `:9001` on the public internet.
- LAN console: `http://192.168.0.46:9001`.
- Verified 2026-07-12: health **200**, valid LE cert — certs are not a cutover blocker.

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" https://s3.rui.cam/minio/health/live
# expect 200 (no -k)
```

## Related (other systems)

- Pairing API: `https://backup.rui.cam` (Laravel) — Forge env/deploy is **operator-only**; agents must not access Forge.
- Client pair default: `pair_api_base` → that URL; `s3_endpoint` comes from approve payload and must be `https://s3.rui.cam`.
