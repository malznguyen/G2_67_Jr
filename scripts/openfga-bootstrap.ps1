param(
    [string]$ApiUrl = $env:FGA_API_URL,
    [string]$ApiToken = $env:FGA_API_TOKEN,
    [string]$StoreName = $env:OPENFGA_STORE_NAME,
    [string]$ModelFile = "infra/openfga/model.fga"
)

$ErrorActionPreference = "Stop"

function Load-DotEnv {
    param([string]$Path = ".env")

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    Get-Content -LiteralPath $Path | ForEach-Object {
        $line = $_.Trim()
        if (-not $line -or $line.StartsWith("#") -or -not $line.Contains("=")) {
            return
        }

        $idx = $line.IndexOf("=")
        $key = $line.Substring(0, $idx).Trim()
        $value = $line.Substring($idx + 1).Trim().Trim('"').Trim("'")
        if ($key -and -not [Environment]::GetEnvironmentVariable($key, "Process")) {
            [Environment]::SetEnvironmentVariable($key, $value, "Process")
        }
    }
}

Load-DotEnv

if (-not $ApiUrl) {
    $ApiUrl = $env:OPENFGA_API_URL
}
if (-not $ApiToken) {
    $ApiToken = $env:OPENFGA_API_TOKEN
}
if (-not $StoreName) {
    $StoreName = "gmrag-v2"
}
if (-not (Get-Command fga -ErrorAction SilentlyContinue)) {
    throw "OpenFGA CLI 'fga' is required. Install it from https://github.com/openfga/cli."
}
if (-not (Test-Path -LiteralPath $ModelFile)) {
    throw "Model file not found: $ModelFile"
}

$common = @("--api-url", $ApiUrl)
if ($ApiToken) {
    $common += @("--api-token", $ApiToken)
}

# ---- Resolve (or create) the store, without creating duplicates. ----
$storesRaw = & fga store list @common
$stores = $storesRaw | ConvertFrom-Json
$store = @($stores.stores) | Where-Object { $_.name -eq $StoreName } | Select-Object -First 1

if ($store) {
    $storeId = $store.id
} else {
    # `fga store create` returns {"store": {"id": ..., "name": ..., ...}},
    # NOT a flat {"id": ...} object.
    $created = (& fga store create --name $StoreName @common) | ConvertFrom-Json
    $storeId = $created.store.id
}
if (-not $storeId) {
    throw "Could not resolve OpenFGA store id for '$StoreName'."
}

# ---- Publish the model only if it actually changed. ----
# `fga model transform --output-format fga` and `fga model get --format fga`
# both round-trip through the same DSL canonicalizer, so comparing their
# output (rather than raw file text or raw JSON, which include
# server-populated defaults) reliably detects "no-op" runs and avoids
# minting a new authorization_model_id on every bootstrap invocation.
$desiredDsl = ((& fga model transform --file $ModelFile --output-format fga @common) -join "`n").Trim()

$modelId = $null
$modelsRaw = & fga model list --store-id $storeId @common
$models = $modelsRaw | ConvertFrom-Json
if ($models.authorization_models -and $models.authorization_models.Count -gt 0) {
    # OpenFGA returns authorization models newest-first.
    $latestModelId = $models.authorization_models[0].id
    $currentDsl = ((& fga model get --store-id $storeId --model-id $latestModelId --format fga --field model @common) -join "`n").Trim()
    if ($currentDsl -eq $desiredDsl) {
        $modelId = $latestModelId
    }
}

if (-not $modelId) {
    $written = (& fga model write --store-id $storeId --file $ModelFile --format fga @common) | ConvertFrom-Json
    $modelId = $written.authorization_model_id
    if (-not $modelId) {
        $modelId = $written.id
    }
}
if (-not $modelId) {
    throw "Could not resolve authorization model id from fga model write output."
}

# ---- Verify the model can be read back before reporting success. ----
$verify = (& fga model get --store-id $storeId --model-id $modelId --format json --field model @common) | ConvertFrom-Json
if (-not $verify.schema_version) {
    throw "Model $modelId could not be read back from store $storeId."
}

Write-Output "OPENFGA_STORE_ID=$storeId"
Write-Output "OPENFGA_AUTHORIZATION_MODEL_ID=$modelId"
Write-Output ""
Write-Output "Run model tests with:"
Write-Output "fga model test --tests infra/openfga/model.fga.yaml"
