#!/usr/bin/env bash
# Live MinIO proof: bootstrap bucket → Laravel pairs a device and signs chunk
# URLs → signed PUT/GET against MinIO → shelf download.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LARAVEL_ROOT="${LARAVEL_ROOT:-$(cd "$ROOT/../box-rui-cam" && pwd)}"
cd "$ROOT"

./dev/minio/bootstrap.sh

echo "== Laravel signed chunk URLs against live MinIO =="
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

echo "OK: live MinIO e2e passed"
