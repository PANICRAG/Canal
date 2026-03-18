#!/bin/bash
# ============================================================================
# CPDP E2E Hosting Test Suite
# ============================================================================
# Tests the full hosting flow against a running platform-service instance.
#
# Usage:
#   ./tests/e2e/hosting_e2e.sh
#   PLATFORM_URL=http://staging:8080 ./tests/e2e/hosting_e2e.sh
#
# Prerequisites:
#   - platform-service running (docker compose or local)
#   - curl, jq installed
# ============================================================================

set -euo pipefail

BASE_URL="${PLATFORM_URL:-http://localhost:8080}"
API_KEY="${API_KEY:-test-api-key-for-testing}"
AUTH="Authorization: Bearer $API_KEY"
ORG_ID="00000000-0000-0000-0000-000000000001"

PASS=0
FAIL=0
SKIP=0

pass() { PASS=$((PASS + 1)); echo "PASS"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL${1:+ ($1)}"; }
skip() { SKIP=$((SKIP + 1)); echo "SKIP${1:+ ($1)}"; }

echo "=== CPDP E2E Hosting Test Suite ==="
echo "Platform: $BASE_URL"
echo "Time:     $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

# --------------------------------------------------------------------------
# T1: Health check
# --------------------------------------------------------------------------
echo -n "T1  Health check... "
HTTP_CODE=$(curl -sf -o /dev/null -w "%{http_code}" "$BASE_URL/health" 2>/dev/null) || HTTP_CODE="000"
if [ "$HTTP_CODE" = "200" ]; then pass; else fail "HTTP $HTTP_CODE — is the service running?"; exit 1; fi

# --------------------------------------------------------------------------
# T2: Create app
# --------------------------------------------------------------------------
echo -n "T2  Create app... "
APP=$(curl -sf -X POST "$BASE_URL/api/hosting/apps" \
  -H "$AUTH" -H "Content-Type: application/json" \
  -d "{\"name\":\"e2e-test-app-$(date +%s)\",\"framework\":\"static\",\"org_id\":\"$ORG_ID\"}" 2>/dev/null) || APP=""
APP_ID=$(echo "$APP" | jq -r '.id // empty' 2>/dev/null)
if [ -n "$APP_ID" ]; then pass; echo "       app_id=$APP_ID"; else fail "no app_id in response"; exit 1; fi

# --------------------------------------------------------------------------
# T3: List apps (should contain at least 1)
# --------------------------------------------------------------------------
echo -n "T3  List apps... "
APPS=$(curl -sf "$BASE_URL/api/hosting/apps" -H "$AUTH" 2>/dev/null) || APPS="[]"
COUNT=$(echo "$APPS" | jq '. | length' 2>/dev/null)
if [ "$COUNT" -ge 1 ] 2>/dev/null; then pass; echo "       count=$COUNT"; else fail "count=$COUNT"; fi

# --------------------------------------------------------------------------
# T4: Get app by ID
# --------------------------------------------------------------------------
echo -n "T4  Get app... "
GOT_APP=$(curl -sf "$BASE_URL/api/hosting/apps/$APP_ID" -H "$AUTH" 2>/dev/null) || GOT_APP=""
GOT_ID=$(echo "$GOT_APP" | jq -r '.id // empty' 2>/dev/null)
if [ "$GOT_ID" = "$APP_ID" ]; then pass; else fail "expected $APP_ID, got $GOT_ID"; fi

# --------------------------------------------------------------------------
# T5: Set env var (requires hosting feature)
# --------------------------------------------------------------------------
echo -n "T5  Set env var... "
ENV_RESP=$(curl -sf -o /dev/null -w "%{http_code}" -X POST "$BASE_URL/api/hosting/apps/$APP_ID/env" \
  -H "$AUTH" -H "Content-Type: application/json" \
  -d '{"key":"NODE_ENV","value":"production","scope":"production"}' 2>/dev/null) || ENV_RESP="000"
if [ "$ENV_RESP" = "200" ]; then pass
elif [ "$ENV_RESP" = "404" ] || [ "$ENV_RESP" = "405" ]; then skip "hosting feature not enabled"
else fail "HTTP $ENV_RESP"; fi

# --------------------------------------------------------------------------
# T6: List env vars
# --------------------------------------------------------------------------
echo -n "T6  List env vars... "
ENVS=$(curl -sf "$BASE_URL/api/hosting/apps/$APP_ID/env" -H "$AUTH" 2>/dev/null) || ENVS=""
ENV_CODE=$(curl -sf -o /dev/null -w "%{http_code}" "$BASE_URL/api/hosting/apps/$APP_ID/env" -H "$AUTH" 2>/dev/null) || ENV_CODE="000"
if [ "$ENV_CODE" = "200" ]; then
  ENV_COUNT=$(echo "$ENVS" | jq '. | length' 2>/dev/null || echo "0")
  pass; echo "       count=$ENV_COUNT"
elif [ "$ENV_CODE" = "404" ] || [ "$ENV_CODE" = "405" ]; then skip "hosting feature not enabled"
else fail "HTTP $ENV_CODE"; fi

# --------------------------------------------------------------------------
# T7: Create deploy hook
# --------------------------------------------------------------------------
echo -n "T7  Create deploy hook... "
HOOK_CODE=$(curl -sf -o /tmp/e2e_hook.json -w "%{http_code}" -X POST "$BASE_URL/api/hosting/apps/$APP_ID/hooks" \
  -H "$AUTH" -H "Content-Type: application/json" \
  -d '{"name":"CMS webhook"}' 2>/dev/null) || HOOK_CODE="000"
if [ "$HOOK_CODE" = "201" ]; then
  HOOK_URL=$(jq -r '.url // empty' /tmp/e2e_hook.json 2>/dev/null)
  pass; echo "       hook_url=$HOOK_URL"
elif [ "$HOOK_CODE" = "404" ] || [ "$HOOK_CODE" = "405" ]; then skip "hosting feature not enabled"
else fail "HTTP $HOOK_CODE"; fi

# --------------------------------------------------------------------------
# T8: List previews (should be empty initially)
# --------------------------------------------------------------------------
echo -n "T8  List previews... "
PREVIEW_CODE=$(curl -sf -o /tmp/e2e_previews.json -w "%{http_code}" "$BASE_URL/api/hosting/apps/$APP_ID/previews" -H "$AUTH" 2>/dev/null) || PREVIEW_CODE="000"
if [ "$PREVIEW_CODE" = "200" ]; then
  P_COUNT=$(jq '. | length' /tmp/e2e_previews.json 2>/dev/null || echo "0")
  pass; echo "       count=$P_COUNT"
elif [ "$PREVIEW_CODE" = "404" ] || [ "$PREVIEW_CODE" = "405" ]; then skip "hosting feature not enabled"
else fail "HTTP $PREVIEW_CODE"; fi

# --------------------------------------------------------------------------
# T9: Deployment history
# --------------------------------------------------------------------------
echo -n "T9  Deployment history... "
HIST_CODE=$(curl -sf -o /tmp/e2e_history.json -w "%{http_code}" "$BASE_URL/api/hosting/apps/$APP_ID/deployment-history" -H "$AUTH" 2>/dev/null) || HIST_CODE="000"
if [ "$HIST_CODE" = "200" ]; then
  H_COUNT=$(jq '. | length' /tmp/e2e_history.json 2>/dev/null || echo "0")
  pass; echo "       count=$H_COUNT"
elif [ "$HIST_CODE" = "404" ] || [ "$HIST_CODE" = "405" ]; then skip "hosting feature not enabled"
else fail "HTTP $HIST_CODE"; fi

# --------------------------------------------------------------------------
# T10: KV set (requires storage feature)
# --------------------------------------------------------------------------
echo -n "T10 KV set... "
KV_SET_CODE=$(curl -sf -o /dev/null -w "%{http_code}" -X POST "$BASE_URL/api/hosting/apps/$APP_ID/kv" \
  -H "$AUTH" -H "Content-Type: application/json" \
  -d '{"command":"set","key":"test","value":"hello","ex":60}' 2>/dev/null) || KV_SET_CODE="000"
if [ "$KV_SET_CODE" = "200" ]; then pass
elif [ "$KV_SET_CODE" = "404" ] || [ "$KV_SET_CODE" = "405" ]; then skip "storage feature not enabled"
else fail "HTTP $KV_SET_CODE"; fi

# --------------------------------------------------------------------------
# T11: KV get
# --------------------------------------------------------------------------
echo -n "T11 KV get... "
if [ "$KV_SET_CODE" = "200" ]; then
  KV_RESULT=$(curl -sf -X POST "$BASE_URL/api/hosting/apps/$APP_ID/kv" \
    -H "$AUTH" -H "Content-Type: application/json" \
    -d '{"command":"get","key":"test"}' 2>/dev/null) || KV_RESULT=""
  KV_VAL=$(echo "$KV_RESULT" | jq -r '.result // empty' 2>/dev/null)
  if [ "$KV_VAL" = "hello" ]; then pass; echo "       value=$KV_VAL"
  else fail "expected 'hello', got '$KV_VAL'"; fi
else skip "KV set failed or skipped"; fi

# --------------------------------------------------------------------------
# T12: Create cron job (requires compute feature)
# --------------------------------------------------------------------------
echo -n "T12 Create cron job... "
CRON_CODE=$(curl -sf -o /tmp/e2e_cron.json -w "%{http_code}" -X POST "$BASE_URL/api/hosting/apps/$APP_ID/crons" \
  -H "$AUTH" -H "Content-Type: application/json" \
  -d '{"path":"/api/cleanup","schedule":"0 * * * *"}' 2>/dev/null) || CRON_CODE="000"
if [ "$CRON_CODE" = "201" ] || [ "$CRON_CODE" = "200" ]; then
  CRON_ID=$(jq -r '.id // empty' /tmp/e2e_cron.json 2>/dev/null)
  pass; echo "       cron_id=$CRON_ID"
elif [ "$CRON_CODE" = "404" ] || [ "$CRON_CODE" = "405" ]; then skip "compute feature not enabled"
else fail "HTTP $CRON_CODE"; fi

# --------------------------------------------------------------------------
# T13: List serverless functions (requires compute feature)
# --------------------------------------------------------------------------
echo -n "T13 List functions... "
FUNC_CODE=$(curl -sf -o /tmp/e2e_funcs.json -w "%{http_code}" "$BASE_URL/api/hosting/apps/$APP_ID/serverless" -H "$AUTH" 2>/dev/null) || FUNC_CODE="000"
if [ "$FUNC_CODE" = "200" ]; then
  F_COUNT=$(jq '. | length' /tmp/e2e_funcs.json 2>/dev/null || echo "0")
  pass; echo "       count=$F_COUNT"
elif [ "$FUNC_CODE" = "404" ] || [ "$FUNC_CODE" = "405" ]; then skip "compute feature not enabled"
else fail "HTTP $FUNC_CODE"; fi

# --------------------------------------------------------------------------
# T14: Create firewall rule (requires security feature)
# --------------------------------------------------------------------------
echo -n "T14 Create firewall rule... "
FW_CODE=$(curl -sf -o /dev/null -w "%{http_code}" -X POST "$BASE_URL/api/hosting/apps/$APP_ID/firewall/rules" \
  -H "$AUTH" -H "Content-Type: application/json" \
  -d '{"rule_type":"ip_block","config":{"ip":"1.2.3.4"},"action":"block","priority":1,"enabled":true}' 2>/dev/null) || FW_CODE="000"
if [ "$FW_CODE" = "201" ] || [ "$FW_CODE" = "200" ]; then pass
elif [ "$FW_CODE" = "404" ] || [ "$FW_CODE" = "405" ]; then skip "security feature not enabled"
else fail "HTTP $FW_CODE"; fi

# --------------------------------------------------------------------------
# T15: Get analytics (requires analytics feature)
# --------------------------------------------------------------------------
echo -n "T15 Get analytics... "
AN_CODE=$(curl -sf -o /dev/null -w "%{http_code}" "$BASE_URL/api/hosting/apps/$APP_ID/analytics" -H "$AUTH" 2>/dev/null) || AN_CODE="000"
if [ "$AN_CODE" = "200" ]; then pass
elif [ "$AN_CODE" = "404" ] || [ "$AN_CODE" = "405" ]; then skip "analytics feature not enabled"
else fail "HTTP $AN_CODE"; fi

# --------------------------------------------------------------------------
# T16: List templates (requires dx feature)
# --------------------------------------------------------------------------
echo -n "T16 List templates... "
TPL_CODE=$(curl -sf -o /tmp/e2e_templates.json -w "%{http_code}" "$BASE_URL/api/hosting/templates" -H "$AUTH" 2>/dev/null) || TPL_CODE="000"
if [ "$TPL_CODE" = "200" ]; then
  T_COUNT=$(jq '. | length' /tmp/e2e_templates.json 2>/dev/null || echo "0")
  pass; echo "       count=$T_COUNT"
elif [ "$TPL_CODE" = "404" ] || [ "$TPL_CODE" = "405" ]; then skip "dx feature not enabled"
else fail "HTTP $TPL_CODE"; fi

# --------------------------------------------------------------------------
# T17: Update app
# --------------------------------------------------------------------------
echo -n "T17 Update app... "
UPD_CODE=$(curl -sf -o /dev/null -w "%{http_code}" -X PUT "$BASE_URL/api/hosting/apps/$APP_ID" \
  -H "$AUTH" -H "Content-Type: application/json" \
  -d '{"name":"e2e-updated-app"}' 2>/dev/null) || UPD_CODE="000"
if [ "$UPD_CODE" = "200" ]; then pass
else fail "HTTP $UPD_CODE"; fi

# --------------------------------------------------------------------------
# T18: Delete app
# --------------------------------------------------------------------------
echo -n "T18 Delete app... "
DEL_CODE=$(curl -sf -o /dev/null -w "%{http_code}" -X DELETE "$BASE_URL/api/hosting/apps/$APP_ID" -H "$AUTH" 2>/dev/null) || DEL_CODE="000"
if [ "$DEL_CODE" = "204" ] || [ "$DEL_CODE" = "200" ]; then pass
else fail "HTTP $DEL_CODE"; fi

# --------------------------------------------------------------------------
# T19: Verify app is gone
# --------------------------------------------------------------------------
echo -n "T19 Verify deleted... "
GONE_CODE=$(curl -sf -o /dev/null -w "%{http_code}" "$BASE_URL/api/hosting/apps/$APP_ID" -H "$AUTH" 2>/dev/null) || GONE_CODE="000"
if [ "$GONE_CODE" = "404" ]; then pass
else fail "expected 404, got HTTP $GONE_CODE"; fi

# --------------------------------------------------------------------------
# Cleanup temp files
# --------------------------------------------------------------------------
rm -f /tmp/e2e_hook.json /tmp/e2e_previews.json /tmp/e2e_history.json \
      /tmp/e2e_cron.json /tmp/e2e_funcs.json /tmp/e2e_templates.json

# --------------------------------------------------------------------------
# Summary
# --------------------------------------------------------------------------
TOTAL=$((PASS + FAIL + SKIP))
echo ""
echo "=== E2E Test Complete ==="
echo "Total: $TOTAL | Pass: $PASS | Fail: $FAIL | Skip: $SKIP"
echo ""

if [ "$FAIL" -gt 0 ]; then
  echo "RESULT: FAILED"
  exit 1
else
  echo "RESULT: OK"
  exit 0
fi
