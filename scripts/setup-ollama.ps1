# =========================================================
# setup-ollama.ps1
# Pull the Ollama models required for end-to-end indexing + chat
# in local dev. Ollama ships *empty* by default — without these
# pulls the worker embedding call and (optional) chat/graph LLM
# calls 404 on `/api/embeddings` / `/api/chat`.
#
# Required for every tenant (always-on):
#   OLLAMA_EMBED_MODEL  (default nomic-embed-text)  — vector indexing
#
# Required ONLY if you are NOT using DeepSeek for chat+graph:
#   OLLAMA_LLM_MODEL    (default llama3.1:8b)       — chat + graph extraction
#
# If you set DEEPSEEK_API_KEY in .env, chat+graph go to DeepSeek and
# only the embed model is required locally. This script pulls all needed
# local models regardless and is safe to re-run (no-ops if already present).
#
# Usage:
#   pwsh ./scripts/setup-ollama.ps1
# Ollama must be running (docker compose ... up -d ollama, or a host install).
# =========================================================
param(
    [string]$OllamaHost = $env:OLLAMA_HOST,
    [string]$EmbedModel = $env:OLLAMA_EMBED_MODEL,
    [string]$LlmModel = $env:OLLAMA_LLM_MODEL
)

$ErrorActionPreference = "Stop"

# Default to the in-container API endpoint when unset, matching .env.example.
if (-not $OllamaHost) { $OllamaHost = "http://localhost:11434" }
if (-not $EmbedModel) { $EmbedModel = "nomic-embed-text" }
if (-not $LlmModel) { $LlmModel = "llama3.1:8b" }

function Invoke-OllamaPull {
    param([string]$Model)
    Write-Host "[setup-ollama] pulling '$Model' from $OllamaHost ..."
    & ollama --host $OllamaHost pull $Model
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to pull '$Model' from $OllamaHost (exit $LASTEXITCODE)"
    }
    Write-Host "[setup-ollama] OK — '$Model' is available locally."
}

Invoke-OllamaPull -Model $EmbedModel

$deepseekKey = $env:DEEPSEEK_API_KEY
if ([string]::IsNullOrWhiteSpace($deepseekKey)) {
    Write-Host "[setup-ollama] DEEPSEEK_API_KEY not set — chat + graph will use the local Ollama LLM, so the chat model is required."
    Invoke-OllamaPull -Model $LlmModel
} else {
    Write-Host "[setup-ollama] DEEPSEEK_API_KEY is set — chat + graph use DeepSeek; only the embed model is needed locally."
}

Write-Host ""
Write-Host "[setup-ollama] summary:"
Write-Host "  embed model: $EmbedModel (required, used by worker indexing)"
if ([string]::IsNullOrWhiteSpace($deepseekKey)) {
    Write-Host "  chat/graph LLM: $LlmModel (local Ollama fallback)"
} else {
    Write-Host "  chat/graph LLM: DeepSeek ($($env:DEEPSEEK_MODEL)) — local Ollama LLM NOT required"
}
Write-Host ""
Write-Host "Verify installed models:"
Write-Host "  curl $OllamaHost/api/tags"