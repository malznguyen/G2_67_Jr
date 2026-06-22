//! Integration tests for the ReBAC `Check` engine (T66, Zanzibar-style).
//!
//! Exercises `check_relation` against a real PostgreSQL inside an RLS-scoped
//! transaction (`BEGIN; SET LOCAL ROLE gmrag_app; SET LOCAL app.tenant_id`),
//! mirroring how the per-request `SharedConnection` is built in production.
//!
//! Matrix: owner, direct user grant, concentric editor⊇owner / viewer⊇editor,
//! workspace-group grant, workspace inheritance (tuple_to_userset), the
//! public `shared` visibility, chat-session ownership, and cross-tenant denial.

use sqlx::PgPool;
use uuid::Uuid;

use gmrag_api::rbac::check::check_relation;
use gmrag_api::rbac::model::{
    ObjectRef, Principal, Relation, NS_CHAT_SESSION, NS_DOCUMENT, NS_WORKSPACE,
};

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_user(pool: &PgPool, email: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(email)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_tenant(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_workspace(pool: &PgPool, tenant_id: Uuid, created_by: Uuid, slug: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(slug)
    .bind(slug)
    .bind(created_by)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn add_workspace_member(pool: &PgPool, workspace_id: Uuid, tenant_id: Uuid, user_id: Uuid) {
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, 'member')",
    )
    .bind(workspace_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_document(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Option<Uuid>,
    owner_id: Uuid,
    visibility: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, visibility)
         VALUES ($1, $2, $3, $4, 'Doc', $5)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(owner_id)
    .bind(visibility)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_chat_session(pool: &PgPool, tenant_id: Uuid, user_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO chat_sessions (id, tenant_id, user_id, title) VALUES ($1, $2, $3, 'S')",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_grant(
    pool: &PgPool,
    tenant_id: Uuid,
    resource_type: &str,
    resource_id: Uuid,
    principal_type: &str,
    principal_id: Uuid,
    relation: &str,
) {
    sqlx::query(
        "INSERT INTO resource_acl
           (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(tenant_id)
    .bind(resource_type)
    .bind(resource_id)
    .bind(principal_type)
    .bind(principal_id)
    .bind(relation)
    .execute(pool)
    .await
    .unwrap();
}

/// Build an RLS-scoped raw connection (mirrors `rls_middleware`).
async fn rls_conn(pool: &PgPool, tenant_id: Uuid) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::Executor::execute(&mut *conn, "BEGIN").await.unwrap();
    sqlx::Executor::execute(&mut *conn, "SET LOCAL ROLE gmrag_app")
        .await
        .unwrap();
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
        .execute(&mut *conn)
        .await
        .unwrap();
    conn
}

fn doc(id: Uuid) -> ObjectRef {
    ObjectRef::new(NS_DOCUMENT, id)
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn owner_has_all_relations_on_own_document(pool: PgPool) {
    let owner = create_user(&pool, "owner@c66.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, None, owner, "private").await;

    let mut c = rls_conn(&pool, tenant).await;
    for rel in [Relation::Owner, Relation::Editor, Relation::Viewer] {
        assert!(
            check_relation(&mut c, &doc(d), rel, Principal::User(owner))
                .await
                .unwrap(),
            "owner must have {} on own document",
            rel.as_str()
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn stranger_has_no_access_to_private_document(pool: PgPool) {
    let owner = create_user(&pool, "owner@c66s.com").await;
    let stranger = create_user(&pool, "stranger@c66s.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, None, owner, "private").await;

    let mut c = rls_conn(&pool, tenant).await;
    for rel in [Relation::Owner, Relation::Editor, Relation::Viewer] {
        assert!(
            !check_relation(&mut c, &doc(d), rel, Principal::User(stranger))
                .await
                .unwrap(),
            "stranger must NOT have {} on a private document",
            rel.as_str()
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn shared_visibility_grants_viewer_to_everyone(pool: PgPool) {
    let owner = create_user(&pool, "owner@c66v.com").await;
    let other = create_user(&pool, "other@c66v.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, None, owner, "shared").await;

    let mut c = rls_conn(&pool, tenant).await;
    assert!(check_relation(&mut c, &doc(d), Relation::Viewer, Principal::User(other))
        .await
        .unwrap());
    // ...but a public share is read-only.
    assert!(!check_relation(&mut c, &doc(d), Relation::Editor, Principal::User(other))
        .await
        .unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn direct_viewer_grant_is_read_only(pool: PgPool) {
    let owner = create_user(&pool, "owner@c66g.com").await;
    let friend = create_user(&pool, "friend@c66g.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, None, owner, "private").await;
    insert_grant(&pool, tenant, NS_DOCUMENT, d, "user", friend, "viewer").await;

    let mut c = rls_conn(&pool, tenant).await;
    assert!(check_relation(&mut c, &doc(d), Relation::Viewer, Principal::User(friend))
        .await
        .unwrap());
    assert!(!check_relation(&mut c, &doc(d), Relation::Editor, Principal::User(friend))
        .await
        .unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn editor_grant_implies_viewer_concentric(pool: PgPool) {
    let owner = create_user(&pool, "owner@c66e.com").await;
    let editor = create_user(&pool, "editor@c66e.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, None, owner, "private").await;
    insert_grant(&pool, tenant, NS_DOCUMENT, d, "user", editor, "editor").await;

    let mut c = rls_conn(&pool, tenant).await;
    assert!(check_relation(&mut c, &doc(d), Relation::Editor, Principal::User(editor))
        .await
        .unwrap());
    assert!(
        check_relation(&mut c, &doc(d), Relation::Viewer, Principal::User(editor))
            .await
            .unwrap(),
        "editor must transitively be a viewer (concentric relations)"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn workspace_group_grant_reaches_members(pool: PgPool) {
    let owner = create_user(&pool, "owner@c66wg.com").await;
    let member = create_user(&pool, "member@c66wg.com").await;
    let outsider = create_user(&pool, "outsider@c66wg.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    add_workspace_member(&pool, ws, tenant, member).await;
    // Document lives outside the workspace, but is shared *with* the workspace.
    let d = insert_document(&pool, tenant, None, owner, "private").await;
    insert_grant(&pool, tenant, NS_DOCUMENT, d, "workspace", ws, "viewer").await;

    let mut c = rls_conn(&pool, tenant).await;
    assert!(
        check_relation(&mut c, &doc(d), Relation::Viewer, Principal::User(member))
            .await
            .unwrap(),
        "a workspace-group grant must reach the workspace's members"
    );
    assert!(
        !check_relation(&mut c, &doc(d), Relation::Viewer, Principal::User(outsider))
            .await
            .unwrap(),
        "a non-member must not inherit the workspace-group grant"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn workspace_members_inherit_viewer_via_parent(pool: PgPool) {
    let owner = create_user(&pool, "owner@c66p.com").await;
    let member = create_user(&pool, "member@c66p.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    add_workspace_member(&pool, ws, tenant, member).await;
    // Private document *inside* the workspace — inheritance via tuple_to_userset.
    let d = insert_document(&pool, tenant, Some(ws), owner, "private").await;

    let mut c = rls_conn(&pool, tenant).await;
    assert!(
        check_relation(&mut c, &doc(d), Relation::Viewer, Principal::User(member))
            .await
            .unwrap(),
        "a workspace member must inherit viewer on the workspace's documents"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn workspace_member_relation_direct(pool: PgPool) {
    let owner = create_user(&pool, "owner@c66m.com").await;
    let member = create_user(&pool, "member@c66m.com").await;
    let outsider = create_user(&pool, "outsider@c66m.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    add_workspace_member(&pool, ws, tenant, member).await;

    let mut c = rls_conn(&pool, tenant).await;
    let ws_obj = ObjectRef::new(NS_WORKSPACE, ws);
    assert!(check_relation(&mut c, &ws_obj, Relation::Member, Principal::User(member))
        .await
        .unwrap());
    assert!(!check_relation(&mut c, &ws_obj, Relation::Member, Principal::User(outsider))
        .await
        .unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn chat_session_owner_and_share(pool: PgPool) {
    let owner = create_user(&pool, "owner@c66c.com").await;
    let other = create_user(&pool, "other@c66c.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let s = insert_chat_session(&pool, tenant, owner).await;
    let obj = ObjectRef::new(NS_CHAT_SESSION, s);

    let mut c = rls_conn(&pool, tenant).await;
    assert!(check_relation(&mut c, &obj, Relation::Owner, Principal::User(owner))
        .await
        .unwrap());
    assert!(!check_relation(&mut c, &obj, Relation::Viewer, Principal::User(other))
        .await
        .unwrap());

    // Share the session with `other` as a viewer.
    insert_grant(&pool, tenant, NS_CHAT_SESSION, s, "user", other, "viewer").await;
    let mut c = rls_conn(&pool, tenant).await;
    assert!(check_relation(&mut c, &obj, Relation::Viewer, Principal::User(other))
        .await
        .unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn cross_tenant_grant_is_invisible(pool: PgPool) {
    let user_a = create_user(&pool, "a@c66x.com").await;
    let user_b = create_user(&pool, "b@c66x.com").await;
    let tenant_a = insert_tenant(&pool, "A").await;
    let tenant_b = insert_tenant(&pool, "B").await;
    let d_b = insert_document(&pool, tenant_b, None, user_b, "shared").await;
    // Even an explicit grant in tenant B must be invisible from tenant A.
    insert_grant(&pool, tenant_b, NS_DOCUMENT, d_b, "user", user_a, "viewer").await;

    // Caller operates in tenant A; RLS hides tenant B rows entirely.
    let mut c = rls_conn(&pool, tenant_a).await;
    assert!(
        !check_relation(&mut c, &doc(d_b), Relation::Viewer, Principal::User(user_a))
            .await
            .unwrap(),
        "a grant in another tenant must never be visible under RLS"
    );
}
