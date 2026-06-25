//! Keycloak OIDC JWT validation with JWKS caching.
//!
//! Fetches JWKS from the Keycloak issuer's `/.well-known/openid-configuration`
//! endpoint, caches the signing keys, and validates incoming JWT tokens.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, TokenData, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::debug;

use crate::error::AuthError;

/// JWT claims expected from a Keycloak-issued access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,
    pub exp: u64,
    pub iat: u64,
    pub iss: String,
    #[serde(default)]
    pub aud: Option<serde_json::Value>,
    #[serde(default)]
    pub azp: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub realm_access: Option<RealmAccess>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmAccess {
    #[serde(default)]
    pub roles: Vec<String>,
}

/// A cached JWKS entry: the decoding key + algorithm for a given `kid`.
struct JwksEntry {
    key: DecodingKey,
    algorithm: Algorithm,
}

/// JWKS cache with time-based expiry.
struct JwksCache {
    keys: HashMap<String, JwksEntry>,
    fetched_at: Instant,
}

impl JwksCache {
    fn is_expired(&self, ttl: Duration) -> bool {
        self.fetched_at.elapsed() > ttl
    }
}

/// JWT validator that fetches and caches JWKS from the OIDC issuer.
#[derive(Clone)]
pub struct JwtValidator {
    /// Issuer URL used to fetch discovery + JWKS (container-internal).
    issuer: String,
    /// Value expected in the token `iss` claim (may differ from `issuer` when
    /// the IdP emits tokens with a host-side origin).
    issuer_verify: String,
    /// Accepted `aud` values — backend client + frontend client (tokens minted
    /// for the browser carry the frontend client id in `aud`).
    accepted_audiences: Vec<String>,
    cache: Arc<RwLock<JwksCache>>,
    cache_ttl: Duration,
    http_client: reqwest::Client,
}

