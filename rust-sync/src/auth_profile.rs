use crate::{auth_get_session, auth_sign_in, credentials, ledger, session_identity};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone)]
struct App {
    pool: deadpool_postgres::Pool,
    origin: HeaderValue,
    secret: Arc<str>,
    secure: bool,
}
pub(crate) fn router(
    pool: deadpool_postgres::Pool,
    origin: HeaderValue,
    secret: Arc<str>,
    secure: bool,
) -> Router {
    Router::new()
        .route("/api/auth/update-user", post(profile))
        .route("/api/auth/change-password", post(change_password))
        .route("/api/auth/set-password", post(set_password))
        .with_state(App {
            pool,
            origin,
            secret,
            secure,
        })
}
fn response(status: StatusCode, body: Value, cookies: &[String]) -> Response {
    let mut r = (status, Json(body)).into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    for c in cookies {
        if let Ok(v) = HeaderValue::from_str(c) {
            r.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    r
}
fn error(status: StatusCode, message: &str) -> Response {
    response(status, json!({"error":message}), &[])
}
fn auth_error(status: StatusCode, code: &str, message: &str) -> Response {
    response(status, json!({"code":code,"message":message}), &[])
}
fn unavailable() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "Server error")
}
fn origin(headers: &HeaderMap, app: &App) -> bool {
    headers.get(header::ORIGIN) == Some(&app.origin)
}
fn image_valid(image: &str) -> bool {
    if image.encode_utf16().count() > 120_000 {
        return false;
    }
    let Some((kind, encoded)) = image
        .strip_prefix("data:image/")
        .and_then(|s| s.split_once(";base64,"))
    else {
        return false;
    };
    if encoded.is_empty() || encoded.len() % 4 != 0 {
        return false;
    }
    let Ok(bytes) = STANDARD.decode(encoded) else {
        return false;
    };
    if STANDARD.encode(&bytes) != encoded {
        return false;
    }
    match kind {
        "png" => bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]),
        "jpeg" => {
            bytes.len() >= 4 && bytes.starts_with(&[255, 216, 255]) && bytes.ends_with(&[255, 217])
        }
        "webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}
