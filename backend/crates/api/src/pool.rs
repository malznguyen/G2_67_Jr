//! Newtype wrappers around [`sqlx::PgPool`] for axum request extensions.
//!
//! Both [`AdminPool`] and [`AppPool`] wrap the same underlying `PgPool`, but
//! they exist so that two pools with different roles can coexist in the axum
//! extension map without colliding (Rust's `TypeId`-keyed map cannot hold two
//! `Extension<PgPool>`).
//!
//! - [`AdminPool`]: connects as the `DATABASE_URL` superuser (`gmrag`).
//!   Used for platform-level / cross-tenant operations that must bypass RLS:
//!   migrations (run before serving), JWT user provisioning, tenant membership
//!   checks (which happen *before* the RLS tenant context is established),
//!   and cross-tenant endpoints such as `GET /users/me`.
//!
//! - [`AppPool`]: every connection runs `SET ROLE gmrag_app` via
//!   [`gmrag_core::init_app_pool`], so PostgreSQL Row Level Security is
//!   enforced. Used exclusively by [`crate::middleware::rls::rls_middleware`]
//!   to acquire the per-request [`SharedConnection`] on which
//!   `SET LOCAL app.tenant_id` is applied.

use sqlx::PgPool;

/// Pool that connects as the superuser / owner role (bypasses RLS).
///
/// Injected as `Extension<AdminPool>` for platform-level and cross-tenant
/// handlers and for the auth/tenant middleware chain.
#[derive(Clone)]
pub struct AdminPool(pub PgPool);

/// Pool whose connections run as `gmrag_app` (RLS enforced).
///
/// Injected as `Extension<AppPool>` and consumed by
/// [`crate::middleware::rls::rls_middleware`].
#[derive(Clone)]
pub struct AppPool(pub PgPool);

impl std::ops::Deref for AdminPool {
    type Target = PgPool;
    fn deref(&self) -> &PgPool {
        &self.0
    }
}

impl std::ops::Deref for AppPool {
    type Target = PgPool;
    fn deref(&self) -> &PgPool {
        &self.0
    }
}
