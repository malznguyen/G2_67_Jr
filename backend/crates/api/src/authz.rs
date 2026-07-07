//! OpenFGA-backed authorization boundary.
//!
//! Production authorization decisions go through [`AuthorizationService`].
//! PostgreSQL still supplies resource metadata and tenant RLS, but it is not
//! used as a fallback authorization engine.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use gmrag_core::config::OpenFgaConfig;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, warn};
use uuid::Uuid;

pub type AuthzService = Arc<dyn AuthorizationService>;

pub const TYPE_USER: &str = "user";
pub const TYPE_TENANT: &str = "tenant";
pub const TYPE_WORKSPACE: &str = "workspace";
pub const TYPE_DOCUMENT: &str = "document";
pub const TYPE_CHAT_SESSION: &str = "chat_session";

pub const REL_TENANT: &str = "tenant";
pub const REL_WORKSPACE: &str = "workspace";
pub const REL_OWNER: &str = "owner";
pub const REL_ADMIN: &str = "admin";
pub const REL_MEMBER: &str = "member";
pub const REL_ACCESSOR: &str = "accessor";
pub const REL_MANAGER: &str = "manager";
pub const REL_EDITOR: &str = "editor";
pub const REL_VIEWER: &str = "viewer";

const OPENFGA_WRITE_BATCH_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consistency {
    MinimizeLatency,
    HigherConsistency,
}

