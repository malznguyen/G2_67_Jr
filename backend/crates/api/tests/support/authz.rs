use std::sync::Arc;

use gmrag_api::authz::{AuthzService, PgTestAuthorizationService};
use sqlx::PgPool;

pub fn test_authz(pool: &PgPool) -> AuthzService {
    Arc::new(PgTestAuthorizationService::new(pool.clone()))
}
