#!/usr/bin/env bash
# Create the local backup-dev bucket used by Laravel BACKUP_STORAGE_DRIVER=minio/fake.
set -euo pipefail
ROOT_USER="${MINIO_ROOT_USER:-minioadmin}"
ROOT_PASSWORD="${MINIO_ROOT_PASSWORD:-minioadmin}"
ENDPOINT="${MINIO_S3_ENDPOINT:-http://127.0.0.1:9000}"
BUCKET="${MINIO_BUCKET:-backup-dev}"

docker run --rm --network host minio/mc:latest \
  alias set local "$ENDPOINT" "$ROOT_USER" "$ROOT_PASSWORD"
docker run --rm --network host minio/mc:latest \
  mb --ignore-existing "local/${BUCKET}"
echo "Ready: ${ENDPOINT}/${BUCKET}"
