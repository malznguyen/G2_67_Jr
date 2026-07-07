//! OpenAPI wire schemas for the GMRAG HTTP API (T84A).
//!
//! These types document the JSON/multipart contract. Handlers may use inline
//! `json!` or private row structs; this module is the single OpenAPI components
//! source (no duplicate schema names in the generated spec).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

// The role enums are ToSchema types referenced by the membership
// request/response schemas below. Re-exported so the OpenApi
// `components(schemas(...))` list in `mod.rs` can name them directly via its
// `use crate::openapi::schemas::*;` glob.
pub use crate::roles::{TenantMemberRole, WorkspaceMemberRole};

// ─── Error envelope ──────────────────────────────────────────────────────────

/// Standard error response envelope.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

/// Machine-readable error code (kebab-case) and human-readable message.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

// ─── Health ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub uptime_ms: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthzResponse {
    pub status: String,
    pub db: String,
    pub openfga: String,
}

// ─── Users ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MeResponse {
    pub user: UserProfile,
    pub tenants: Vec<UserTenantMembership>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UserProfile {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UserTenantMembership {
    pub id: Uuid,
    pub name: String,
    /// Tenant role: `owner`, `admin`, or `member`.
    pub role: TenantMemberRole,
}

// ─── Tenants ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TenantListItem {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Membership role: `owner`, `admin`, or `member`.
    pub role: TenantMemberRole,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TenantsResponse {
    pub tenants: Vec<TenantListItem>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateTenantRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateTenantResponse {
    pub id: Uuid,
    pub name: String,
    pub role: TenantMemberRole,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateTenantRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateTenantResponse {
    pub id: Uuid,
    pub name: String,
}

// ─── Tenant members ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TenantMemberItem {
    pub user_id: Uuid,
    pub role: TenantMemberRole,
    pub email: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TenantMembersResponse {
    pub members: Vec<TenantMemberItem>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct InviteMemberRequest {
    pub email: String,
    /// Optional tenant role (`owner`, `admin`, `member`); defaults to `member`.
    pub role: Option<TenantMemberRole>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct InviteMemberResponse {
    pub id: Uuid,
    pub email: String,
    pub role: TenantMemberRole,
    pub token: Uuid,
    /// Invitation status, e.g. `pending`.
    pub status: String,
}

// ─── Workspaces ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceItem {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WorkspacesResponse {
    pub workspaces: Vec<WorkspaceItem>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateWorkspaceResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateWorkspaceRequest {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateWorkspaceResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

// ─── Workspace members ───────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceMemberItem {
    pub user_id: Uuid,
    pub role: WorkspaceMemberRole,
    pub email: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceMembersResponse {
    pub members: Vec<WorkspaceMemberItem>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AddWorkspaceMemberRequest {
    pub user_id: Uuid,
    /// Optional workspace role (`owner`, `admin`, `member`); defaults to
    /// `member`.
    pub role: Option<WorkspaceMemberRole>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AddWorkspaceMemberResponse {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub role: WorkspaceMemberRole,
}

// ─── Documents ───────────────────────────────────────────────────────────────

/// Document visibility: `shared` (tenant-readable) or `private`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DocumentVisibility {
    Shared,
    Private,
}

/// Document ingest/index status.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[schema(example = "indexed")]
pub enum DocumentStatus {
    #[serde(rename = "uploaded")]
    Uploaded,
    #[serde(rename = "processing")]
    Processing,
    #[serde(rename = "indexed")]
    Indexed,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DocumentItem {
    pub id: Uuid,
    pub title: String,
    pub visibility: String,
    pub owner_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DocumentsResponse {
    pub documents: Vec<DocumentItem>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateDocumentResponse {
    pub id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DocumentPreviewMeta {
    pub id: Uuid,
    pub title: String,
    pub status: String,
    pub visibility: String,
    pub owner_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub mime_type: Option<String>,
    pub byte_size: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DocumentChunkPreview {
    pub chunk_index: i32,
    pub content: String,
    pub token_count: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DocumentPreviewResponse {
    pub document: DocumentPreviewMeta,
    pub chunks: Vec<DocumentChunkPreview>,
}

/// Multipart form fields for document upload.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UploadDocumentForm {
    #[schema(format = Binary, content_media_type = "application/octet-stream")]
    pub file: String,
    /// `shared` or `private`.
    pub visibility: String,
    pub workspace_id: Uuid,
    pub title: Option<String>,
}

// ─── ACL ─────────────────────────────────────────────────────────────────────

/// Shareable resource namespace: `document` or `chat_session`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum AclResourceType {
    #[serde(rename = "document")]
    Document,
    #[serde(rename = "chat_session")]
    ChatSession,
}

/// Grant principal type: `user` or `workspace`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum AclPrincipalType {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "workspace")]
    Workspace,
}

/// Grantable relation: `editor` or `viewer` (`owner` is implicit).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum AclRelation {
    #[serde(rename = "editor")]
    Editor,
    #[serde(rename = "viewer")]
    Viewer,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GrantItem {
    /// Opaque OpenFGA tuple identifier, suitable only for `DELETE /acl/{grant_id}`.
    pub id: String,
    pub principal_type: String,
    pub principal_id: Uuid,
    pub relation: String,
    /// OpenFGA direct tuples do not expose creation timestamps.
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GrantsResponse {
    pub grants: Vec<GrantItem>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateGrantRequest {
    pub resource_type: String,
    pub resource_id: Uuid,
    pub principal_type: String,
    pub principal_id: Uuid,
    pub relation: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateGrantResponse {
    /// Opaque OpenFGA tuple identifier, suitable only for `DELETE /acl/{grant_id}`.
    pub id: String,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub principal_type: String,
    pub principal_id: Uuid,
    pub relation: String,
}

// ─── Chat ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatSessionItem {
    pub id: Uuid,
    pub title: String,
    pub workspace_id: Option<Uuid>,
    pub model: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatSessionsResponse {
    pub sessions: Vec<ChatSessionItem>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateChatSessionRequest {
    pub title: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub model: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateChatSessionResponse {
    pub id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PostChatRequest {
    pub message: String,
}

/// T84D Phase 3.2 — single chat message (history endpoint).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatMessageItem {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub token_count: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatMessagesResponse {
    pub messages: Vec<ChatMessageItem>,
}

/// SSE `data:` line payload (tagged JSON).
///
/// In-stream errors (`type: "error"`) use flat `{ type, code, message }` — not
/// the HTTP `{ error: { code, message } }` envelope. Known codes: `stream-failed`,
/// `persist-failed`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatSseEvent {
    Text {
        content: String,
    },
    Citation {
        index: u32,
        point_id: Uuid,
        document_id: Uuid,
        chunk_index: i32,
        filename: Option<String>,
        /// T84D Phase 3.1: 1-based page range (nullable).
        page_start: Option<i32>,
        page_end: Option<i32>,
    },
    CitationUnknown {
        index: u32,
    },
    Done {
        finish_reason: Option<String>,
    },
    Error {
        code: String,
        message: String,
    },
}

// ─── Graph ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GraphNodeItem {
    pub id: Uuid,
    pub kind: String,
    pub label: String,
    pub properties: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GraphEdgeItem {
    pub id: Uuid,
    pub src_node_id: Uuid,
    pub dst_node_id: Uuid,
    pub kind: String,
    pub weight: f32,
    pub properties: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceGraphResponse {
    pub nodes: Vec<GraphNodeItem>,
    pub edges: Vec<GraphEdgeItem>,
    pub next_cursor: Option<String>,
}

// ─── Settings ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LlmSettingsResponse {
    pub configured: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub dimensions: Option<i32>,
    pub enabled: Option<bool>,
    pub llm_model: Option<String>,
    pub llm_base_url: Option<String>,
    pub has_api_key: bool,
    pub api_key_masked: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PutLlmSettingsRequest {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub dimensions: Option<i32>,
    pub enabled: Option<bool>,
    pub llm_model: Option<String>,
    pub llm_base_url: Option<String>,
}

// ─── Metering ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UsageMetricItem {
    pub metric: String,
    pub total: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UsageResponse {
    pub usage: Vec<UsageMetricItem>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct QuotaResponse {
    pub configured: bool,
    pub max_documents: i32,
    pub max_workspaces: i32,
    pub max_storage_bytes: i64,
    pub max_members: i32,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditLogItem {
    pub id: Uuid,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AuditLogsResponse {
    pub logs: Vec<AuditLogItem>,
}
