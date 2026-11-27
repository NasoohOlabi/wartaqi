use axum::{
    body::Body,
    http::{Method, Response, StatusCode, header},
};
use url::Url;

const CACHE_CONTROL: &str = "public, max-age=15, stale-while-revalidate=15, stale-if-error=86400";
const RESOURCE: &str = concat!(
    r#"{"resource":"ORIGIN/mcp","#,
    r#""authorization_servers":["ORIGIN/api/auth"],"#,
    r#""bearer_methods_supported":["header"],"#,
    r#""dpop_signing_alg_values_supported":["EdDSA","#,
    r#""ES256","#,
    r#""ES512","#,
    r#""PS256","#,
    r#""RS256"],"#,
    r#""scopes_supported":["mcp:read","#,
    r#""mcp:write"]}"#,
);
const AUTHORIZATION_SERVER: &str = concat!(
    r#"{"client_id_metadata_document_supported":true,"#,
    r#""scopes_supported":["mcp:read","#,
    r#""mcp:write","#,
    r#""offline_access"],"#,
    r#""issuer":"ORIGIN/api/auth","#,
    r#""authorization_endpoint":"ORIGIN/api/auth/oauth2/authorize","#,
    r#""token_endpoint":"ORIGIN/api/auth/oauth2/token","#,
    r#""registration_endpoint":"ORIGIN/api/auth/oauth2/register","#,
    r#""introspection_endpoint":"ORIGIN/api/auth/oauth2/introspect","#,
    r#""revocation_endpoint":"ORIGIN/api/auth/oauth2/revoke","#,
    r#""response_types_supported":["code"],"#,
    r#""response_modes_supported":["query"],"#,
    r#""grant_types_supported":["authorization_code","#,
    r#""refresh_token"],"#,
    r#""token_endpoint_auth_methods_supported":["none","#,
    r#""client_secret_basic","#,
    r#""client_secret_post","#,
    r#""private_key_jwt"],"#,
    r#""token_endpoint_auth_signing_alg_values_supported":["RS256","#,
    r#""RS384","#,
    r#""RS512","#,
    r#""PS256","#,
    r#""PS384","#,
    r#""PS512","#,
    r#""ES256","#,
    r#""ES384","#,
    r#""ES512","#,
    r#""EdDSA"],"#,
    r#""introspection_endpoint_auth_methods_supported":["client_secret_basic","#,
    r#""client_secret_post","#,
    r#""private_key_jwt"],"#,
    r#""introspection_endpoint_auth_signing_alg_values_supported":["RS256","#,
    r#""RS384","#,
    r#""RS512","#,
    r#""PS256","#,
    r#""PS384","#,
    r#""PS512","#,
    r#""ES256","#,
    r#""ES384","#,
    r#""ES512","#,
    r#""EdDSA"],"#,
    r#""revocation_endpoint_auth_methods_supported":["client_secret_basic","#,
    r#""client_secret_post","#,
    r#""private_key_jwt"],"#,
    r#""revocation_endpoint_auth_signing_alg_values_supported":["RS256","#,
    r#""RS384","#,
    r#""RS512","#,
    r#""PS256","#,
    r#""PS384","#,
    r#""PS512","#,
    r#""ES256","#,
    r#""ES384","#,
    r#""ES512","#,
    r#""EdDSA"],"#,
    r#""code_challenge_methods_supported":["S256"],"#,
    r#""authorization_response_iss_parameter_supported":true,"#,
    r#""dpop_signing_alg_values_supported":["EdDSA","#,
    r#""ES256","#,
    r#""ES512","#,
    r#""PS256","#,
    r#""RS256"],"#,
    r#""backchannel_logout_supported":false,"#,
    r#""backchannel_logout_session_supported":false}"#,
);

/// Metadata uses the trusted configured origin. Public discovery does not inspect
/// request Host or Origin, matching Better Auth; the MCP endpoint checks both.
pub(crate) struct Discovery {
    origin: String,
}

impl Discovery {
    pub(crate) fn new(origin: &str) -> Result<Self, &'static str> {
        let url = Url::parse(origin).map_err(|_| "Invalid origin")?;
        if url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || !(url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))))
        {
            return Err("Invalid origin");
        }
        Ok(Self {
            origin: url.origin().ascii_serialization(),
        })
    }

    pub(crate) fn handle(&self, path: &str, method: &Method) -> Option<Response<Body>> {
        let body = match path {
            "/.well-known/oauth-protected-resource"
            | "/.well-known/oauth-protected-resource/mcp" => RESOURCE,
            "/.well-known/oauth-authorization-server/api/auth" => AUTHORIZATION_SERVER,
            "/.well-known/oauth-authorization-server" => {
                return Some(
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap(),
                );
            }
            _ => return None,
        };
        if method != Method::GET && method != Method::HEAD {
            return Some(
                Response::builder()
                    .status(StatusCode::METHOD_NOT_ALLOWED)
                    .header(header::ALLOW, "GET, HEAD")
                    .body(Body::empty())
                    .unwrap(),
            );
        }
        Some(
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CACHE_CONTROL, CACHE_CONTROL)
                .body(if method == Method::HEAD {
                    Body::empty()
                } else {
                    Body::from(body.replace("ORIGIN", &self.origin))
                })
                .unwrap(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::json;

    #[tokio::test]
    async fn matches_bun_discovery_and_methods() {
        let origin = "http://127.0.0.1:3000";
        let discovery = Discovery::new(origin).unwrap();
        for path in [
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/mcp",
        ] {
            let response = discovery.handle(path, &Method::GET).unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CACHE_CONTROL], CACHE_CONTROL);
            let bytes = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                body,
                json!({"resource":format!("{origin}/mcp"),"authorization_servers":[format!("{origin}/api/auth")],"bearer_methods_supported":["header"],"dpop_signing_alg_values_supported":["EdDSA","ES256","ES512","PS256","RS256"],"scopes_supported":["mcp:read","mcp:write"]})
            );
            let head = discovery.handle(path, &Method::HEAD).unwrap();
            assert!(to_bytes(head.into_body(), 1024).await.unwrap().is_empty());
            let post = discovery.handle(path, &Method::POST).unwrap();
            assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
            assert_eq!(post.headers()[header::ALLOW], "GET, HEAD");
        }
        let provider = discovery
            .handle(
                "/.well-known/oauth-authorization-server/api/auth",
                &Method::GET,
            )
            .unwrap();
        let bytes = to_bytes(provider.into_body(), 16 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["issuer"], format!("{origin}/api/auth"));
        assert_eq!(body["code_challenge_methods_supported"], json!(["S256"]));
        assert_eq!(body["client_id_metadata_document_supported"], true);
        assert_eq!(
            body["token_endpoint"],
            format!("{origin}/api/auth/oauth2/token")
        );
        assert_eq!(
            discovery
                .handle("/.well-known/oauth-authorization-server", &Method::GET)
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert!(discovery.handle("/other", &Method::GET).is_none());
    }

    #[test]
    fn origin_uses_only_configured_secure_or_local_authority() {
        assert!(Discovery::new("https://example.org").is_ok());
        assert!(Discovery::new("http://localhost:3000").is_ok());
        assert!(Discovery::new("http://example.org").is_err());
        assert!(Discovery::new("https://example.org/path").is_err());
        assert!(Discovery::new("https://user@example.org").is_err());
    }
}
