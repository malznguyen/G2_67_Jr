#!/usr/bin/env bash
# Bootstrap the `gmrag` realm + `gmrag-frontend` OIDC client in Keycloak.
#
# Run AFTER a fresh Keycloak bootstrap (empty keycloak_data volume):
#   docker compose --env-file .env -f infra/docker-compose.yml stop keycloak
#   docker volume rm g2-67-jr_keycloak_data || true
#   docker compose --env-file .env -f infra/docker-compose.yml up -d keycloak
#   # wait for healthy, then:
#   docker exec gmrag-keycloak bash /opt/keycloak/bootstrap.sh
#
# Idempotent: re-running is safe (skips realm/client that already exist).
set -euo pipefail

KC="/opt/keycloak/bin/kcadm.sh"
ADMIN_USER="${KEYCLOAK_ADMIN:-admin}"
ADMIN_PASS="${KEYCLOAK_ADMIN_PASSWORD:?KEYCLOAK_ADMIN_PASSWORD required}"
REALM="${KEYCLOAK_REALM:-gmrag}"
FRONTEND_CLIENT="${KEYCLOAK_FRONTEND_CLIENT_ID:-gmrag-frontend}"
BACKEND_CLIENT="${KEYCLOAK_CLIENT_ID:-gmrag-backend}"
SERVER="http://localhost:8080"

echo "[bootstrap] logging in as ${ADMIN_USER} @ master"
"$KC" config credentials --server "$SERVER" --realm master --user "$ADMIN_USER" --password "$ADMIN_PASS"

# --- Realm -----------------------------------------------------------------
if "$KC" get "realms/${REALM}" 2>/dev/null | grep -q "\"realm\""; then
  echo "[bootstrap] realm ${REALM} already exists — skipping"
else
  echo "[bootstrap] creating realm ${REALM}"
  "$KC" create realms -s "realm=${REALM}" -s enabled=true \
    -s "accessTokenLifespan=3600" -s "ssoSessionIdleTimeout=1800" \
    -s "ssoSessionMaxLifespan=36000"
fi

# --- Frontend client (public PKCE, browser) -------------------------------
if "$KC" get "clients?clientId=${FRONTEND_CLIENT}" -r "${REALM}" 2>/dev/null | grep -q "\"clientId\""; then
  echo "[bootstrap] client ${FRONTEND_CLIENT} already exists — skipping"
else
  echo "[bootstrap] creating client ${FRONTEND_CLIENT} (public PKCE)"
  "$KC" create clients -r "${REALM}" \
    -s "clientId=${FRONTEND_CLIENT}" \
    -s "enabled=true" \
    -s "publicClient=true" \
    -s "standardFlowEnabled=true" \
    -s "directAccessGrantsEnabled=false" \
    -s "serviceAccountsEnabled=false" \
    -s "frontchannelLogout=false" \
    -s "redirectUris=[\"http://localhost:3000/api/auth/callback/keycloak\",\"http://localhost:3000\",\"http://0.0.0.0:3000/api/auth/callback/keycloak\",\"http://127.0.0.1:3000/api/auth/callback/keycloak\"]" \
    -s "webOrigins=[\"http://localhost:3000\",\"http://0.0.0.0:3000\",\"http://127.0.0.1:3000\"]" \
    -s "attributes={\"post.logout.redirect.uris\":\"http://localhost:3000*\",\"pkce.code.challenge.method\":\"S256\"}"
fi

# --- Backend client (confidential, service) -------------------------------
if "$KC" get "clients?clientId=${BACKEND_CLIENT}" -r "${REALM}" 2>/dev/null | grep -q "\"clientId\""; then
  echo "[bootstrap] client ${BACKEND_CLIENT} already exists — skipping"
else
  echo "[bootstrap] creating client ${BACKEND_CLIENT} (confidential)"
  "$KC" create clients -r "${REALM}" \
    -s "clientId=${BACKEND_CLIENT}" \
    -s "enabled=true" \
    -s "publicClient=false" \
    -s "standardFlowEnabled=false" \
    -s "directAccessGrantsEnabled=false" \
    -s "serviceAccountsEnabled=true" \
    -s "secret=${KEYCLOAK_CLIENT_SECRET:-}"
fi

echo "[bootstrap] done — realm ${REALM} ready"
