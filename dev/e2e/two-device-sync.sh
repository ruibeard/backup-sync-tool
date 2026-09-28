#!/usr/bin/env bash
# Two-device live sync proof, no Docker:
#   rclone S3 server (local dir) + Laravel on a scratch SQLite DB + desktop engine test.
#   Devices get signed chunk URLs from Laravel; only Laravel knows the S3 key.
# Needs: rclone, php, jq, cargo, and box-rui-cam next to this repo (or LARAVEL_ROOT).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LARAVEL_ROOT="${LARAVEL_ROOT:-$(cd "$ROOT/../box-rui-cam" && pwd)}"
S3_PORT="${S3_PORT:-9100}"
API_PORT="${API_PORT:-8765}"
WORK="$(mktemp -d)"
PIDS=()
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT

S3_ACCESS=e2e-access
S3_SECRET=e2e-secret
S3="http://127.0.0.1:$S3_PORT"
API="http://127.0.0.1:$API_PORT"

mkdir -p "$WORK/s3"
rclone serve s3 --auth-key "$S3_ACCESS,$S3_SECRET" --addr "127.0.0.1:$S3_PORT" "$WORK/s3" >"$WORK/rclone.log" 2>&1 &
PIDS+=($!)

# Process env wins over box-rui-cam/.env (Laravel's dotenv is immutable).
export APP_ENV=local APP_URL="$API" DB_CONNECTION=sqlite DB_DATABASE="$WORK/e2e.sqlite" \
  CACHE_STORE=array SESSION_DRIVER=array QUEUE_CONNECTION=sync \
  BACKUP_STORAGE_DRIVER=minio MINIO_ENABLED=true MINIO_BUCKET=backup-dev \
  MINIO_S3_ENDPOINT="$S3" MINIO_S3_PUBLIC_ENDPOINT="$S3" \
  MINIO_ROOT_USER="$S3_ACCESS" MINIO_ROOT_PASSWORD="$S3_SECRET"
touch "$WORK/e2e.sqlite"
(cd "$LARAVEL_ROOT" && php artisan migrate --force -q)
# server.php serves from the working directory, so run it inside public/.
(cd "$LARAVEL_ROOT/public" && exec php -S "127.0.0.1:$API_PORT" \
  ../vendor/laravel/framework/src/Illuminate/Foundation/resources/server.php) >"$WORK/api.log" 2>&1 &
PIDS+=($!)

for _ in $(seq 50); do
  curl -s "$S3/" -o /dev/null && curl -s "$API/login" -o /dev/null && break
  sleep 0.2
done

# Approve a pairing code as a web admin would (login is Google-only).
cat >"$WORK/approve.php" <<'PHP'
<?php
[$_, $root, $code] = $argv;
require $root.'/vendor/autoload.php';
$app = require $root.'/bootstrap/app.php';
$app->make(Illuminate\Contracts\Console\Kernel::class)->bootstrap();
$user = App\Models\User::unguarded(fn () => App\Models\User::query()->firstOrCreate(
    ['email' => 'e2e@example.test'],
    ['name' => 'E2E', 'password' => bcrypt(Illuminate\Support\Str::random(32))],
));
auth()->setUser($user);
$request = Illuminate\Http\Request::create('/pair/approve', 'POST', ['code' => $code, 'remote_folder' => 'E2E-Shop']);
$request->setLaravelSession($app['session']->driver());
$app->instance('request', $request);
$app->make(App\Http\Controllers\PairingController::class)->approve($request);
if ($errors = $request->session()->get('errors')) {
    fwrite(STDERR, json_encode($errors->all()).PHP_EOL);
    exit(1);
}
PHP

pair() {
  local start code poll
  start=$(curl -fsS -X POST "$API/api/pair/start" -H 'Content-Type: application/json' -H 'Accept: application/json' \
    -d "{\"machine_name\":\"E2E-$1\",\"supported_transports\":[\"chunk_store\"]}")
  code=$(jq -r .code <<<"$start")
  poll=$(jq -r .poll_token <<<"$start")
  php "$WORK/approve.php" "$LARAVEL_ROOT" "$code"
  curl -fsS "$API/api/pair/status/$poll"
}

A=$(pair A)
B=$(pair B)
export BST_E2E_API="$API"
export BST_E2E_A_TOKEN=$(jq -r .device_token <<<"$A") BST_E2E_A_UUID=$(jq -r .device_uuid <<<"$A")
export BST_E2E_B_TOKEN=$(jq -r .device_token <<<"$B") BST_E2E_B_UUID=$(jq -r .device_uuid <<<"$B")

cd "$ROOT"
if ! cargo test -q two_device_sync_e2e -- --ignored --nocapture; then
  echo "--- api.log"; tail -40 "$WORK/api.log"
  echo "--- rclone.log"; tail -40 "$WORK/rclone.log"
  exit 1
fi
echo "OK: two-device sync e2e passed"
