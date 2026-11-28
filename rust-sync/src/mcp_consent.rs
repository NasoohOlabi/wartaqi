use axum::{
    extract::Request,
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ring::rand::{SecureRandom, SystemRandom};

pub(crate) async fn page(request: Request, secret: &str) -> Response {
    if request.method() != Method::GET {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET")],
            "Method not allowed",
        )
            .into_response();
    }
    if !crate::auth_browser::verify_oauth_query(request.uri().query().unwrap_or_default(), secret) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CACHE_CONTROL, "no-store")],
            "This connection request is invalid or expired. Start again in your MCP client.",
        )
            .into_response();
    }
    let mut nonce = [0_u8; 16];
    if SystemRandom::new().fill(&mut nonce).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let nonce = STANDARD.encode(nonce);
    let body = include_str!("mcp_consent.html")
        .replace("__WARTAQ_CSP_NONCE__", &nonce)
        .replace(
            "__WARTAQ_IS_CONSENT__",
            if request.uri().path() == "/mcp/consent" {
                "true"
            } else {
                "false"
            },
        );
    let policy = format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    );
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&policy).expect("base64 nonce is ASCII"),
            ),
        ],
        body,
    )
        .into_response()
}
