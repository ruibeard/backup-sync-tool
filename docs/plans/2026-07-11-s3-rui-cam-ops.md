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

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" https://s3.rui.cam/minio/health/live
# expect 200
```

## Related (other systems)

- Pairing API: `https://backup.rui.cam` (Laravel)
- Client pair default: `pair_api_base` → that URL; `s3_endpoint` comes from approve payload