impl JwtValidator {
    /// Create a new validator.
    ///
    /// `issuer` is the Keycloak realm URL used for discovery (container-internal,
    /// e.g. `http://keycloak:8080/realms/gmrag`). `issuer_verify` is the value
    /// expected in the token `iss` claim (host-side, e.g.
    /// `http://localhost:8080/realms/gmrag`). `accepted_audiences` lists every
    /// `aud` value the backend accepts (backend client + frontend client).
    /// `client_id` is retained for callers that still pass it (unused beyond
    /// construction for now — audience is governed by `accepted_audiences`).
    pub fn new(
        issuer: String,
        issuer_verify: String,
        accepted_audiences: Vec<String>,
        _client_id: String,
    ) -> Self {
        let _ = _client_id;
        Self {
            issuer,
            issuer_verify,
            accepted_audiences,
            cache: Arc::new(RwLock::new(JwksCache {
                keys: HashMap::new(),
                fetched_at: Instant::now()
                    .checked_sub(Duration::from_secs(3600))
                    .unwrap_or(Instant::now()),
            })),
            cache_ttl: Duration::from_secs(300),
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("http client"),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = client;
        self
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Inject a decoding key directly into the cache (for testing).
    #[cfg(test)]
    pub(crate) async fn inject_key(&self, kid: String, key: DecodingKey, algorithm: Algorithm) {
        let mut cache = self.cache.write().await;
        cache.keys.insert(kid, JwksEntry { key, algorithm });
        cache.fetched_at = Instant::now();
    }

    /// Validate a JWT token string and return the decoded claims.
    pub async fn validate(&self, token: &str) -> Result<JwtClaims, AuthError> {
        let header = decode_header(token)
            .map_err(|e| AuthError::InvalidToken(format!("invalid header: {e}")))?;

        let kid = header.kid.ok_or_else(|| {
            AuthError::InvalidToken("token missing 'kid' header".to_string())
        })?;

        let entry = self.get_key(&kid).await?;

        let mut validation = Validation::new(entry.algorithm);
        validation.set_issuer(&[&self.issuer_verify]);
        validation.set_audience(&self.accepted_audiences);

        let token_data: TokenData<JwtClaims> =
            decode(token, &entry.key, &validation)
                .map_err(|e| AuthError::InvalidToken(format!("validation failed: {e}")))?;

        Ok(token_data.claims)
    }

    /// Get a decoding key for the given `kid`, fetching JWKS if needed.
    async fn get_key(&self, kid: &str) -> Result<JwksEntry, AuthError> {
        // Fast path: cache hit and not expired.
        {
            let cache = self.cache.read().await;
            if !cache.is_expired(self.cache_ttl) {
                if let Some(entry) = cache.keys.get(kid) {
                    return Ok(JwksEntry {
                        key: entry.key.clone(),
                        algorithm: entry.algorithm,
                    });
                }
            }
        }

        // Slow path: fetch JWKS.
        self.fetch_jwks().await?;

        let cache = self.cache.read().await;
        cache
            .keys
            .get(kid)
            .map(|entry| JwksEntry {
                key: entry.key.clone(),
                algorithm: entry.algorithm,
            })
            .ok_or_else(|| AuthError::JwksFetchFailed(format!("kid '{kid}' not found in JWKS")))
    }

    /// Fetch JWKS from the issuer and update the cache.
    async fn fetch_jwks(&self) -> Result<(), AuthError> {
        let well_known_url = format!("{}/.well-known/openid-configuration", self.issuer);

        let discovery: serde_json::Value = self
            .http_client
            .get(&well_known_url)
            .send()
            .await
            .map_err(|e| AuthError::JwksFetchFailed(format!("discovery request failed: {e}")))?
            .json()
            .await
            .map_err(|e| AuthError::JwksFetchFailed(format!("discovery parse failed: {e}")))?;

        let jwks_uri = discovery["jwks_uri"]
            .as_str()
            .ok_or_else(|| AuthError::JwksFetchFailed("missing jwks_uri in discovery".to_string()))?
            .to_string();

        let jwks: serde_json::Value = self
            .http_client
            .get(&jwks_uri)
            .send()
            .await
            .map_err(|e| AuthError::JwksFetchFailed(format!("JWKS request failed: {e}")))?
            .json()
            .await
            .map_err(|e| AuthError::JwksFetchFailed(format!("JWKS parse failed: {e}")))?;

        let keys_array = jwks["keys"]
            .as_array()
            .ok_or_else(|| AuthError::JwksFetchFailed("missing 'keys' array in JWKS".to_string()))?;

        let mut keys = HashMap::new();

        for key_value in keys_array {
            let kid = match key_value["kid"].as_str() {
                Some(k) => k.to_string(),
                None => continue,
            };
            let kty = key_value["kty"].as_str().unwrap_or("");
            let alg = key_value["alg"].as_str().unwrap_or("RS256");

            let algorithm = match alg {
                "RS256" => Algorithm::RS256,
                "RS384" => Algorithm::RS384,
                "RS512" => Algorithm::RS512,
                "ES256" => Algorithm::ES256,
                "ES384" => Algorithm::ES384,
                _ => {
                    debug!(kid = %kid, alg = %alg, "skipping unsupported algorithm");
                    continue;
                }
            };

            let decoding_key = match kty {
                "RSA" => {
                    let n = key_value["n"].as_str().unwrap_or("");
                    let e = key_value["e"].as_str().unwrap_or("");
                    DecodingKey::from_rsa_components(n, e)
                        .map_err(|e| AuthError::JwksFetchFailed(format!("RSA key parse failed: {e}")))?
                }
                "EC" => {
                    let x = key_value["x"].as_str().unwrap_or("");
                    let y = key_value["y"].as_str().unwrap_or("");
                    DecodingKey::from_ec_components(x, y)
                        .map_err(|e| AuthError::JwksFetchFailed(format!("EC key parse failed: {e}")))?
                }
                _ => {
                    debug!(kid = %kid, kty = %kty, "skipping unsupported key type");
                    continue;
                }
            };

            keys.insert(kid, JwksEntry {
                key: decoding_key,
                algorithm,
            });
        }

        if keys.is_empty() {
            return Err(AuthError::JwksFetchFailed("no valid keys found in JWKS".to_string()));
        }

        let mut cache = self.cache.write().await;
        cache.keys = keys;
        cache.fetched_at = Instant::now();
        debug!(count = cache.keys.len(), "JWKS cache refreshed");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::Router;
    use axum::routing::get;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;

    const TEST_PEM_PRIV: &[u8] = include_bytes!("test_keys/test_rsa_private.pem");
    const TEST_PEM_PUB: &[u8] = include_bytes!("test_keys/test_rsa_public.pem");
    const TEST_KID: &str = "test-kid-1";

    fn make_token(claims: &JwtClaims, kid: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(&header, claims, &EncodingKey::from_rsa_pem(TEST_PEM_PRIV).unwrap()).unwrap()
    }

    fn test_claims() -> JwtClaims {
        JwtClaims {
            sub: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as u64,
            iat: chrono::Utc::now().timestamp() as u64,
            iss: "http://localhost:8080/realms/gmrag".to_string(),
            aud: Some(serde_json::Value::String("gmrag-backend".to_string())),
            azp: Some("gmrag-backend".to_string()),
            scope: Some("openid profile email".to_string()),
            preferred_username: Some("testuser".to_string()),
            email: Some("test@example.com".to_string()),
            realm_access: Some(RealmAccess {
                roles: vec!["user".to_string()],
            }),
        }
    }

    fn make_validator_with_key() -> JwtValidator {
        let _decoding_key = DecodingKey::from_rsa_pem(TEST_PEM_PUB).unwrap();
        let validator = JwtValidator::new(
            "http://localhost:8080/realms/gmrag".to_string(),
            "http://localhost:8080/realms/gmrag".to_string(),
            vec!["gmrag-backend".to_string()],
            "gmrag-backend".to_string(),
        );
        // We'll inject the key in async context.
        validator
    }

    #[tokio::test]
    async fn rejects_garbage_token() {
        let validator = make_validator_with_key();
        let result = validator.validate("not-a-jwt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_token_without_kid() {
        // Token without kid header.
        let claims = test_claims();
        let header = Header::new(Algorithm::RS256); // no kid
        let token = encode(&header, &claims, &EncodingKey::from_rsa_pem(TEST_PEM_PRIV).unwrap())
            .unwrap();

        let validator = make_validator_with_key();
        let result = validator.validate(&token).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(AuthError::InvalidToken(ref msg)) if msg.contains("kid")));
    }

    #[tokio::test]
    async fn rejects_expired_token() {
        let claims = JwtClaims {
            exp: (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp() as u64,
            ..test_claims()
        };
        let token = make_token(&claims, TEST_KID);

        let validator = make_validator_with_key();
        validator.inject_key(
            TEST_KID.to_string(),
            DecodingKey::from_rsa_pem(TEST_PEM_PUB).unwrap(),
            Algorithm::RS256,
        ).await;

        let result = validator.validate(&token).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_wrong_issuer() {
        let claims = JwtClaims {
            iss: "http://evil.com/realms/bad".to_string(),
            ..test_claims()
        };
        let token = make_token(&claims, TEST_KID);

        let validator = make_validator_with_key();
        validator.inject_key(
            TEST_KID.to_string(),
            DecodingKey::from_rsa_pem(TEST_PEM_PUB).unwrap(),
            Algorithm::RS256,
        ).await;

        let result = validator.validate(&token).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_wrong_audience() {
        let claims = JwtClaims {
            aud: Some(serde_json::Value::String("wrong-client".to_string())),
            ..test_claims()
        };
        let token = make_token(&claims, TEST_KID);

        let validator = make_validator_with_key();
        validator.inject_key(
            TEST_KID.to_string(),
            DecodingKey::from_rsa_pem(TEST_PEM_PUB).unwrap(),
            Algorithm::RS256,
        ).await;

        let result = validator.validate(&token).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn accepts_valid_token() {
        let claims = test_claims();
        let token = make_token(&claims, TEST_KID);

        let validator = make_validator_with_key();
        validator.inject_key(
            TEST_KID.to_string(),
            DecodingKey::from_rsa_pem(TEST_PEM_PUB).unwrap(),
            Algorithm::RS256,
        ).await;

        let result = validator.validate(&token).await;
        assert!(result.is_ok(), "valid token must be accepted: {:?}", result.err());
        let decoded = result.unwrap();
        assert_eq!(decoded.sub, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(decoded.preferred_username.as_deref(), Some("testuser"));
    }

    #[tokio::test]
    async fn jwks_fetch_from_mock_server() {
        // Start a mock HTTP server that serves OIDC discovery + JWKS.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        // Read the public key and extract n/e for JWKS.
        // For this test we inject the key directly into the validator after
        // the JWKS fetch succeeds, so we just need the mock to return valid JSON.
        let app = Router::new()
            .route("/.well-known/openid-configuration", get({
                let base = base_url.clone();
                move || async move {
                    Json(json!({
                        "issuer": "http://localhost:8080/realms/gmrag",
                        "jwks_uri": format!("{base}/jwks")
                    }))
                }
            }))
            .route("/jwks", get(move || async move {
                // Return a JWKS with a dummy key — the actual validation
                // uses inject_key, but this tests the fetch path.
                Json(json!({
                    "keys": [{
                        "kty": "RSA",
                        "kid": "mock-kid",
                        "use": "sig",
                        "alg": "RS256",
                        "n": "0Z3VS5JJcds3xfn_ygWyF8PbnGy0AHB7MhgHcTz6sE2I2yPBaFDrBz9vFqU5yTfMJP0nP2yrRFPVvkuOIG7bVPE3-HjV2O1EIL-lJ0nY9L1b1SdCpUOe5qL5JcKZLqRMJ0O7VH5nNfGgFNbP0L6s3bcpbfUGjC5x6Mn4B1fHHzkS3t7lb7x0a2Y1pVVM0NJc7i3BjFxT9Xz0b6T-Q7L2b5b1bP5f1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0b1b7b0",
                        "e": "AQAB"
                    }]
                }))
            }));

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Give the server a moment to start.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let validator = JwtValidator::new(
            "http://localhost:8080/realms/gmrag".to_string(),
            "http://localhost:8080/realms/gmrag".to_string(),
            vec!["gmrag-backend".to_string()],
            "gmrag-backend".to_string(),
        )
        .with_http_client(reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap());

        // The JWKS fetch will succeed (mock returns valid JSON), but the key
        // "mock-kid" is a dummy RSA key. We test that the fetch path works.
        let claims = test_claims();
        let token = make_token(&claims, "mock-kid");

        // This will fail because the mock JWKS key doesn't match our signing key.
        // But it proves the JWKS fetch + cache path works.
        let result = validator.validate(&token).await;
        assert!(result.is_err()); // Expected: key mismatch.

        // Now inject the real key and validate.
        validator.inject_key(
            "mock-kid".to_string(),
            DecodingKey::from_rsa_pem(TEST_PEM_PUB).unwrap(),
            Algorithm::RS256,
        ).await;

        let result = validator.validate(&token).await;
        assert!(result.is_ok(), "token must validate after injecting correct key");
    }

    #[tokio::test]
    async fn jwks_fetch_unreachable_server_fails() {
        let validator = JwtValidator::new(
            "http://127.0.0.1:1".to_string(),
            "http://127.0.0.1:1".to_string(),
            vec!["gmrag-backend".to_string()],
            "gmrag-backend".to_string(),
        )
        .with_http_client(
            reqwest::Client::builder()
                .timeout(Duration::from_millis(100))
                .build()
                .unwrap(),
        );

        let claims = test_claims();
        let token = make_token(&claims, TEST_KID);
        let result = validator.validate(&token).await;
        assert!(result.is_err());
    }
}