impl Consistency {
    fn as_openfga(self) -> &'static str {
        match self {
            Consistency::MinimizeLatency => "MINIMIZE_LATENCY",
            Consistency::HigherConsistency => "HIGHER_CONSISTENCY",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRequest {
    pub user: String,
    pub relation: String,
    pub object: String,
    pub consistency: Consistency,
}

impl CheckRequest {
    pub fn new(user: String, relation: impl Into<String>, object: String) -> Self {
        Self {
            user,
            relation: relation.into(),
            object,
            consistency: Consistency::MinimizeLatency,
        }
    }

    pub fn higher_consistency(mut self) -> Self {
        self.consistency = Consistency::HigherConsistency;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub request: CheckRequest,
    pub allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelationshipTuple {
    pub user: String,
    pub relation: String,
    pub object: String,
}

impl RelationshipTuple {
    pub fn new(user: String, relation: impl Into<String>, object: String) -> Self {
        Self {
            user,
            relation: relation.into(),
            object,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantPrincipal {
    User(Uuid),
    Workspace(Uuid),
}

impl GrantPrincipal {
    pub fn principal_type(&self) -> &'static str {
        match self {
            GrantPrincipal::User(_) => TYPE_USER,
            GrantPrincipal::Workspace(_) => TYPE_WORKSPACE,
        }
    }

    pub fn id(&self) -> Uuid {
        match self {
            GrantPrincipal::User(id) | GrantPrincipal::Workspace(id) => *id,
        }
    }

    pub fn to_openfga_user(&self) -> String {
        match self {
            GrantPrincipal::User(id) => user_obj(*id),
            GrantPrincipal::Workspace(id) => workspace_member_userset(*id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueGrantId {
    pub user: String,
    pub relation: String,
    pub object: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GrantIdPayload {
    v: u8,
    user: String,
    relation: String,
    object: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthzError {
    #[error("openfga configuration error: {0}")]
    Config(String),

    #[error("authorization unavailable: {0}")]
    Unavailable(String),

    #[error("authorization request failed: {0}")]
    Request(String),

    #[error("authorization response malformed: {0}")]
    Malformed(String),
}

#[async_trait]
pub trait AuthorizationService: Send + Sync {
    async fn check(&self, request: CheckRequest) -> Result<bool, AuthzError>;

    async fn batch_check(
        &self,
        requests: Vec<CheckRequest>,
    ) -> Result<Vec<CheckResult>, AuthzError>;

    async fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
        consistency: Consistency,
    ) -> Result<Vec<String>, AuthzError>;

    async fn read_direct_relationships(
        &self,
        object: &str,
    ) -> Result<Vec<RelationshipTuple>, AuthzError>;

    /// Enumerate every direct relationship tuple currently stored in the
    /// authorization backend (OpenFGA), with no object filter. Used by the
    /// Phase 3 drift reconciler to detect orphaned tuples whose referenced
    /// Postgres entity no longer exists. Implementations must paginate if the
    /// backend caps page size.
    async fn read_all_direct_relationships(&self) -> Result<Vec<RelationshipTuple>, AuthzError>;

    async fn write_relationships(
        &self,
        writes: Vec<RelationshipTuple>,
        deletes: Vec<RelationshipTuple>,
    ) -> Result<(), AuthzError>;

    async fn delete_all_direct_relationships_for_object(
        &self,
        object: &str,
    ) -> Result<(), AuthzError>;

    async fn health(&self) -> Result<(), AuthzError>;
}

#[derive(Clone)]
pub struct OpenFgaAuthorizationService {
    client: reqwest::Client,
    api_url: String,
    store_id: String,
    authorization_model_id: String,
    api_token: Option<String>,
}

impl OpenFgaAuthorizationService {
    pub fn new(cfg: &OpenFgaConfig) -> Result<Self, AuthzError> {
        let timeout = Duration::from_millis(cfg.request_timeout_ms);
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| AuthzError::Config(format!("reqwest client: {e}")))?;
        Ok(Self {
            client,
            api_url: cfg.api_url.trim_end_matches('/').to_string(),
            store_id: cfg.store_id.clone(),
            authorization_model_id: cfg.authorization_model_id.clone(),
            api_token: cfg.api_token.clone(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_url, path)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let request = self.client.request(method, self.url(path));
        match &self.api_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    async fn post_json<T, R>(&self, path: &str, body: &T) -> Result<R, AuthzError>
    where
        T: Serialize + ?Sized,
        R: for<'de> Deserialize<'de>,
    {
        let started = Instant::now();
        let response = self
            .request(reqwest::Method::POST, path)
            .json(body)
            .send()
            .await
            .map_err(|e| AuthzError::Unavailable(e.to_string()))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| AuthzError::Unavailable(e.to_string()))?;
        debug!(
            status = %status,
            elapsed_ms = started.elapsed().as_millis(),
            path,
            "openfga request completed"
        );
        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes);
            return Err(map_openfga_status(status, msg.as_ref()));
        }
        serde_json::from_slice(&bytes).map_err(|e| AuthzError::Malformed(e.to_string()))
    }
}

#[async_trait]
impl AuthorizationService for OpenFgaAuthorizationService {
    async fn check(&self, request: CheckRequest) -> Result<bool, AuthzError> {
        #[derive(Serialize)]
        struct Body<'a> {
            authorization_model_id: &'a str,
            tuple_key: TupleKeyRef<'a>,
            #[serde(skip_serializing_if = "Option::is_none")]
            consistency: Option<&'static str>,
        }
        #[derive(Serialize)]
        struct TupleKeyRef<'a> {
            user: &'a str,
            relation: &'a str,
            object: &'a str,
        }
        // OpenFGA's check response is intentionally parsed with a permissive
        // shape: the server may add new top-level fields over time (v1.18.1
        // already returns `{"allowed": true, "resolution": ""}`), and we must
        // not fail closed by rejecting unknown fields. Only `allowed` is
        // meaningful to us; ignore everything else.
        #[derive(Deserialize)]
        struct Response {
            #[allow(dead_code)]
            #[serde(default)]
            resolution: Option<String>,
            allowed: bool,
        }

        let path = format!("/stores/{}/check", self.store_id);
        let body = Body {
            authorization_model_id: &self.authorization_model_id,
            tuple_key: TupleKeyRef {
                user: &request.user,
                relation: &request.relation,
                object: &request.object,
            },
            consistency: (request.consistency == Consistency::HigherConsistency)
                .then(|| request.consistency.as_openfga()),
        };
        let response: Response = self.post_json(&path, &body).await?;
        Ok(response.allowed)
    }

    async fn batch_check(
        &self,
        requests: Vec<CheckRequest>,
    ) -> Result<Vec<CheckResult>, AuthzError> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let allowed = self.check(request.clone()).await?;
            results.push(CheckResult { request, allowed });
        }
        Ok(results)
    }

    async fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
        consistency: Consistency,
    ) -> Result<Vec<String>, AuthzError> {
        #[derive(Serialize)]
        struct Body<'a> {
            authorization_model_id: &'a str,
            user: &'a str,
            relation: &'a str,
            #[serde(rename = "type")]
            object_type: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            consistency: Option<&'static str>,
        }
        // Same forward-compatible treatment as the check() response: OpenFGA may
        // add new top-level fields in future versions; ignore anything we don't
        // need rather than failing closed on a malformed-response error.
        #[derive(Deserialize)]
        struct Response {
            objects: Vec<String>,
        }

        let path = format!("/stores/{}/list-objects", self.store_id);
        let body = Body {
            authorization_model_id: &self.authorization_model_id,
            user,
            relation,
            object_type,
            consistency: (consistency == Consistency::HigherConsistency)
                .then(|| consistency.as_openfga()),
        };
        let response: Response = self.post_json(&path, &body).await?;
        Ok(response.objects)
    }

    async fn read_direct_relationships(
        &self,
        object: &str,
    ) -> Result<Vec<RelationshipTuple>, AuthzError> {
        #[derive(Serialize)]
        struct Body<'a> {
            tuple_key: ReadTupleKey<'a>,
        }
        #[derive(Serialize)]
        struct ReadTupleKey<'a> {
            object: &'a str,
        }
        #[derive(Deserialize)]
        struct Response {
            tuples: Vec<TupleEnvelope>,
        }
        #[derive(Deserialize)]
        struct TupleEnvelope {
            key: RelationshipTuple,
        }

        let path = format!("/stores/{}/read", self.store_id);
        let body = Body {
            tuple_key: ReadTupleKey { object },
        };
        let response: Response = self.post_json(&path, &body).await?;
        Ok(response.tuples.into_iter().map(|t| t.key).collect())
    }

    async fn read_all_direct_relationships(&self) -> Result<Vec<RelationshipTuple>, AuthzError> {
        // OpenFGA `/read` with no `tuple_key` filter returns every tuple,
        // paginated by `continuation_token`. Loop until the token is absent
        // (or empty) so the reconciler sees the complete live tuple set.
        #[derive(Serialize)]
        struct Body {
            #[serde(skip_serializing_if = "Option::is_none")]
            continuation_token: Option<String>,
        }
        #[derive(Deserialize)]
        struct Response {
            tuples: Vec<TupleEnvelope>,
            #[serde(default)]
            continuation_token: Option<String>,
        }
        #[derive(Deserialize)]
        struct TupleEnvelope {
            key: RelationshipTuple,
        }

        let path = format!("/stores/{}/read", self.store_id);
        let mut all = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let body = Body {
                continuation_token: continuation.clone(),
            };
            let response: Response = self.post_json(&path, &body).await?;
            all.extend(response.tuples.into_iter().map(|t| t.key));
            match response.continuation_token {
                Some(token) if !token.is_empty() => continuation = Some(token),
                _ => break,
            }
        }
        Ok(all)
    }

    async fn write_relationships(
        &self,
        writes: Vec<RelationshipTuple>,
        deletes: Vec<RelationshipTuple>,
    ) -> Result<(), AuthzError> {
        #[derive(Serialize)]
        struct Body<'a> {
            authorization_model_id: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            writes: Option<TupleSet>,
            #[serde(skip_serializing_if = "Option::is_none")]
            deletes: Option<TupleSet>,
        }
        #[derive(Serialize)]
        struct TupleSet {
            tuple_keys: Vec<RelationshipTuple>,
            #[serde(skip_serializing_if = "Option::is_none")]
            on_duplicate: Option<&'static str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            on_missing: Option<&'static str>,
        }

        let writes = dedupe_tuples(writes);
        let deletes = dedupe_tuples(deletes);
        let max_len = writes.len().max(deletes.len()).max(1);
        let path = format!("/stores/{}/write", self.store_id);

        for start in (0..max_len).step_by(OPENFGA_WRITE_BATCH_LIMIT) {
            let end = (start + OPENFGA_WRITE_BATCH_LIMIT).min(max_len);
            let write_chunk = writes.get(start..writes.len().min(end)).unwrap_or(&[]);
            let delete_chunk = deletes.get(start..deletes.len().min(end)).unwrap_or(&[]);
            if write_chunk.is_empty() && delete_chunk.is_empty() {
                continue;
            }

            let body = Body {
                authorization_model_id: &self.authorization_model_id,
                writes: (!write_chunk.is_empty()).then(|| TupleSet {
                    tuple_keys: write_chunk.to_vec(),
                    on_duplicate: Some("ignore"),
                    on_missing: None,
                }),
                deletes: (!delete_chunk.is_empty()).then(|| TupleSet {
                    tuple_keys: delete_chunk.to_vec(),
                    on_duplicate: None,
                    on_missing: Some("ignore"),
                }),
            };
            let _: serde_json::Value = self.post_json(&path, &body).await?;
        }
        Ok(())
    }

