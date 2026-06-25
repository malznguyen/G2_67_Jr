#!/usr/bin/env bash
# Seed 5 demo users into the `gmrag` Keycloak realm, each with a distinct
# realm role, so JWT `realm_access.roles` carries a role signal the backend /
# frontend can differentiate. Tenant/workspace membership itself is managed by
# the GMRAG2 API (tenant_members / workspace_members tables) after first login
# auto-provisioning — Keycloak roles here are an identity-level discriminator.
#
# Usage (after bootstrap.sh has created the realm):
#   docker cp infra/keycloak/seed-users.sh gmrag-keycloak:/tmp/seed-users.sh
#   docker exec -e KEYCLOAK_ADMIN=admin -e KEYCLOAK_ADMIN_PASSWORD=<pw> \
#     gmrag-keycloak bash /tmp/seed-users.sh
#
# Idempotent: skips users/roles that already exist.
set -euo pipefail

KC="/opt/keycloak/bin/kcadm.sh"
ADMIN_USER="${KEYCLOAK_ADMIN:-admin}"
ADMIN_PASS="${KEYCLOAK_ADMIN_PASSWORD:?KEYCLOAK_ADMIN_PASSWORD required}"
REALM="${KEYCLOAK_REALM:-gmrag}"
SERVER="http://localhost:8080"
DEFAULT_PASSWORD="${SEED_USER_PASSWORD:-password123}"

"$KC" config credentials --server "$SERVER" --realm master --user "$ADMIN_USER" --password "$ADMIN_PASS"

# Realm roles to create (idempotent).
ROLES=("platform_admin" "tenant_owner" "tenant_member" "workspace_editor" "workspace_viewer")
for r in "${ROLES[@]}"; do
  if "$KC" get "roles/${r}" -r "${REALM}" 2>/dev/null | grep -q "\"name\""; then
    echo "[seed] role ${r} exists — skipping"
  else
    echo "[seed] creating realm role ${r}"
    "$KC" create roles -r "${REALM}" -s "name=${r}" -s "description=${r} role for GMRAG2 demo"
  fi
done

# users: username|email|firstName|lastName|role
USERS=(
  "admin|admin@gmrag.local|Platform|Admin|platform_admin"
  "owner1|owner1@gmrag.local|Tenant|Owner|tenant_owner"
  "member1|member1@gmrag.local|Tenant|Member|tenant_member"
  "editor1|editor1@gmrag.local|Workspace|Editor|workspace_editor"
  "viewer1|viewer1@gmrag.local|Workspace|Viewer|workspace_viewer"
)

for row in "${USERS[@]}"; do
  IFS="|" read -r username email first last role <<< "$row"
  pw="${DEFAULT_PASSWORD}"
  existing=$( "$KC" get "users?username=${username}" -r "${REALM}" 2>/dev/null || true )
  if echo "$existing" | grep -q "\"id\""; then
    echo "[seed] user ${username} exists — skipping create"
  else
    echo "[seed] creating user ${username} (${role})"
    "$KC" create users -r "${REALM}" \
      -s "username=${username}" \
      -s "email=${email}" \
      -s "firstName=${first}" \
      -s "lastName=${last}" \
      -s "enabled=true" \
      -s "emailVerified=true" \
      -s "requiredActions=[]"
    uid="$( "$KC" get "users?username=${username}" -r "${REALM}" | tr -d ' ' | grep -o '\"id\":\"[^\"]*\"' | head -1 | cut -d'"' -f4 )"
    if [ -n "$uid" ]; then
      "$KC" set-password -r "${REALM}" --userid "$uid" --new-password "$pw"
      echo "[seed] set password for ${username}"
    fi
  fi

  # Assign realm role (idempotent: re-assigning is a no-op error we swallow).
  "$KC" add-roles -r "${REALM}" --uusername "${username}" --rolename "${role}" 2>/dev/null \
    || echo "[seed] role ${role} already assigned to ${username}"
done

echo "[seed] done — 5 users ready in realm ${REALM}"
echo "[seed] default password for all: ${DEFAULT_PASSWORD} (override via SEED_<USERNAME>_PASSWORD)"