pub(crate) fn profile_valid(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    if obj.keys().any(|k| k != "name" && k != "image") {
        return false;
    }
    let Some(name) = obj.get("name").and_then(Value::as_str) else {
        return false;
    };
    let name = ledger::js_trim(name);
    if name.is_empty() || name.encode_utf16().count() > 120 {
        return false;
    }
    match obj.get("image") {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => image_valid(s),
        _ => false,
    }
}
const USER: &str = r#"SELECT s."userId"::text, jsonb_build_object('id', u.id::text, 'name', u.name, 'email', u.email, 'emailVerified', u."emailVerified", 'image', u.image, 'createdAt', to_char(u."createdAt" AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'), 'updatedAt', to_char(u."updatedAt" AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"')) FROM neon_auth.session s JOIN neon_auth."user" u ON u.id = s."userId" WHERE s.token = $1 AND s."expiresAt" > statement_timestamp()"#;
async fn profile(State(app): State<App>, request: Request) -> Response {
    if !origin(request.headers(), &app) {
        return error(StatusCode::FORBIDDEN, "Forbidden");
    }
    let (parts, body) = request.into_parts();
    let Some(token) = session_identity::token(&parts.headers, &app.secret, app.secure) else {
        return error(StatusCode::UNAUTHORIZED, "Unauthorized");
    };
    let body_bytes = to_bytes(body, 121_024).await;
    let Ok(mut db) = app.pool.get().await else {
        return unavailable();
    };
    let Ok(tx) = db.transaction().await else {
        return unavailable();
    };
    if tx
        .execute(
            "SELECT pg_advisory_xact_lock(hashtextextended('auth-mutations', 0))",
            &[],
        )
        .await
        .is_err()
    {
        return unavailable();
    }
    let Ok(Some(row)) = tx.query_opt(USER, &[&token]).await else {
        return error(StatusCode::UNAUTHORIZED, "Unauthorized");
    };
    let user_id: String = row.get(0);
    if parts
        .headers
        .get("x-user-id")
        .is_none_or(|h| h.as_bytes() != user_id.as_bytes())
    {
        return error(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    let Ok(bytes) = body_bytes else {
        return error(StatusCode::BAD_REQUEST, "Invalid account data");
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "Invalid account data");
    };
    if !profile_valid(&value) {
        return error(StatusCode::BAD_REQUEST, "Invalid account data");
    }
    let name = value["name"].as_str().unwrap();
    let result = if value.get("image").is_some() {
        let image = value["image"].as_str();
        tx.execute("UPDATE neon_auth.\"user\" SET name = $1, image = $2, \"updatedAt\" = date_trunc('milliseconds', statement_timestamp()) WHERE id::text = $3", &[&name, &image, &user_id]).await
    } else {
        tx.execute("UPDATE neon_auth.\"user\" SET name = $1, \"updatedAt\" = date_trunc('milliseconds', statement_timestamp()) WHERE id::text = $2", &[&name, &user_id]).await
    };
    if result.is_err() || tx.commit().await.is_err() {
        return unavailable();
    }
    let remember = !auth_get_session::marker(&parts.headers, &app.secret, app.secure);
    response(
        StatusCode::OK,
        json!({"status":true}),
        &[auth_get_session::signed_cookie(
            "session_token",
            &token,
            &app.secret,
            app.secure,
            remember,
        )],
    )
}
async fn password_route(app: App, request: Request, set: bool) -> Response {
    if !origin(request.headers(), &app) {
        return error(StatusCode::FORBIDDEN, "Forbidden");
    }
    let (parts, body) = request.into_parts();
    let token = session_identity::token(&parts.headers, &app.secret, app.secure);
    let body_bytes = if set {
        None
    } else {
        Some(to_bytes(body, 8192).await)
    };
    let Ok(mut db) = app.pool.get().await else {
        return unavailable();
    };
    let Ok(tx) = db.transaction().await else {
        return unavailable();
    };
    if tx
        .execute(
            "SELECT pg_advisory_xact_lock(hashtextextended('auth-mutations', 0))",
            &[],
        )
        .await
        .is_err()
    {
        return unavailable();
    }
    let row = if let Some(token) = &token {
        match tx.query_opt(USER, &[token]).await {
            Ok(row) => row,
            Err(_) => return unavailable(),
        }
    } else {
        None
    };
    if let Some(row) = &row {
        let id: String = row.get(0);
        match tx.query_opt("SELECT 1 FROM app_accounts WHERE auth_user_id = $1 AND (invitation_username IS NOT NULL OR invitation_request_id IS NOT NULL)", &[&id]).await {
            Ok(Some(_)) => return error(StatusCode::FORBIDDEN, "Forbidden"), Ok(None) => (), Err(_) => return unavailable(),
        }
    }
    // Better Auth's setPassword endpoint is server-only and has no browser route.
    if set {
        return error(StatusCode::NOT_FOUND, "Not found");
    }
    let Some(row) = row else {
        return auth_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "Unauthorized");
    };
    let user_id: String = row.get(0);
    let user: Value = row.get(1);
    let Some(Ok(bytes)) = body_bytes else {
        return error(StatusCode::BAD_REQUEST, "Invalid request");
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "Invalid request");
    };
    let Some(new_password) = value.get("newPassword").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "Invalid request");
    };
    let Some(current_password) = value.get("currentPassword").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "Invalid request");
    };
    let revoke = match value.get("revokeOtherSessions") {
        None => false,
        Some(Value::Bool(b)) => *b,
        _ => return error(StatusCode::BAD_REQUEST, "Invalid request"),
    };
    if new_password.encode_utf16().count() < 8 {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "PASSWORD_TOO_SHORT",
            "Password too short",
        );
    }
    if new_password.encode_utf16().count() > 128 || current_password.encode_utf16().count() > 128 {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "PASSWORD_TOO_LONG",
            "Password too long",
        );
    }
    let Ok(account) = tx.query_opt("SELECT id::text, password FROM neon_auth.account WHERE \"userId\"::text = $1 AND \"providerId\" = 'credential' AND \"accountId\" = $1 LIMIT 1", &[&user_id]).await else { return unavailable() };
    let Some(account) = account else {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "CREDENTIAL_ACCOUNT_NOT_FOUND",
            "Credential account not found",
        );
    };
    let id: String = account.get(0);
    let stored: Option<String> = account.get(1);
    let Some(stored) = stored else {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "CREDENTIAL_ACCOUNT_NOT_FOUND",
            "Credential account not found",
        );
    };
    let current = current_password.to_owned();
    let new = new_password.to_owned();
    let Ok(Ok(hash)) = tokio::task::spawn_blocking(move || {
        if !credentials::verify_password(&current, &stored) {
            return Ok(None);
        }
        credentials::hash_password(&new).map(Some)
    })
    .await
    else {
        return unavailable();
    };
    let Some(hash) = hash else {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PASSWORD",
            "Invalid password",
        );
    };
    if tx.execute("UPDATE neon_auth.account SET password = $1, \"updatedAt\" = date_trunc('milliseconds', statement_timestamp()) WHERE id::text = $2", &[&hash, &id]).await.is_err() { return unavailable() }
    let mut cookies = Vec::new();
    let mut result_token = None;
    if revoke {
        let Ok(new_token) = auth_sign_in::session_token() else {
            return unavailable();
        };
        if tx
            .execute(
                "DELETE FROM neon_auth.session WHERE \"userId\"::text = $1",
                &[&user_id],
            )
            .await
            .is_err()
        {
            return unavailable();
        }
        let agent = parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        let ip = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(str::trim)
            .unwrap_or("");
        if tx.execute("INSERT INTO neon_auth.session (id, \"userId\", token, \"expiresAt\", \"createdAt\", \"updatedAt\", \"ipAddress\", \"userAgent\") VALUES (gen_random_uuid(), $1::text::uuid, $2, date_trunc('milliseconds', statement_timestamp()) + interval '7 days', date_trunc('milliseconds', statement_timestamp()), date_trunc('milliseconds', statement_timestamp()), $3, $4)", &[&user_id, &new_token, &ip, &agent]).await.is_err() { return unavailable() }
        let remember = !auth_get_session::marker(&parts.headers, &app.secret, app.secure);
        cookies.push(auth_get_session::signed_cookie(
            "session_token",
            &new_token,
            &app.secret,
            app.secure,
            remember,
        ));
        result_token = Some(new_token);
    }
    if tx.commit().await.is_err() {
        return unavailable();
    }
    response(
        StatusCode::OK,
        json!({"token":result_token,"user":user}),
        &cookies,
    )
}
async fn change_password(State(app): State<App>, request: Request) -> Response {
    password_route(app, request, false).await
}
async fn set_password(State(app): State<App>, request: Request) -> Response {
    password_route(app, request, true).await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_profile_images_and_fields() {
        assert!(profile_valid(
            &json!({"name":" user ","image":"data:image/png;base64,iVBORw0KGgo="})
        ));
        assert!(profile_valid(&json!({"name":"user","image":null})));
        assert!(!profile_valid(&json!({"name":" ","image":null})));
        assert!(!profile_valid(
            &json!({"name":"user","role":"administrator"})
        ));
        assert!(!profile_valid(
            &json!({"name":"user","image":"data:image/png;base64,SGVsbG8="})
        ));
    }
}
