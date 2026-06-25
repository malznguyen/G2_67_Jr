/**
 * Minimal OpenAPI path typings for the GMRAG backend.
 *
 * Regenerate from a running backend with:
 *   pnpm gen:api
 *
 * This file is a hand-maintained fallback so the frontend builds without the
 * backend online. Keep it in sync with `backend/crates/api/src/openapi/schemas.rs`
 * and `docs/FRONTEND_API_CONTRACT.md`.
 */

export interface paths {
  "/users/me": {
    get: operations["getMe"];
  };
  "/tenants": {
    get: operations["listTenants"];
    post: operations["createTenant"];
  };
  "/tenants/{tid}": {
    patch: operations["updateTenant"];
    delete: operations["deleteTenant"];
  };
  "/tenants/{tid}/workspaces": {
    get: operations["listWorkspaces"];
    post: operations["createWorkspace"];
  };
  "/tenants/{tid}/documents": {
    get: operations["listDocuments"];
    post: operations["uploadDocument"];
  };
  "/tenants/{tid}/documents/{doc_id}": {
    delete: operations["deleteDocument"];
  };
  "/tenants/{tid}/documents/{doc_id}/preview": {
    get: operations["previewDocument"];
  };
  "/tenants/{tid}/chat_sessions": {
    get: operations["listChatSessions"];
    post: operations["createChatSession"];
  };
  "/tenants/{tid}/chat_sessions/{sid}/messages": {
    get: operations["listChatMessages"];
  };
}

export interface components {
  schemas: never;
}

export interface operations {
  getMe: {
    responses: {
      200: {
        content: {
          "application/json": components_schemas.MeResponse;
        };
      };
      401: {
        content: {
          "application/json": components_schemas.ErrorResponse;
        };
      };
    };
  };
  listTenants: {
    responses: {
      200: {
        content: {
          "application/json": components_schemas.TenantsResponse;
        };
      };
    };
  };
  createTenant: {
    requestBody: {
      content: {
        "application/json": components_schemas.CreateTenantRequest;
      };
    };
    responses: {
      201: {
        content: {
          "application/json": components_schemas.CreateTenantResponse;
        };
      };
    };
  };
  updateTenant: {
    parameters: {
      path: { tid: string };
    };
    requestBody: {
      content: {
        "application/json": components_schemas.UpdateTenantRequest;
      };
    };
    responses: {
      200: {
        content: {
          "application/json": components_schemas.UpdateTenantResponse;
        };
      };
    };
  };
  deleteTenant: {
    parameters: {
      path: { tid: string };
    };
    responses: {
      204: { content: never };
    };
  };
  listWorkspaces: {
    parameters: {
      path: { tid: string };
    };
    responses: {
      200: {
        content: {
          "application/json": components_schemas.WorkspacesResponse;
        };
      };
    };
  };
  createWorkspace: {
    parameters: {
      path: { tid: string };
    };
    requestBody: {
      content: {
        "application/json": components_schemas.CreateWorkspaceRequest;
      };
    };
    responses: {
      201: {
        content: {
          "application/json": components_schemas.CreateWorkspaceResponse;
        };
      };
    };
  };
  listDocuments: {
    parameters: {
      path: { tid: string };
      query: { workspace_id: string };
    };
    responses: {
      200: {
        content: {
          "application/json": components_schemas.DocumentsResponse;
        };
      };
    };
  };
  deleteDocument: {
    parameters: {
      path: { tid: string; doc_id: string };
    };
    responses: {
      204: { content: never };
    };
  };
  previewDocument: {
    parameters: {
      path: { tid: string; doc_id: string };
    };
    responses: {
      200: {
        content: {
          "application/json": components_schemas.DocumentPreviewResponse;
        };
      };
    };
  };
  uploadDocument: {
    parameters: {
      path: { tid: string };
    };
    requestBody: {
      content: {
        "multipart/form-data": components_schemas.UploadDocumentForm;
      };
    };
    responses: {
      201: {
        content: {
          "application/json": components_schemas.CreateDocumentResponse;
        };
      };
    };
  };
  listChatSessions: {
    parameters: {
      path: { tid: string };
    };
    responses: {
      200: {
        content: {
          "application/json": components_schemas.ChatSessionsResponse;
        };
      };
    };
  };
  createChatSession: {
    parameters: {
      path: { tid: string };
    };
    requestBody: {
      content: {
        "application/json": components_schemas.CreateChatSessionRequest;
      };
    };
    responses: {
      201: {
        content: {
          "application/json": components_schemas.CreateChatSessionResponse;
        };
      };
    };
  };
  listChatMessages: {
    parameters: {
      path: { tid: string; sid: string };
    };
    responses: {
      200: {
        content: {
          "application/json": components_schemas.ChatMessagesResponse;
        };
      };
    };
  };
}

export interface components_schemas {
  ErrorResponse: { error: { code: string; message: string } };
  MeResponse: {
    user: { id: string; email: string; name: string; created_at: string };
    tenants: UserTenantMembership[];
  };
  UserTenantMembership: { id: string; name: string; role: "owner" | "member" };
  TenantListItem: { id: string; name: string; created_at: string; role: string };
  TenantsResponse: { tenants: components_schemas["TenantListItem"][] };
  CreateTenantRequest: { name: string };
  CreateTenantResponse: { id: string };
  UpdateTenantRequest: { name?: string };
  UpdateTenantResponse: { id: string; name: string };
  WorkspacesResponse: {
    workspaces: { id: string; name: string; description?: string; created_at: string }[];
  };
  CreateWorkspaceRequest: { name: string; description?: string };
  CreateWorkspaceResponse: { id: string };
  DocumentsResponse: {
    documents: {
      id: string;
      title: string;
      filename: string;
      visibility: "shared" | "private";
      workspace_id: string | null;
      owner_id: string;
      status: DocumentStatus;
      created_at: string;
    }[];
  };
  DocumentStatus: "uploaded" | "processing" | "indexed" | "failed";
  DocumentPreviewResponse: {
    document_id: string;
    title: string;
    status: DocumentStatus;
    visibility: "shared" | "private";
    mime_type: string | null;
    byte_size: number;
    chunks: {
      index: number;
      text: string;
    }[];
  };
  UploadDocumentForm: {
    file: Blob;
    visibility: "shared" | "private";
    workspace_id: string;
    title?: string;
  };
  CreateDocumentResponse: { id: string };
  ChatSessionsResponse: {
    sessions: {
      id: string;
      title: string;
      workspace_id: string | null;
      model: string | null;
      created_at: string;
      updated_at: string;
    }[];
  };
  CreateChatSessionRequest: {
    title?: string;
    workspace_id?: string | null;
    model?: string;
  };
  CreateChatSessionResponse: { id: string };
  ChatMessagesResponse: {
    messages: {
      id: string;
      role: "user" | "assistant";
      content: string;
      token_count: number | null;
      created_at: string;
    }[];
  };
}
