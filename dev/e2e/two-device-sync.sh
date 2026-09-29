#!/usr/bin/env bash
# Two-device live sync proof, no Docker:
#   rclone S3 server (local dir) + Laravel on a scratch SQLite DB + desktop engine test.
#   Devices get signed file URLs from Laravel; only Laravel knows the S3 key.
# Needs: rclone, php, jq, cargo, and box-rui-cam next to this repo (or LARAVEL_ROOT).
#
# E2E_API=https://backup.rui.cam runs the same test against a deployed control plane
# and its real object store. No local stack starts: the script prints two approve
# links and waits until an admin approves both into the same new customer folder.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
S3_PORT="${S3_PORT:-9100}"
API_PORT="${API_PORT:-8765}"
WORK="$(mktemp -d)"
PIDS=()
cleanup() {
  # bash 3.2 (macOS) treats an empty array as unbound under set -u.
  for pid in ${PIDS[@]+"${PIDS[@]}"}; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT

start_local_stack() {
  LARAVEL_ROOT="${LARAVEL_ROOT:-$(cd "$ROOT/../box-rui-cam" && pwd)}"
  local s3_access=e2e-access s3_secret=e2e-secret s3="http://127.0.0.1:$S3_PORT"
  API="http://127.0.0.1:$API_PORT"

  mkdir -p "$WORK/s3"
  rclone serve s3 --auth-key "$s3_access,$s3_secret" --addr "127.0.0.1:$S3_PORT" "$WORK/s3" >"$WORK/rclone.log" 2>&1 &
  PIDS+=($!)

  # Process env wins over box-rui-cam/.env (Laravel's dotenv is immutable).
  export APP_ENV=local APP_URL="$API" DB_CONNECTION=sqlite DB_DATABASE="$WORK/e2e.sqlite" \
    CACHE_STORE=array SESSION_DRIVER=array QUEUE_CONNECTION=sync \
    BACKUP_STORAGE_DRIVER=minio MINIO_ENABLED=true MINIO_BUCKET=backup-dev \
    MINIO_S3_ENDPOINT="$s3" MINIO_S3_PUBLIC_ENDPOINT="$s3" \
    MINIO_ROOT_USER="$s3_access" MINIO_ROOT_PASSWORD="$s3_secret"
  touch "$WORK/e2e.sqlite"
  (cd "$LARAVEL_ROOT" && php artisan migrate --force -q)
  # server.php serves from the working directory, so run it inside public/.
  (cd "$LARAVEL_ROOT/public" && exec php -S "127.0.0.1:$API_PORT" \
    ../vendor/laravel/framework/src/Illuminate/Foundation/resources/server.php) >"$WORK/api.log" 2>&1 &
  PIDS+=($!)

  for _ in $(seq 50); do
    curl -s "$s3/" -o /dev/null && curl -s "$API/login" -o /dev/null && break
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
}

pair_start() {
  curl -fsS -X POST "$API/api/pair/start" -H 'Content-Type: application/json' -H 'Accept: application/json' \
    -d "{\"machine_name\":\"E2E-$1\",\"supported_transports\":[\"chunk_store\"]}"
}

# Poll until approved (the payload is handed out once), up to 15 minutes.
pair_wait() {
  local status state
  for _ in $(seq 300); do
    status=$(curl -fsS "$API/api/pair/status/$1")
    state=$(jq -r .status <<<"$status")
    case "$state" in
      approved) echo "$status"; return 0 ;;
      pending) sleep 3 ;;
      *) echo "Pairing ended as $state" >&2; return 1 ;;
    esac
  done
  echo "Pairing not approved in time" >&2
  return 1
}

if [[ -n "${E2E_API:-}" ]]; then
  API="${E2E_API%/}"
  START_A=$(pair_start A)
  START_B=$(pair_start B)
  echo "Approve both into the same NEW customer folder (for example bst-e2e-test):"
  echo "  A: $(jq -r .approve_url <<<"$START_A")"
  echo "  B: $(jq -r .approve_url <<<"$START_B")"
  A=$(pair_wait "$(jq -r .poll_token <<<"$START_A")") || exit 1
  B=$(pair_wait "$(jq -r .poll_token <<<"$START_B")") || exit 1
  if [[ "$(jq -r .destination_uuid <<<"$A")" != "$(jq -r .destination_uuid <<<"$B")" ]]; then
    echo "A and B were approved into different folders"
    exit 1
  fi
else
  start_local_stack
  pair_local() {
    local start
    start=$(pair_start "$1")
    php "$WORK/approve.php" "$LARAVEL_ROOT" "$(jq -r .code <<<"$start")" || exit 1
    curl -fsS "$API/api/pair/status/$(jq -r .poll_token <<<"$start")"
  }
  A=$(pair_local A) || { echo "Pairing A failed"; exit 1; }
  B=$(pair_local B) || { echo "Pairing B failed"; exit 1; }
fi

export BST_E2E_API="$API"
export BST_E2E_A_TOKEN=$(jq -r .device_token <<<"$A") BST_E2E_A_UUID=$(jq -r .device_uuid <<<"$A")
export BST_E2E_B_TOKEN=$(jq -r .device_token <<<"$B") BST_E2E_B_UUID=$(jq -r .device_uuid <<<"$B")

cd "$ROOT"
if ! cargo test -q two_device_sync_e2e -- --ignored --nocapture; then
  [[ -f "$WORK/api.log" ]] && { echo "--- api.log"; tail -40 "$WORK/api.log"; }
  [[ -f "$WORK/rclone.log" ]] && { echo "--- rclone.log"; tail -40 "$WORK/rclone.log"; }
  exit 1
fi
echo "OK: two-device sync e2e passed (${E2E_API:-local rclone})"
