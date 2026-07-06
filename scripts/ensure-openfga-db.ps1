# =========================================================
# ensure-openfga-db.ps1
# Idempotently creates the `openfga` logical database inside the
# local Postgres container.
#
# WHY: infra/postgres/init.sql only runs on FIRST volume init
# (docker-entrypoint-initdb.d). On a reused `gmrag-pgdata` volume,
# the `openfga` database silently does not exist, and the
# `openfga-migrate` container fails with "database openfga does not
# exist" — or worse, hangs if OpenFGA retries forever.
#
# Run this AFTER postgres16 is healthy and BEFORE openfga-migrate:
#   pwsh ./scripts/ensure-openfga-db.ps1
#
# Safe to re-run: uses CREATE DATABASE ... WHERE NOT EXISTS.
# =========================================================
param(
    [string]$Container = "gmrag-postgres16",
    [string]$PostgresUser = $env:POSTGRES_USER,
    [string]$DbOwner = $env:POSTGRES_USER,
    [string]$DbName = "openfga"
)

$ErrorActionPreference = "Stop"

if (-not $PostgresUser) {
    throw "POSTGRES_USER is required (set it in .env or pass -PostgresUser)."
}

# psql is invoked inside the container, so no host psql dependency.
# The SELECT ... \gexec idiom is the same one used by init.sql and
# avoids a conditional CREATE DATABASE that would error on re-runs.
$sql = @"
SELECT 'CREATE DATABASE $DbName OWNER $DbOwner'
WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = '$DbName')\gexec
"@

Write-Host "[ensure-openfga-db] ensuring database '$DbName' exists in container '$Container' (owner=$DbOwner)"
$psqlArgs = @(
    "exec", $Container,
    "psql", "-v", "ON_ERROR_STOP=1", "-U", $PostgresUser, "-d", "postgres"
)

& docker @psqlArgs --command $sql
if ($LASTEXITCODE -ne 0) {
    throw "Failed to ensure '$DbName' database exists (exit $LASTEXITCODE). Is postgres16 healthy?"
}

Write-Host "[ensure-openfga-db] OK — '$DbName' database is present."