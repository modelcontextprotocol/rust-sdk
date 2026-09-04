#!/usr/bin/env bash
# =============================================================================
# keycloak-oauth-fixture.sh — Live OAuth fixture for rmcp refresh tests
#
# Starts a throwaway Keycloak and provisions the realm that
# crates/rmcp/tests/test_live_oauth_refresh.rs expects. Those tests are
# #[ignore]d, so nothing here runs in CI.
#
#   Provision:  ./scripts/keycloak-oauth-fixture.sh
#   Run tests:  cargo test -p rmcp --all-features \
#                 --test test_live_oauth_refresh -- --ignored
#   Tear down:  docker rm -f kc-rmcp-test
#
# Requires Docker. Override the port with KC_BASE (default localhost:8081);
# the tests read the same variable.
# =============================================================================
set -euo pipefail

KC=${KC_BASE:-http://localhost:8081}
PORT=${KC##*:}
CONTAINER=kc-rmcp-test

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" -p "$PORT:8080" \
  -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin \
  quay.io/keycloak/keycloak:26.0 start-dev >/dev/null

echo "waiting for keycloak at $KC ..."
until curl -sf -o /dev/null "$KC/realms/master/.well-known/openid-configuration"; do
  sleep 3
done

token=$(curl -s -X POST "$KC/realms/master/protocol/openid-connect/token" \
  -d client_id=admin-cli -d username=admin -d password=admin -d grant_type=password |
  python3 -c 'import sys,json;print(json.load(sys.stdin)["access_token"])')

provision() {
  curl -s -X POST "$KC/admin/realms$1" \
    -H "Authorization: Bearer $token" -H "Content-Type: application/json" \
    -d "$2" -o /dev/null -w "  ${1:-/} -> %{http_code}\n"
}

# revokeRefreshToken makes refresh tokens single-use. The concurrency test
# depends on it: without the refresh guard the second caller replays a consumed
# token and Keycloak answers invalid_grant.
provision "" '{"realm":"rmcp","enabled":true,"revokeRefreshToken":true,"refreshTokenMaxReuse":0}'
provision "/rmcp/clients" '{"clientId":"rmcp-client","secret":"rmcp-secret","publicClient":false,
  "directAccessGrantsEnabled":true,"standardFlowEnabled":true,
  "redirectUris":["http://localhost/callback"]}'
# The profile fields and empty requiredActions keep the direct access grant from
# failing with "Account is not fully set up".
provision "/rmcp/users" '{"username":"alice","enabled":true,"emailVerified":true,
  "email":"alice@example.com","firstName":"Alice","lastName":"Example","requiredActions":[],
  "credentials":[{"type":"password","value":"alice-pw","temporary":false}]}'

echo "ready"