    async fn delete_all_direct_relationships_for_object(
        &self,
        object: &str,
    ) -> Result<(), AuthzError> {
        let tuples = self.read_direct_relationships(object).await?;
        self.write_relationships(Vec::new(), tuples).await
    }

    async fn health(&self) -> Result<(), AuthzError> {
        let response = self
            .request(reqwest::Method::GET, "/healthz")
            .send()
            .await
            .map_err(|e| AuthzError::Unavailable(e.to_string()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(AuthzError::Unavailable(format!(
                "healthz returned {}",
                response.status()
            )))
        }
    }
}

fn map_openfga_status(status: StatusCode, body: &str) -> AuthzError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::BAD_REQUEST => {
            AuthzError::Request(body.to_string())
        }
        _ => AuthzError::Unavailable(format!("{status}: {body}")),
    }
}

fn dedupe_tuples(tuples: Vec<RelationshipTuple>) -> Vec<RelationshipTuple> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tuple in tuples {
        if seen.insert(tuple.clone()) {
            out.push(tuple);
        }
    }
    out
}

pub fn object(type_name: &str, id: Uuid) -> String {
    format!("{type_name}:{id}")
}

pub fn user_obj(id: Uuid) -> String {
    object(TYPE_USER, id)
}

pub fn tenant_obj(id: Uuid) -> String {
    object(TYPE_TENANT, id)
}

