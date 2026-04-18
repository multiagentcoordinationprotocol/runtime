use crate::auth::resolver::{AuthError, AuthResolver, ResolvedIdentity};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::metadata::MetadataMap;

#[derive(Debug, Clone, Deserialize)]
struct MACPClaims {
    sub: String,
    #[serde(default)]
    macp_scopes: Option<MACPScopes>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct MACPScopes {
    #[serde(default)]
    can_start_sessions: Option<bool>,
    #[serde(default)]
    can_manage_mode_registry: Option<bool>,
    #[serde(default)]
    is_observer: Option<bool>,
    #[serde(default)]
    allowed_modes: Option<Vec<String>>,
    #[serde(default)]
    max_open_sessions: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub issuer: String,
    pub audience: String,
    pub algorithms: Vec<Algorithm>,
}

struct CachedKeys {
    keys: Vec<DecodingKey>,
    fetched_at: std::time::Instant,
}

pub struct JwtBearerResolver {
    config: JwtConfig,
    jwks_source: JwksSource,
    cached_keys: Arc<RwLock<Option<CachedKeys>>>,
    cache_ttl: std::time::Duration,
}

enum JwksSource {
    Inline(Vec<DecodingKey>),
    Url(String),
}

impl JwtBearerResolver {
    pub fn from_inline_json(config: JwtConfig, jwks_json: &str) -> Result<Self, String> {
        let jwks: serde_json::Value =
            serde_json::from_str(jwks_json).map_err(|e| format!("invalid JWKS JSON: {e}"))?;
        let keys = Self::parse_jwks(&jwks)?;
        tracing::info!(
            keys = keys.len(),
            issuer = %config.issuer,
            "JWT resolver initialized with inline JWKS"
        );
        Ok(Self {
            config,
            jwks_source: JwksSource::Inline(keys.clone()),
            cached_keys: Arc::new(RwLock::new(Some(CachedKeys {
                keys,
                fetched_at: std::time::Instant::now(),
            }))),
            cache_ttl: std::time::Duration::from_secs(u64::MAX),
        })
    }

    pub fn from_url(config: JwtConfig, url: String, cache_ttl_secs: u64) -> Self {
        tracing::info!(
            url = %url,
            issuer = %config.issuer,
            cache_ttl_secs,
            "JWT resolver initialized with JWKS URL"
        );
        Self {
            config,
            jwks_source: JwksSource::Url(url),
            cached_keys: Arc::new(RwLock::new(None)),
            cache_ttl: std::time::Duration::from_secs(cache_ttl_secs),
        }
    }

    fn extract_bearer(metadata: &MetadataMap) -> Option<String> {
        metadata
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::to_string)
    }

    async fn get_keys(&self) -> Result<Vec<DecodingKey>, AuthError> {
        {
            let guard = self.cached_keys.read().await;
            if let Some(cached) = guard.as_ref() {
                if cached.fetched_at.elapsed() < self.cache_ttl {
                    return Ok(cached.keys.clone());
                }
            }
        }

        match &self.jwks_source {
            JwksSource::Inline(keys) => Ok(keys.clone()),
            JwksSource::Url(url) => {
                let keys = self.fetch_jwks(url).await?;
                let mut guard = self.cached_keys.write().await;
                *guard = Some(CachedKeys {
                    keys: keys.clone(),
                    fetched_at: std::time::Instant::now(),
                });
                Ok(keys)
            }
        }
    }

