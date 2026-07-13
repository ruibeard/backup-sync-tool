#!/usr/bin/env bash
# Ensure a stable local codesign identity exists (stops Keychain prompts after each rebuild).
# Prints the identity SHA-1 hash (preferred) or name for codesign --sign.
# Default common name: "Backup Sync Tool Dev"
set -euo pipefail

IDENTITY="${MACOS_SIGN_IDENTITY:-Backup Sync Tool Dev}"
LOGIN_KC="${HOME}/Library/Keychains/login.keychain-db"
if [[ ! -f "$LOGIN_KC" ]]; then
  LOGIN_KC="${HOME}/Library/Keychains/login.keychain"
fi

# If MACOS_SIGN_IDENTITY looks like a 40-char hex hash, use it directly.
if [[ "$IDENTITY" =~ ^[0-9A-Fa-f]{40}$ ]]; then
  echo "$IDENTITY"
  exit 0
fi

# Prefer a trusted (-v) match; else any codesigning match (self-signed may be NOT_TRUSTED).
pick_hash() {
  local line hash
  line="$(security find-identity -v -p codesigning 2>/dev/null | grep -F "\"$IDENTITY\"" | grep -v CSSMERR | head -1 || true)"
  if [[ -z "$line" ]]; then
    line="$(security find-identity -p codesigning 2>/dev/null | grep -F "\"$IDENTITY\"" | head -1 || true)"
  fi
  hash="$(awk '{print $2}' <<<"$line")"
  [[ -n "$hash" ]] && echo "$hash"
}

EXISTING="$(pick_hash || true)"
if [[ -n "${EXISTING:-}" ]]; then
  echo "$EXISTING"
  exit 0
fi

TMP="$(mktemp -d)"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

# Self-signed code-signing cert (10y), local machine only — not for notarization.
openssl genrsa -out "$TMP/key.pem" 2048 2>/dev/null
cat >"$TMP/ext.cnf" <<'EOF'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=critical,codeSigning
subjectKeyIdentifier=hash
EOF
openssl req -new -key "$TMP/key.pem" -out "$TMP/csr.pem" -subj "/CN=${IDENTITY}" 2>/dev/null
openssl x509 -req -in "$TMP/csr.pem" -signkey "$TMP/key.pem" -out "$TMP/cert.pem" \
  -days 3650 -extfile "$TMP/ext.cnf" 2>/dev/null
# OpenSSL 3 defaults break macOS SecKeychainItemImport; -legacy uses compatible PBE.
openssl pkcs12 -export -out "$TMP/ident.p12" -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
  -name "$IDENTITY" -passout pass:bst-dev -legacy 2>/dev/null

# -A: allow any app to use this key (needed so codesign can use it non-interactively).
# Do not call add-trusted-cert here — it blocks on a Keychain password dialog.
# codesign accepts the identity by SHA-1 hash even when find-identity -v marks it NOT_TRUSTED.
security import "$TMP/ident.p12" -k "$LOGIN_KC" -P bst-dev -A -T /usr/bin/codesign -T /usr/bin/security >/dev/null

# Unlock partition list so codesign can use the private key without UI (best-effort).
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "" "$LOGIN_KC" >/dev/null 2>&1 || true

HASH="$(pick_hash || true)"
if [[ -z "${HASH:-}" ]]; then
  echo "error: failed to install codesign identity \"$IDENTITY\"" >&2
  exit 1
fi

echo "Created codesign identity: $IDENTITY ($HASH)" >&2
echo "$HASH"