pub fn workspace_obj(id: Uuid) -> String {
    object(TYPE_WORKSPACE, id)
}

pub fn document_obj(id: Uuid) -> String {
    object(TYPE_DOCUMENT, id)
}

pub fn chat_session_obj(id: Uuid) -> String {
    object(TYPE_CHAT_SESSION, id)
}

pub fn tenant_member_userset(id: Uuid) -> String {
    format!("{}#{}", tenant_obj(id), REL_MEMBER)
}

pub fn workspace_member_userset(id: Uuid) -> String {
    format!("{}#{}", workspace_obj(id), REL_MEMBER)
}

pub fn typed_uuid(object: &str, expected_type: &str) -> Result<Uuid, AuthzError> {
    let Some((type_name, raw_id)) = object.split_once(':') else {
        return Err(AuthzError::Malformed(format!(
            "object '{object}' is missing type prefix"
        )));
    };
    if type_name != expected_type {
        return Err(AuthzError::Malformed(format!(
            "object '{object}' has type '{type_name}', expected '{expected_type}'"
        )));
    }
    if raw_id.contains('#') {
        return Err(AuthzError::Malformed(format!(
            "object '{object}' must not contain a userset relation"
        )));
    }
    Uuid::parse_str(raw_id)
        .map_err(|e| AuthzError::Malformed(format!("object '{object}' has invalid UUID: {e}")))
}

pub fn parse_user_object(user: &str) -> Result<Uuid, AuthzError> {
    typed_uuid(user, TYPE_USER)
}

pub fn parse_grant_principal(user: &str) -> Result<GrantPrincipal, AuthzError> {
    if let Some(raw) = user.strip_prefix("workspace:") {
        let Some((id, relation)) = raw.split_once('#') else {
            return Err(AuthzError::Malformed(
                "workspace grant principal must include #member".into(),
            ));
        };
        if relation != REL_MEMBER {
            return Err(AuthzError::Malformed(format!(
                "workspace grant relation must be member, got {relation}"
            )));
        }
        let id = Uuid::parse_str(id)
            .map_err(|e| AuthzError::Malformed(format!("invalid workspace principal: {e}")))?;
        return Ok(GrantPrincipal::Workspace(id));
    }
    parse_user_object(user).map(GrantPrincipal::User)
}

pub fn encode_grant_id(tuple: &RelationshipTuple) -> String {
    let payload = GrantIdPayload {
        v: 1,
        user: tuple.user.clone(),
        relation: tuple.relation.clone(),
        object: tuple.object.clone(),
    };
    let json = serde_json::to_vec(&payload).expect("grant id payload serializes");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

pub fn decode_grant_id(encoded: &str) -> Result<OpaqueGrantId, AuthzError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| AuthzError::Malformed(format!("grant id is not base64url: {e}")))?;
    let payload: GrantIdPayload = serde_json::from_slice(&bytes)
        .map_err(|e| AuthzError::Malformed(format!("grant id is not valid json: {e}")))?;
    if payload.v != 1 {
        return Err(AuthzError::Malformed(format!(
            "unsupported grant id version {}",
            payload.v
        )));
    }
    Ok(OpaqueGrantId {
        user: payload.user,
        relation: payload.relation,
        object: payload.object,
    })
}

pub fn tuple(user: String, relation: impl Into<String>, object: String) -> RelationshipTuple {
    RelationshipTuple::new(user, relation, object)
}

pub fn tenant_role_tuple(user_id: Uuid, role: &str, tenant_id: Uuid) -> RelationshipTuple {
    tuple(user_obj(user_id), role, tenant_obj(tenant_id))
}

pub fn workspace_role_tuple(user_id: Uuid, role: &str, workspace_id: Uuid) -> RelationshipTuple {
    tuple(user_obj(user_id), role, workspace_obj(workspace_id))
}

