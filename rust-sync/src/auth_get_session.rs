use axum::{
    Json,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ring::hmac;
use serde_json::{Value, json};

use crate::session_identity;

pub(crate) fn cookie_name(secure: bool, suffix: &str) -> String {
    format!(
        "{}better-auth.{suffix}",
        if secure { "__Secure-" } else { "" }
    )
}

pub(crate) fn marker(headers: &HeaderMap, secret: &str, secure: bool) -> bool {
    let name = cookie_name(secure, "dont_remember");
    let session_name = cookie_name(secure, "session_token");
    let value = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .filter_map(|part| part.split_once('='))
        .find_map(|(key, value)| (key.trim() == name).then_some(value.trim()));
    let Some(value) = value else { return false };
    let Ok(cookie) = HeaderValue::from_str(&format!("{session_name}={value}")) else {
        return false;
    };
    let mut mapped = HeaderMap::new();
    mapped.insert(header::COOKIE, cookie);
    session_identity::token(&mapped, secret, secure).is_some()
}

fn response(status: StatusCode, body: Value, cookies: &[String]) -> Response {
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

pub(crate) fn expired_cookies(secure: bool) -> Vec<String> {
    ["session_token", "session_data", "dont_remember"]
        .iter()
        .map(|name| {
            format!(
                "{}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax{}",
                cookie_name(secure, name),
                if secure { "; Secure" } else { "" }
            )
        })
        .collect()
}

fn renewed_cookie(token: &str, secret: &str, secure: bool) -> String {
    signed_cookie("session_token", token, secret, secure, true)
}

pub(crate) fn signed_cookie(
    suffix: &str,
    token: &str,
    secret: &str,
    secure: bool,
    remember: bool,
) -> String {
    let signature = STANDARD.encode(
        hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes()),
            token.as_bytes(),
        )
        .as_ref(),
    );
    let value: String = format!("{token}.{signature}")
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let age = if remember { "; Max-Age=604800" } else { "" };
    format!(
        "{}={value}{age}; Path=/; HttpOnly; SameSite=Lax{}",
        cookie_name(secure, suffix),
        if secure { "; Secure" } else { "" }
    )
}

// Uses PostgreSQL to format timestamps as JavaScript Date.toJSON() does.
const SESSION: &str = r#"
SELECT jsonb_build_object(
  'session', jsonb_build_object('id', s.id::text, 'expiresAt', to_char(s."expiresAt" AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'),
    'token', s.token, 'createdAt', to_char(s."createdAt" AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'),
    'updatedAt', to_char(s."updatedAt" AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'),
    'ipAddress', s."ipAddress", 'userAgent', s."userAgent", 'userId', s."userId"::text),
  'user', jsonb_build_object('id', u.id::text, 'name', u.name, 'email', u.email, 'emailVerified', u."emailVerified", 'image', u.image,
    'createdAt', to_char(u."createdAt" AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'),
    'updatedAt', to_char(u."updatedAt" AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'))),
  s."expiresAt" < statement_timestamp(), s."expiresAt" - interval '6 days' <= statement_timestamp()
FROM neon_auth.session s JOIN neon_auth."user" u ON u.id = s."userId" WHERE s.token = $1
"#;

pub(crate) async fn get_session(
    db: &tokio_postgres::Client,
    headers: &HeaderMap,
    secret: &str,
    secure: bool,
    disable_refresh: bool,
) -> Response {
    let Some(token) = session_identity::token(headers, secret, secure) else {
        return response(StatusCode::OK, Value::Null, &[]);
    };
    match get_session_inner(db, headers, secret, secure, disable_refresh, &token).await {
        Ok(result) => result,
        Err(_) => response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"code":"FAILED_TO_GET_SESSION","message":"Failed to get session"}),
            &[],
        ),
    }
}

async fn get_session_inner(
    db: &tokio_postgres::Client,
    headers: &HeaderMap,
    secret: &str,
    secure: bool,
    disable_refresh: bool,
    token: &str,
) -> Result<Response, tokio_postgres::Error> {
    let Some(row) = db.query_opt(SESSION, &[&token]).await? else {
        return Ok(response(
            StatusCode::OK,
            Value::Null,
            &expired_cookies(secure),
        ));
    };
    if row.get::<_, bool>(1) {
        db.execute("DELETE FROM neon_auth.session WHERE token = $1", &[&token])
            .await?;
        return Ok(response(
            StatusCode::OK,
            Value::Null,
            &expired_cookies(secure),
        ));
    }
    if row.get::<_, bool>(2) && !disable_refresh && !marker(headers, secret, secure) {
        let updated = db.execute("UPDATE neon_auth.session SET \"expiresAt\" = date_trunc('milliseconds', statement_timestamp()) + interval '7 days', \"updatedAt\" = date_trunc('milliseconds', statement_timestamp()) WHERE token = $1", &[&token]).await?;
        if updated == 0 {
            return Ok(response(
                StatusCode::UNAUTHORIZED,
                json!({"code":"FAILED_TO_GET_SESSION","message":"Failed to get session"}),
                &expired_cookies(secure),
            ));
        }
        if let Some(row) = db.query_opt(SESSION, &[&token]).await? {
            return Ok(response(
                StatusCode::OK,
                row.get(0),
                &[renewed_cookie(token, secret, secure)],
            ));
        }
        return Ok(response(
            StatusCode::UNAUTHORIZED,
            json!({"code":"FAILED_TO_GET_SESSION","message":"Failed to get session"}),
            &expired_cookies(secure),
        ));
    }
    Ok(response(StatusCode::OK, row.get(0), &[]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_cookie_round_trips_and_marker_is_verified() {
        let secret = "synthetic-secret-for-session-fixture";
        let cookie = renewed_cookie("fixture-token", secret, false);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(cookie.split(';').next().unwrap()).unwrap(),
        );
        assert_eq!(
            session_identity::token(&headers, secret, false).as_deref(),
            Some("fixture-token")
        );
        assert!(!marker(&headers, secret, false));
        let marker_cookie =
            renewed_cookie("true", secret, false).replace("session_token", "dont_remember");
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(marker_cookie.split(';').next().unwrap()).unwrap(),
        );
        assert!(marker(&headers, secret, false));
        assert!(!marker(&headers, "wrong", false));
    }
}
