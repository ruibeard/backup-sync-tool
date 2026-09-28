#!/usr/bin/env bash
# Create the local backup-dev bucket used by Laravel BACKUP_STORAGE_DRIVER=minio.
# Uses the compose network (Docker Desktop Mac cannot rely on --network host).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
COMPOSE_FILE="${ROOT}/docker-compose.yml"
ROOT_USER="${MINIO_ROOT_USER:-minioadmin}"
ROOT_PASSWORD="${MINIO_ROOT_PASSWORD:-minioadmin}"
BUCKET="${MINIO_BUCKET:-backup-dev}"
NETWORK="${MINIO_DOCKER_NETWORK:-minio_default}"
SERVICE_HOST="${MINIO_SERVICE_HOST:-minio}"

# Ensure compose stack is up (idempotent).
docker compose -f "$COMPOSE_FILE" up -d >/dev/null

# Wait for health on host-mapped port.
for _ in $(seq 1 30); do
  if curl -sf "http://127.0.0.1:9000/minio/health/live" >/dev/null; then
    break
  fi
  sleep 1
done

docker run --rm --network "$NETWORK" --entrypoint /bin/sh minio/mc:latest -c "
  mc alias set local http://${SERVICE_HOST}:9000 '${ROOT_USER}' '${ROOT_PASSWORD}' >/dev/null
  mc mb --ignore-existing \"local/${BUCKET}\"
"
echo "Ready: http://127.0.0.1:9000/${BUCKET}"