pub fn workspace_tenant_tuple(tenant_id: Uuid, workspace_id: Uuid) -> RelationshipTuple {
    tuple(
        tenant_obj(tenant_id),
        REL_TENANT,
        workspace_obj(workspace_id),
    )
}

pub fn document_tenant_tuple(tenant_id: Uuid, document_id: Uuid) -> RelationshipTuple {
    tuple(tenant_obj(tenant_id), REL_TENANT, document_obj(document_id))
}

pub fn document_workspace_tuple(workspace_id: Uuid, document_id: Uuid) -> RelationshipTuple {
    tuple(
        workspace_obj(workspace_id),
        REL_WORKSPACE,
        document_obj(document_id),
    )
}

pub fn document_owner_tuple(user_id: Uuid, document_id: Uuid) -> RelationshipTuple {
    tuple(user_obj(user_id), REL_OWNER, document_obj(document_id))
}

pub fn document_shared_tuple(tenant_id: Uuid, document_id: Uuid) -> RelationshipTuple {
    tuple(
        tenant_member_userset(tenant_id),
        REL_VIEWER,
        document_obj(document_id),
    )
}

pub fn chat_tenant_tuple(tenant_id: Uuid, session_id: Uuid) -> RelationshipTuple {
    tuple(
        tenant_obj(tenant_id),
        REL_TENANT,
        chat_session_obj(session_id),
    )
}

pub fn chat_workspace_tuple(workspace_id: Uuid, session_id: Uuid) -> RelationshipTuple {
    tuple(
        workspace_obj(workspace_id),
        REL_WORKSPACE,
        chat_session_obj(session_id),
    )
}

pub fn chat_owner_tuple(user_id: Uuid, session_id: Uuid) -> RelationshipTuple {
    tuple(user_obj(user_id), REL_OWNER, chat_session_obj(session_id))
}

#[derive(Clone)]
pub struct PgTestAuthorizationService {
    pool: sqlx::PgPool,
    direct: Arc<Mutex<HashSet<RelationshipTuple>>>,
}

impl PgTestAuthorizationService {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            direct: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    async fn has_direct(&self, user: &str, relation: &str, object: &str) -> bool {
        let direct = self.direct.lock().await;
        direct.contains(&RelationshipTuple::new(
            user.to_string(),
            relation.to_string(),
            object.to_string(),
        ))
    }

