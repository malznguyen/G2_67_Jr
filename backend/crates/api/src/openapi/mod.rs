//! OpenAPI specification generation and Swagger UI (T84A).

pub mod schemas;

use axum::Router;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

use crate::openapi::schemas::*;

/// Bearer JWT security scheme (Keycloak OIDC).
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .description(Some(
                            "Keycloak OIDC access token. Use Authorize and paste the JWT \
                             (with or without the `Bearer ` prefix).",
                        ))
                        .build(),
                ),
            );
        }
    }
}

/// Generated OpenAPI document for the GMRAG HTTP API.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "GMRAG API",
        version = "0.1.0",
        description = "GMRAG 2.0 HTTP API. Authenticated routes require `Authorization: Bearer <JWT>`. \
            Tenant-scoped routes under `/tenants/{tid}/...` also require header `X-Tenant-Id` \
            matching the path `{tid}`. List endpoints return full result sets (no pagination) \
            unless noted."
    ),
    paths(
        crate::health,
        crate::healthz,
        crate::routes::users::get_me,
        crate::routes::tenants::list_tenants,
        crate::routes::tenants::create_tenant,
        crate::routes::tenants::update_tenant,
        crate::routes::tenants::delete_tenant,
        crate::routes::tenant_members::list_members,
        crate::routes::tenant_members::invite_member,
        crate::routes::tenant_members::remove_member,
        crate::routes::workspaces::list_workspaces,
        crate::routes::workspaces::create_workspace,
        crate::routes::workspaces::update_workspace,
        crate::routes::workspaces::delete_workspace,
        crate::routes::ws_members::list_members,
        crate::routes::ws_members::add_member,
        crate::routes::ws_members::remove_member,
        crate::routes::documents::list_documents,
        crate::routes::documents::upload_document,
        crate::routes::documents::delete_document,
        crate::routes::documents::preview_document,
        crate::routes::acl::list_grants,
        crate::routes::acl::create_grant,
        crate::routes::acl::revoke_grant,
        crate::routes::chat::list_sessions,
        crate::routes::chat::create_session,
        crate::routes::chat::delete_session,
        crate::routes::chat::post_chat,
        crate::routes::chat::list_messages,
        crate::routes::graph::get_workspace_graph,
        crate::routes::settings::get_llm_settings,
        crate::routes::settings::put_llm_settings,
        crate::routes::metering::get_usage,
        crate::routes::metering::get_quotas,
        crate::routes::metering::get_audit_logs,
    ),
    components(schemas(
        ErrorResponse,
        ErrorBody,
        HealthResponse,
        HealthzResponse,
        MeResponse,
        UserProfile,
        UserTenantMembership,
        TenantListItem,
        TenantsResponse,
        CreateTenantRequest,
        CreateTenantResponse,
        UpdateTenantRequest,
        UpdateTenantResponse,
        TenantMemberItem,
        TenantMembersResponse,
        InviteMemberRequest,
        InviteMemberResponse,
        WorkspaceItem,
        WorkspacesResponse,
        CreateWorkspaceRequest,
        CreateWorkspaceResponse,
        UpdateWorkspaceRequest,
        UpdateWorkspaceResponse,
        WorkspaceMemberItem,
        WorkspaceMembersResponse,
        AddWorkspaceMemberRequest,
        AddWorkspaceMemberResponse,
        DocumentVisibility,
        DocumentStatus,
        DocumentItem,
        DocumentsResponse,
        CreateDocumentResponse,
        DocumentPreviewMeta,
        DocumentChunkPreview,
        DocumentPreviewResponse,
        UploadDocumentForm,
        AclResourceType,
        AclPrincipalType,
        AclRelation,
        GrantItem,
        GrantsResponse,
        CreateGrantRequest,
        CreateGrantResponse,
        ChatSessionItem,
        ChatSessionsResponse,
        CreateChatSessionRequest,
        CreateChatSessionResponse,
        PostChatRequest,
        ChatSseEvent,
        ChatMessageItem,
        ChatMessagesResponse,
        GraphNodeItem,
        GraphEdgeItem,
        WorkspaceGraphResponse,
        LlmSettingsResponse,
        PutLlmSettingsRequest,
        UsageMetricItem,
        UsageResponse,
        QuotaResponse,
        AuditLogItem,
        AuditLogsResponse,
    )),
    tags(
        (name = "Health", description = "Liveness and readiness probes"),
        (name = "Users", description = "Authenticated user profile"),
        (name = "Tenants", description = "Tenant CRUD and membership context"),
        (name = "TenantMembers", description = "Tenant member invitations and removal"),
        (name = "Workspaces", description = "Workspace CRUD within a tenant"),
        (name = "WorkspaceMembers", description = "Workspace membership"),
        (name = "Documents", description = "Document upload, list, preview, delete"),
        (name = "ACL", description = "ReBAC resource sharing grants"),
        (name = "Chat", description = "Chat sessions and RAG SSE streaming"),
        (name = "Graph", description = "Workspace knowledge graph"),
        (name = "Settings", description = "Tenant LLM / BYOK configuration"),
        (name = "Metering", description = "Usage, quotas, and audit logs (owner-only)"),
    ),
    modifiers(&SecurityAddon),
)]
pub struct ApiDoc;

/// Swagger UI at `/swagger` and OpenAPI JSON at `/openapi.json`.
pub fn swagger_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    SwaggerUi::new("/swagger")
        .url("/openapi.json", ApiDoc::openapi())
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_doc_generates_without_panic() {
        let doc = ApiDoc::openapi();
        assert_eq!(doc.info.title, "GMRAG API");
        let op_count: usize = doc
            .paths
            .paths
            .values()
            .map(|item| {
                [
                    item.get.is_some(),
                    item.post.is_some(),
                    item.put.is_some(),
                    item.patch.is_some(),
                    item.delete.is_some(),
                ]
                .into_iter()
                .filter(|present| *present)
                .count()
            })
            .sum();
        assert!(
            op_count >= 34,
            "expected at least 34 operations, got {op_count}"
        );
    }

    #[test]
    fn bearer_security_scheme_present() {
        let doc = ApiDoc::openapi();
        let schemes = doc
            .components
            .as_ref()
            .map(|c| &c.security_schemes)
            .expect("components.securitySchemes");
        assert!(schemes.contains_key("bearer_auth"));
    }
}
