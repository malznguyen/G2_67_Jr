//! OpenAPI / Swagger UI route tests (T84A).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gmrag_api::openapi::{swagger_router, ApiDoc};
use tower::ServiceExt;
use utoipa::OpenApi;
use utoipa::openapi::security::{HttpAuthScheme, SecurityScheme};

fn docs_app() -> axum::Router {
    swagger_router()
}

#[tokio::test]
async fn swagger_ui_available() {
    let app = docs_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/swagger/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn openapi_json_generated() {
    let app = docs_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.get("openapi").is_some());
    assert_eq!(json["info"]["title"], "GMRAG API");
}

#[tokio::test]
async fn security_scheme_present() {
    let doc = ApiDoc::openapi();
    let schemes = doc
        .components
        .as_ref()
        .map(|c| &c.security_schemes)
        .expect("components.securitySchemes");
    let bearer = schemes.get("bearer_auth").expect("bearer_auth scheme");
    assert!(matches!(bearer, SecurityScheme::Http(_)));
    if let SecurityScheme::Http(http) = bearer {
        assert!(matches!(http.scheme, HttpAuthScheme::Bearer));
    }
}

fn count_operations(doc: &utoipa::openapi::OpenApi) -> usize {
    doc.paths
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
        .sum()
}

#[tokio::test]
async fn paths_populated() {
    let doc = ApiDoc::openapi();
    let op_count = count_operations(&doc);
    assert!(
        op_count >= 34,
        "expected at least 34 operations, got {op_count}"
    );
}

#[tokio::test]
async fn tags_present() {
    let doc = ApiDoc::openapi();
    let tag_names: Vec<_> = doc
        .tags
        .as_ref()
        .map(|tags| tags.iter().map(|t| t.name.as_str()).collect())
        .unwrap_or_default();
    for expected in [
        "Health",
        "Users",
        "Tenants",
        "Documents",
        "Chat",
        "ACL",
        "Metering",
    ] {
        assert!(
            tag_names.contains(&expected),
            "missing tag {expected}"
        );
    }
}
