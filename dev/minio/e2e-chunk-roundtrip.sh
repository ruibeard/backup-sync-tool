#!/usr/bin/env bash
# Live MinIO proof: bootstrap bucket → desktop SigV4 PUT/GET.
# Optional: Laravel live pair/ensureBucket when box-rui-cam is available.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

./dev/minio/bootstrap.sh

export BACKUP_SYNC_MINIO_ENDPOINT="${BACKUP_SYNC_MINIO_ENDPOINT:-http://127.0.0.1:9000}"
export BACKUP_SYNC_MINIO_ACCESS="${BACKUP_SYNC_MINIO_ACCESS:-minioadmin}"
export BACKUP_SYNC_MINIO_SECRET="${BACKUP_SYNC_MINIO_SECRET:-minioadmin}"
export BACKUP_SYNC_MINIO_BUCKET="${BACKUP_SYNC_MINIO_BUCKET:-backup-dev}"

echo "== Desktop ChunkStoreClient PUT/GET against live MinIO =="
cargo test -q put_get_roundtrip -- --nocapture

LARAVEL_ROOT="${LARAVEL_ROOT:-$(cd "$ROOT/../box-rui-cam" 2>/dev/null && pwd || true)}"
if [[ -n "${LARAVEL_ROOT}" && -f "${LARAVEL_ROOT}/artisan" ]]; then
  echo "== Laravel live MinIO ensureBucket + pair credentials =="
  (
    cd "$LARAVEL_ROOT"
    MINIO_LIVE=1 \
      BACKUP_STORAGE_DRIVER=minio \
      MINIO_ENABLED=true \
      MINIO_S3_ENDPOINT=http://127.0.0.1:9000 \
      MINIO_S3_PUBLIC_ENDPOINT=http://127.0.0.1:9000 \
      MINIO_ROOT_USER=minioadmin \
      MINIO_ROOT_PASSWORD=minioadmin \
      MINIO_BUCKET=backup-dev \
      php artisan test --filter=test_live_minio_pair_and_chunk_roundtrip
  )
else
  echo "Skip Laravel live test (box-rui-cam not found next to desktop repo)."
fi

echo "OK: live MinIO e2e passed"
