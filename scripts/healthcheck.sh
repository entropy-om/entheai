#!/usr/bin/env bash
#
# entheai fleet healthcheck.
#   - gateway liveness: GET  redis-gateway /health            (expect 200)
#   - license verify:   POST entheai.com/api/license/verify   (expect 401, NOT 503)
#   - beta releases:    GET  entheai.com/api/releases         (expect 200)
#
# Needs only curl and the gateway bearer token at
# ~/.config/vaked/gateway.token. Exits non-zero if any check fails.

set -uo pipefail

TOKEN_FILE="${HOME}/.config/vaked/gateway.token"
GATEWAY_URL="https://redis-gateway-production.up.railway.app/health"
API_URL="https://entheai.com"
CURL=(curl -s --max-time 15 -o /dev/null -w '%{http_code}')

fails=0

report() {
    local name="$1" status="$2" detail="$3"
    if [ "$status" = "OK" ]; then
        printf 'OK   %s\n' "$name"
    else
        printf 'FAIL %s — %s\n' "$name" "$detail"
        fails=1
    fi
}

# --- 1. Gateway health -----------------------------------------------------
if [ ! -f "$TOKEN_FILE" ]; then
    report "gateway health" FAIL "missing token file: $TOKEN_FILE"
else
    code=$("${CURL[@]}" -H "Authorization: Bearer $(cat "$TOKEN_FILE")" "$GATEWAY_URL")
    if [ "$code" = "200" ]; then
        report "gateway health" OK ""
    elif [ "$code" = "503" ]; then
        report "gateway health" FAIL "got HTTP 503 (redis unreachable), want 200"
    else
        report "gateway health" FAIL "got HTTP ${code:-no response}, want 200"
    fi
fi

# --- 2. License verify with bogus key (expect 401, NOT 503) ---------------
code=$("${CURL[@]}" -X POST -H 'Content-Type: application/json' \
    -d '{"key":"ENTH-BOGUS-KEY0-AAAA-AAAA"}' \
    "$API_URL/api/license/verify")
if [ "$code" = "401" ]; then
    report "license verify" OK ""
elif [ "$code" = "503" ]; then
    report "license verify" FAIL "got HTTP 503 (gateway down), want 401"
else
    report "license verify" FAIL "got HTTP ${code:-no response}, want 401"
fi

# --- 3. Beta releases ------------------------------------------------------
code=$("${CURL[@]}" "$API_URL/api/releases?channel=beta")
if [ "$code" = "200" ]; then
    report "beta releases" OK ""
else
    report "beta releases" FAIL "got HTTP ${code:-no response}, want 200"
fi

exit "$fails"