    async fn user_in_tenant_role(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        roles: &[&str],
    ) -> Result<bool, AuthzError> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM tenant_members
                WHERE tenant_id = $1 AND user_id = $2 AND role = ANY($3)
             )",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(roles)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AuthzError::Unavailable(e.to_string()))?;
        Ok(exists)
    }

    async fn workspace_tenant(&self, workspace_id: Uuid) -> Result<Option<Uuid>, AuthzError> {
        sqlx::query_scalar("SELECT tenant_id FROM workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AuthzError::Unavailable(e.to_string()))
    }

    async fn document_row(
        &self,
        document_id: Uuid,
    ) -> Result<Option<(Uuid, Option<Uuid>, Uuid, String)>, AuthzError> {
        sqlx::query_as(
            "SELECT tenant_id, workspace_id, owner_id, visibility FROM documents WHERE id = $1",
        )
        .bind(document_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AuthzError::Unavailable(e.to_string()))
    }

    async fn chat_row(
        &self,
        session_id: Uuid,
    ) -> Result<Option<(Uuid, Option<Uuid>, Uuid)>, AuthzError> {
        sqlx::query_as("SELECT tenant_id, workspace_id, user_id FROM chat_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AuthzError::Unavailable(e.to_string()))
    }

    async fn workspace_role(
        &self,
        workspace_id: Uuid,
        user_id: Uuid,
        roles: &[&str],
    ) -> Result<bool, AuthzError> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM workspace_members
                WHERE workspace_id = $1 AND user_id = $2 AND role = ANY($3)
             )",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(roles)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AuthzError::Unavailable(e.to_string()))?;
        Ok(exists)
    }

    async fn workspace_accessor(
        &self,
        workspace_id: Uuid,
        user_id: Uuid,
    ) -> Result<bool, AuthzError> {
        if self
            .workspace_role(workspace_id, user_id, &[REL_OWNER, REL_ADMIN, REL_MEMBER])
            .await?
        {
            return Ok(true);
        }
        let Some(tenant_id) = self.workspace_tenant(workspace_id).await? else {
            return Ok(false);
        };
        self.user_in_tenant_role(tenant_id, user_id, &[REL_OWNER])
            .await
    }

    async fn workspace_manager(
        &self,
        workspace_id: Uuid,
        user_id: Uuid,
    ) -> Result<bool, AuthzError> {
        if self
            .workspace_role(workspace_id, user_id, &[REL_OWNER, REL_ADMIN])
            .await?
        {
            return Ok(true);
        }
        let Some(tenant_id) = self.workspace_tenant(workspace_id).await? else {
            return Ok(false);
        };
        self.user_in_tenant_role(tenant_id, user_id, &[REL_OWNER])
            .await
    }

    async fn direct_grant_matches_user(
        &self,
        object: &str,
        relation: &str,
        user_id: Uuid,
    ) -> Result<bool, AuthzError> {
        let direct = self.direct.lock().await;
        for tuple in direct.iter() {
            if tuple.object != object || tuple.relation != relation {
                continue;
            }
            if tuple.user == user_obj(user_id) {
                return Ok(true);
            }
            if let Ok(GrantPrincipal::Workspace(workspace_id)) = parse_grant_principal(&tuple.user)
            {
                drop(direct);
                return self
                    .workspace_role(workspace_id, user_id, &[REL_OWNER, REL_ADMIN, REL_MEMBER])
                    .await;
            }
        }
        Ok(false)
    }

    async fn check_user(
        &self,
        user_id: Uuid,
        relation: &str,
        object: &str,
    ) -> Result<bool, AuthzError> {
        if self.has_direct(&user_obj(user_id), relation, object).await {
            return Ok(true);
        }
        if let Ok(tenant_id) = typed_uuid(object, TYPE_TENANT) {
            return match relation {
                REL_OWNER => {
                    self.user_in_tenant_role(tenant_id, user_id, &[REL_OWNER])
                        .await
                }
                REL_ADMIN => {
                    self.user_in_tenant_role(tenant_id, user_id, &[REL_ADMIN])
                        .await
                }
                REL_MEMBER => {
                    self.user_in_tenant_role(
                        tenant_id,
                        user_id,
                        &[REL_OWNER, REL_ADMIN, REL_MEMBER],
                    )
                    .await
                }
                _ => Ok(false),
            };
        }
        if let Ok(workspace_id) = typed_uuid(object, TYPE_WORKSPACE) {
            return match relation {
                REL_OWNER => {
                    self.workspace_role(workspace_id, user_id, &[REL_OWNER])
                        .await
                }
                REL_ADMIN => {
                    self.workspace_role(workspace_id, user_id, &[REL_ADMIN])
                        .await
                }
                REL_MEMBER => {
                    self.workspace_role(workspace_id, user_id, &[REL_OWNER, REL_ADMIN, REL_MEMBER])
                        .await
                }
                REL_ACCESSOR => self.workspace_accessor(workspace_id, user_id).await,
                REL_MANAGER => self.workspace_manager(workspace_id, user_id).await,
                _ => Ok(false),
            };
        }
        if let Ok(document_id) = typed_uuid(object, TYPE_DOCUMENT) {
            let Some((tenant_id, workspace_id, owner_id, visibility)) =
                self.document_row(document_id).await?
            else {
                return Ok(false);
            };
            return match relation {
                REL_OWNER => Ok(owner_id == user_id),
                REL_EDITOR => Ok(owner_id == user_id
                    || self
                        .direct_grant_matches_user(object, REL_EDITOR, user_id)
                        .await?),
                REL_VIEWER => {
                    if owner_id == user_id
                        || visibility == "shared"
                        || self
                            .direct_grant_matches_user(object, REL_VIEWER, user_id)
                            .await?
                        || self
                            .direct_grant_matches_user(object, REL_EDITOR, user_id)
                            .await?
                        || self
                            .user_in_tenant_role(
                                tenant_id,
                                user_id,
                                &[REL_MEMBER, REL_ADMIN, REL_OWNER],
                            )
                            .await?
                            && self
                                .has_direct(&tenant_member_userset(tenant_id), REL_VIEWER, object)
                                .await
                    {
                        return Ok(true);
                    }
                    if let Some(workspace_id) = workspace_id {
                        return self.workspace_accessor(workspace_id, user_id).await;
                    }
                    Ok(false)
                }
                _ => Ok(false),
            };
        }
        if let Ok(session_id) = typed_uuid(object, TYPE_CHAT_SESSION) {
            let Some((_tenant_id, workspace_id, owner_id)) = self.chat_row(session_id).await?
            else {
                return Ok(false);
            };
            return match relation {
                REL_OWNER => Ok(owner_id == user_id),
                REL_EDITOR => Ok(owner_id == user_id
                    || self
                        .direct_grant_matches_user(object, REL_EDITOR, user_id)
                        .await?),
                REL_VIEWER => {
                    if owner_id == user_id
                        || self
                            .direct_grant_matches_user(object, REL_VIEWER, user_id)
                            .await?
                        || self
                            .direct_grant_matches_user(object, REL_EDITOR, user_id)
                            .await?
                    {
                        return Ok(true);
                    }
                    if let Some(workspace_id) = workspace_id {
                        return self.workspace_accessor(workspace_id, user_id).await;
                    }
                    Ok(false)
                }
                _ => Ok(false),
            };
        }
        Ok(false)
    }
}