    async fn fetch_jwks(&self, url: &str) -> Result<Vec<DecodingKey>, AuthError> {
        let resp = reqwest::get(url)
            .await
            .map_err(|e| AuthError::FetchFailed(format!("JWKS fetch failed: {e}")))?;
        let jwks: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AuthError::FetchFailed(format!("JWKS parse failed: {e}")))?;
        Self::parse_jwks(&jwks).map_err(AuthError::FetchFailed)
    }

    fn parse_jwks(jwks: &serde_json::Value) -> Result<Vec<DecodingKey>, String> {
        let keys_arr = jwks
            .get("keys")
            .and_then(|k| k.as_array())
            .ok_or_else(|| "JWKS missing 'keys' array".to_string())?;

        let mut decoding_keys = Vec::new();
        for key in keys_arr {
            let kty = key.get("kty").and_then(|v| v.as_str()).unwrap_or("");
            match kty {
                "RSA" => {
                    let n = key.get("n").and_then(|v| v.as_str()).unwrap_or("");
                    let e = key.get("e").and_then(|v| v.as_str()).unwrap_or("");
                    if !n.is_empty() && !e.is_empty() {
                        if let Ok(dk) = DecodingKey::from_rsa_components(n, e) {
                            decoding_keys.push(dk);
                        }
                    }
                }
                "EC" => {
                    let x = key.get("x").and_then(|v| v.as_str()).unwrap_or("");
                    let y = key.get("y").and_then(|v| v.as_str()).unwrap_or("");
                    let crv = key.get("crv").and_then(|v| v.as_str()).unwrap_or("P-256");
                    if !x.is_empty() && !y.is_empty() {
                        if let Ok(dk) = DecodingKey::from_ec_components(x, y) {
                            let _ = crv;
                            decoding_keys.push(dk);
                        }
                    }
                }
                "oct" => {
                    if let Some(k_val) = key.get("k").and_then(|v| v.as_str()) {
                        decoding_keys.push(
                            DecodingKey::from_base64_secret(k_val)
                                .unwrap_or_else(|_| DecodingKey::from_secret(k_val.as_bytes())),
                        );
                    }
                }
                _ => {}
            }
        }

        if decoding_keys.is_empty() {
            return Err("no usable keys found in JWKS".to_string());
        }
        Ok(decoding_keys)
    }
}

#[async_trait::async_trait]
impl AuthResolver for JwtBearerResolver {
    fn name(&self) -> &str {
        "jwt_bearer"
    }

    async fn resolve(&self, metadata: &MetadataMap) -> Result<Option<ResolvedIdentity>, AuthError> {
        let token = match Self::extract_bearer(metadata) {
            Some(t) => t,
            None => return Ok(None),
        };

        // Only handle JWT-shaped tokens (contain dots)
        if !token.contains('.') {
            return Ok(None);
        }

        let keys = self.get_keys().await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);
        validation.algorithms = self.config.algorithms.clone();

        let mut last_err = None;
        for key in &keys {
            match decode::<MACPClaims>(&token, key, &validation) {
                Ok(token_data) => {
                    let claims = token_data.claims;
                    let scopes = claims.macp_scopes.unwrap_or_default();

                    return Ok(Some(ResolvedIdentity {
                        sender: claims.sub,
                        allowed_modes: scopes.allowed_modes.map(|m| m.into_iter().collect()),
                        can_start_sessions: scopes.can_start_sessions.unwrap_or(true),
                        max_open_sessions: scopes.max_open_sessions,
                        can_manage_mode_registry: scopes.can_manage_mode_registry.unwrap_or(false),
                        is_observer: scopes.is_observer.unwrap_or(false),
                        resolver: "jwt_bearer".to_string(),
                    }));
                }
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            }
        }

        match last_err {
            Some(e) => {
                use jsonwebtoken::errors::ErrorKind;
                match e.kind() {
                    ErrorKind::ExpiredSignature => Err(AuthError::Expired),
                    ErrorKind::InvalidIssuer => {
                        Err(AuthError::InvalidCredential("invalid issuer".to_string()))
                    }
                    ErrorKind::InvalidAudience => {
                        Err(AuthError::InvalidCredential("invalid audience".to_string()))
                    }
                    _ => Err(AuthError::InvalidCredential(format!(
                        "JWT validation failed: {e}"
                    ))),
                }
            }
            None => Err(AuthError::InvalidCredential(
                "no keys available to validate JWT".to_string(),
            )),
        }
    }
}
