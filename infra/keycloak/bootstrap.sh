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
frontend_client_id=""
existing_frontend_client=$( "$KC" get "clients?clientId=${FRONTEND_CLIENT}" -r "${REALM}" 2>/dev/null || true )
if echo "$existing_frontend_client" | grep -q "\"clientId\""; then
  echo "[bootstrap] client ${FRONTEND_CLIENT} already exists — skipping"
  frontend_client_id="$( echo "$existing_frontend_client" | tr -d ' ' | grep -o '\"id\":\"[^\"]*\"' | head -1 | cut -d'"' -f4 )"
  "$KC" update "clients/${frontend_client_id}" -r "${REALM}" \
    -s "publicClient=true" \
    -s "directAccessGrantsEnabled=false" \
    -s "serviceAccountsEnabled=false"
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
  frontend_client_id="$( "$KC" get "clients?clientId=${FRONTEND_CLIENT}" -r "${REALM}" | tr -d ' ' | grep -o '\"id\":\"[^\"]*\"' | head -1 | cut -d'"' -f4 )"
fi

# --- Audience mapper for gmrag-frontend -----------------------------------
# Browser login access tokens otherwise carry `aud=account` and only put the
# frontend client in `azp`. The API validates `aud`, so force
# `aud=gmrag-frontend` onto tokens minted for the public frontend client.
frontend_mapper_name="aud-gmrag-frontend"
frontend_mapper_exists=false
if [ -n "$frontend_client_id" ] && [ "$frontend_client_id" != "null" ]; then
  if "$KC" get "clients/${frontend_client_id}/protocol-mappers/models" -r "${REALM}" 2>/dev/null \
      | tr -d ' ' | grep -q "\"name\":\"${frontend_mapper_name}\""; then
    frontend_mapper_exists=true
  fi
fi
if $frontend_mapper_exists; then
  echo "[bootstrap] audience mapper '${frontend_mapper_name}' already exists — skipping"
else
  echo "[bootstrap] adding audience mapper (aud=${FRONTEND_CLIENT}) to ${FRONTEND_CLIENT}"
  "$KC" create "clients/${frontend_client_id}/protocol-mappers/models" -r "${REALM}" \
    -s "name=${frontend_mapper_name}" \
    -s "protocol=openid-connect" \
    -s "protocolMapper=oidc-audience-mapper" \
    -s 'config."included.client.audience"='"\"${FRONTEND_CLIENT}\"" \
    -s 'config."id.token.claim"="false"' \
    -s 'config."access.token.claim"="true"' \
    -s 'config."userinfo.token.claim"="false"'
fi

# --- Backend client (confidential, service) -------------------------------
# Resolves the backend client UUID (creates it if missing) so we can attach
# protocol mappers regardless of whether the client pre-existed.
backend_client_id=""
existing_client=$( "$KC" get "clients?clientId=${BACKEND_CLIENT}" -r "${REALM}" 2>/dev/null || true )
if echo "$existing_client" | grep -q "\"id\""; then
  echo "[bootstrap] client ${BACKEND_CLIENT} already exists — skipping create"
  backend_client_id="$( echo "$existing_client" | tr -d ' ' | grep -o '\"id\":\"[^\"]*\"' | head -1 | cut -d'"' -f4 )"
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
  backend_client_id="$( "$KC" get "clients?clientId=${BACKEND_CLIENT}" -r "${REALM}" | tr -d ' ' | grep -o '\"id\":\"[^\"]*\"' | head -1 | cut -d'"' -f4 )"
fi

# --- Audience mapper for gmrag-backend ------------------------------------
# Without an `aud` claim, service-account tokens issued for ${BACKEND_CLIENT}
# carry only the client id in `azp` and the API rejects them with
# InvalidAudience. This mapper forces `aud=gmrag-backend` (configurable
# via BACKEND_AUDIENCE) so the backend JWT validator accepts the token.
# Idempotent: re-running finds the existing mapper by name and skips.
BACKEND_AUDIENCE="${BACKEND_AUDIENCE:-${BACKEND_CLIENT}}"
mapper_name="aud-gmrag-backend"
mapper_exists=false
if [ -n "$backend_client_id" ] && [ "$backend_client_id" != "null" ]; then
  if "$KC" get "clients/${backend_client_id}/protocol-mappers/models" -r "${REALM}" 2>/dev/null \
      | tr -d ' ' | grep -q "\"name\":\"${mapper_name}\""; then
    mapper_exists=true
  fi
fi
if $mapper_exists; then
  echo "[bootstrap] audience mapper '${mapper_name}' already exists — skipping"
else
  echo "[bootstrap] adding audience mapper (aud=${BACKEND_AUDIENCE}) to ${BACKEND_CLIENT}"
  "$KC" create "clients/${backend_client_id}/protocol-mappers/models" -r "${REALM}" \
    -s "name=${mapper_name}" \
    -s "protocol=openid-connect" \
    -s "protocolMapper=oidc-audience-mapper" \
    -s 'config."included.client.audience"='"\"${BACKEND_AUDIENCE}\"" \
    -s 'config."id.token.claim"="false"' \
    -s 'config."access.token.claim"="true"' \
    -s 'config."userinfo.token.claim"="false"'
fi

echo "[bootstrap] done — realm ${REALM} ready"