#[async_trait]
impl AuthorizationService for PgTestAuthorizationService {
    async fn check(&self, request: CheckRequest) -> Result<bool, AuthzError> {
        let user_id = parse_user_object(&request.user)?;
        self.check_user(user_id, &request.relation, &request.object)
            .await
    }

    async fn batch_check(
        &self,
        requests: Vec<CheckRequest>,
    ) -> Result<Vec<CheckResult>, AuthzError> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let allowed = self.check(request.clone()).await?;
            results.push(CheckResult { request, allowed });
        }
        Ok(results)
    }

    async fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
        _consistency: Consistency,
    ) -> Result<Vec<String>, AuthzError> {
        let ids: Vec<Uuid> = match object_type {
            TYPE_TENANT => sqlx::query_scalar("SELECT id FROM tenants"),
            TYPE_WORKSPACE => sqlx::query_scalar("SELECT id FROM workspaces"),
            TYPE_DOCUMENT => sqlx::query_scalar("SELECT id FROM documents"),
            TYPE_CHAT_SESSION => sqlx::query_scalar("SELECT id FROM chat_sessions"),
            _ => return Ok(Vec::new()),
        }
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AuthzError::Unavailable(e.to_string()))?;

        let mut out = Vec::new();
        for id in ids {
            let object = object(object_type, id);
            if self
                .check(CheckRequest::new(
                    user.to_string(),
                    relation.to_string(),
                    object.clone(),
                ))
                .await?
            {
                out.push(object);
            }
        }
        Ok(out)
    }

    async fn read_direct_relationships(
        &self,
        object: &str,
    ) -> Result<Vec<RelationshipTuple>, AuthzError> {
        let direct = self.direct.lock().await;
        Ok(direct
            .iter()
            .filter(|tuple| tuple.object == object)
            .cloned()
            .collect())
    }

    async fn read_all_direct_relationships(&self) -> Result<Vec<RelationshipTuple>, AuthzError> {
        // The in-memory `direct` set IS the full tuple set for the test
        // backend — no pagination needed.
        let direct = self.direct.lock().await;
        Ok(direct.iter().cloned().collect())
    }

    async fn write_relationships(
        &self,
        writes: Vec<RelationshipTuple>,
        deletes: Vec<RelationshipTuple>,
    ) -> Result<(), AuthzError> {
        let mut direct = self.direct.lock().await;
        for tuple in deletes {
            direct.remove(&tuple);
        }
        for tuple in writes {
            direct.insert(tuple);
        }
        Ok(())
    }

    async fn delete_all_direct_relationships_for_object(
        &self,
        object: &str,
    ) -> Result<(), AuthzError> {
        let mut direct = self.direct.lock().await;
        direct.retain(|tuple| tuple.object != object);
        Ok(())
    }

    async fn health(&self) -> Result<(), AuthzError> {
        Ok(())
    }
}

pub async fn check_or_unavailable(
    authz: &AuthzService,
    request: CheckRequest,
) -> Result<bool, crate::error::ApiError> {
    match authz.check(request).await {
        Ok(true) => {
            crate::metrics::metrics().inc_authz("allowed");
            Ok(true)
        }
        Ok(false) => {
            crate::metrics::metrics().inc_authz("denied");
            Ok(false)
        }
        Err(e) => {
            crate::metrics::metrics().inc_authz("error");
            warn!(error = %e, "authorization check failed closed");
            Err(crate::error::ApiError::AuthorizationUnavailable(
                e.to_string(),
            ))
        }
    }
}

pub async fn write_or_unavailable(
    authz: &AuthzService,
    writes: Vec<RelationshipTuple>,
    deletes: Vec<RelationshipTuple>,
) -> Result<(), crate::error::ApiError> {
    authz
        .write_relationships(writes, deletes)
        .await
        .map_err(|e| {
            warn!(error = %e, "authorization relationship write failed closed");
            crate::error::ApiError::AuthorizationUnavailable(e.to_string())
        })
}

pub async fn delete_object_or_unavailable(
    authz: &AuthzService,
    object: &str,
) -> Result<(), crate::error::ApiError> {
    authz
        .delete_all_direct_relationships_for_object(object)
        .await
        .map_err(|e| {
            warn!(error = %e, object, "authorization relationship cleanup failed closed");
            crate::error::ApiError::AuthorizationUnavailable(e.to_string())
        })
}

pub async fn list_objects_or_unavailable(
    authz: &AuthzService,
    user: &str,
    relation: &str,
    object_type: &str,
    consistency: Consistency,
) -> Result<Vec<String>, crate::error::ApiError> {
    authz
        .list_objects(user, relation, object_type, consistency)
        .await
        .map_err(|e| {
            warn!(
                error = %e,
                relation,
                object_type,
                "authorization list_objects failed closed"
            );
            crate::error::ApiError::AuthorizationUnavailable(e.to_string())
        })
}

pub fn parsed_uuid_set(objects: Vec<String>, expected_type: &str) -> (Vec<Uuid>, usize) {
    let mut ids = Vec::new();
    let mut malformed = 0;
    for object in objects {
        match typed_uuid(&object, expected_type) {
            Ok(id) => ids.push(id),
            Err(_) => malformed += 1,
        }
    }
    (ids, malformed)
}

pub fn group_by_object(tuples: Vec<RelationshipTuple>) -> HashMap<String, Vec<RelationshipTuple>> {
    let mut grouped: HashMap<String, Vec<RelationshipTuple>> = HashMap::new();
    for tuple in tuples {
        grouped.entry(tuple.object.clone()).or_default().push(tuple);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_uuid_rejects_wrong_type_and_userset() {
        let id = Uuid::new_v4();
        assert_eq!(
            typed_uuid(&format!("document:{id}"), TYPE_DOCUMENT).unwrap(),
            id
        );
        assert!(typed_uuid(&format!("workspace:{id}"), TYPE_DOCUMENT).is_err());
        assert!(typed_uuid(&format!("document:{id}#viewer"), TYPE_DOCUMENT).is_err());
    }

    #[test]
    fn grant_id_round_trips() {
        let tuple = RelationshipTuple::new(
            user_obj(Uuid::new_v4()),
            REL_VIEWER,
            document_obj(Uuid::new_v4()),
        );
        let encoded = encode_grant_id(&tuple);
        let decoded = decode_grant_id(&encoded).unwrap();
        assert_eq!(decoded.user, tuple.user);
        assert_eq!(decoded.relation, REL_VIEWER);
        assert_eq!(decoded.object, tuple.object);
    }

    #[test]
    fn malformed_grant_id_is_rejected() {
        assert!(decode_grant_id("not base64!").is_err());
    }

    #[test]
    fn workspace_member_principal_parses() {
        let id = Uuid::new_v4();
        let principal = parse_grant_principal(&workspace_member_userset(id)).unwrap();
        assert_eq!(principal, GrantPrincipal::Workspace(id));
    }

    // ---- Forward-compatibility regression tests for OpenFGA responses ----
    // OpenFGA v1.18.1 `check` returns {"allowed": true, "resolution": ""}.
    // Future versions may add more top-level fields. The API must parse
    // `allowed`/`objects` and silently ignore unknown fields rather than
    // fail closed with `authorization-unavailable`.

    #[derive(Deserialize)]
    struct CheckResponse {
        #[allow(dead_code)]
        #[serde(default)]
        resolution: Option<String>,
        allowed: bool,
    }

    #[derive(Deserialize)]
    struct ListObjectsResponse {
        objects: Vec<String>,
    }

    #[test]
    fn check_response_tolerates_resolution_field() {
        // Exact live v1.18.1 shape.
        let resp: CheckResponse =
            serde_json::from_str(r#"{"allowed":true,"resolution":""}"#).unwrap();
        assert!(resp.allowed);
        assert_eq!(resp.resolution.as_deref(), Some(""));
    }

    #[test]
    fn check_response_tolerates_unknown_future_fields() {
        // Hypothetical future OpenFGA response adding new top-level keys.
        let resp: CheckResponse = serde_json::from_str(
            r#"{"allowed":false,"resolution":"denied","debug_id":"abc","warnings":[]}"#,
        )
        .unwrap();
        assert!(!resp.allowed);
    }

    #[test]
    fn check_response_parses_minimal_shape() {
        let resp: CheckResponse = serde_json::from_str(r#"{"allowed":true}"#).unwrap();
        assert!(resp.allowed);
        assert!(resp.resolution.is_none());
    }

    #[test]
    fn list_objects_response_tolerates_unknown_fields() {
        let resp: ListObjectsResponse = serde_json::from_str(
            r#"{"objects":["tenant:550e8400-e29b-41d4-a716-446655440000"],"resolution":"","continuation_token":null}"#,
        )
        .unwrap();
        assert_eq!(resp.objects.len(), 1);
    }
}
