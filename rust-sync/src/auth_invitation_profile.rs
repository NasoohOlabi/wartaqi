use crate::{access, credentials, ledger, session_identity, writes::Error};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
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
        .route("/api/me/invitation-profile", any(update))
        .with_state(App {
            pool,
            origin,
            secret,
            secure,
        })
}
fn response(status: StatusCode, value: Value) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "application/json;charset=utf-8"),
        ],
        Json(value),
    )
        .into_response()
}
fn error(status: StatusCode, message: &str) -> Response {
    response(status, json!({"error":message}))
}
fn unavailable() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "Server error")
}
fn password(value: &Value) -> Option<&str> {
    let s = value.as_str()?;
    (8..=128).contains(&s.encode_utf16().count()).then_some(s)
}
fn contact(value: &Value) -> Option<&str> {
    let s = ledger::js_trim(value.as_str()?);
    if s.encode_utf16().count() > 40 || s.is_empty() {
        return None;
    }
    let body = s.strip_prefix('+').unwrap_or(s);
    if body.is_empty()
        || !body
            .bytes()
            .all(|c| c.is_ascii_digit() || b" ()-".contains(&c))
    {
        return None;
    }
    let digits = body.bytes().filter(u8::is_ascii_digit).count();
    (7..=15).contains(&digits).then_some(s)
}
struct Input<'a> {
    phone: Option<&'a str>,
    current: Option<&'a str>,
    new: Option<&'a str>,
}
fn input(value: &Value, setup: bool) -> Option<Input<'_>> {
    let obj = value.as_object()?;
    if setup {
        if obj.len() != 1 {
            return None;
        }
        Some(Input {
            phone: None,
            current: None,
            new: Some(password(obj.get("newPassword")?)?),
        })
    } else {
        if obj
            .keys()
            .any(|k| k != "phone" && k != "currentPassword" && k != "newPassword")
        {
            return None;
        }
        let current = obj.get("currentPassword")?.as_str()?;
        if current.is_empty() || current.encode_utf16().count() > 128 {
            return None;
        }
        let new = match obj.get("newPassword") {
            Some(v) => Some(password(v)?),
            None => None,
        };
        Some(Input {
            phone: Some(contact(obj.get("phone")?)?),
            current: Some(current),
            new,
        })
    }
}
async fn update(State(app): State<App>, request: Request) -> Response {
    let method = request.method().clone();
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .filter(|value| !value.is_empty());
    if (method != Method::GET && method != Method::HEAD && origin != Some(&app.origin))
        || origin.is_some_and(|value| value != app.origin)
    {
        return error(StatusCode::FORBIDDEN, "Forbidden");
    }
    let with_origin = origin.is_some();
    let mut response = match method {
        Method::POST => save(&app, request).await,
        Method::OPTIONS => (
            StatusCode::NO_CONTENT,
            [
                (header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS"),
                (
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    "Authorization, Content-Type",
                ),
                (header::CACHE_CONTROL, "no-store"),
            ],
        )
            .into_response(),
        _ => error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed"),
    };
    if with_origin {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, app.origin);
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}

async fn save(app: &App, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Some(token) = session_identity::token(&parts.headers, &app.secret, app.secure) else {
        return error(StatusCode::UNAUTHORIZED, "Unauthorized");
    };
    let Some(expected) = parts
        .headers
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
    else {
        return error(StatusCode::UNAUTHORIZED, "Unauthorized");
    };
    let body = to_bytes(body, 2 * 1024 * 1024).await;
    let Ok(mut db) = app.pool.get().await else {
        return unavailable();
    };
    let Ok(tx) = db.transaction().await else {
        return unavailable();
    };
    // Serializes account changes with resets, sign-out and other auth mutations.
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
    let session = match tx.query_opt("SELECT id::text, \"userId\"::text FROM neon_auth.session WHERE token = $1 AND \"expiresAt\" > statement_timestamp()", &[&token]).await { Ok(Some(row)) => row, Ok(None) => return error(StatusCode::UNAUTHORIZED, "Unauthorized"), Err(_) => return unavailable() };
    let session_id: String = session.get(0);
    let user_id: String = session.get(1);
    if user_id != expected {
        return error(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match access::load(&tx, &user_id).await {
        Ok(Some(_)) => (),
        Ok(None) => return error(StatusCode::FORBIDDEN, "Forbidden"),
        Err(_) => return unavailable(),
    }
    let value = body
        .map_err(|_| Error::TooLarge)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|_| Error::Invalid));
    let result = match save_with_access(&tx, &user_id, Some(&session_id), value).await {
        Ok(result) => result,
        Err(Error::Invalid) => return error(StatusCode::BAD_REQUEST, "Invalid request"),
        Err(Error::Forbidden) => return error(StatusCode::FORBIDDEN, "Forbidden"),
        Err(Error::TooLarge) => return error(StatusCode::PAYLOAD_TOO_LARGE, "Request too large"),
        Err(_) => return unavailable(),
    };
    if tx.commit().await.is_err() {
        return unavailable();
    }
    response(StatusCode::OK, result)
}

/// The caller holds the auth mutation and application account locks.
pub(crate) async fn save_with_access(
    tx: &tokio_postgres::Transaction<'_>,
    user_id: &str,
    session_id: Option<&str>,
    value: Result<Value, Error>,
) -> Result<Value, Error> {
    let account = tx.query_opt(
        "SELECT invitation_username IS NOT NULL OR invitation_request_id IS NOT NULL, invitation_requires_setup, invitation_phone FROM app_accounts WHERE auth_user_id = $1",
        &[&user_id],
    ).await?.ok_or(Error::Forbidden)?;
    let setup: bool = account.get(1);
    if !account.get::<_, bool>(0) || (setup && session_id.is_none()) {
        return Err(Error::Forbidden);
    }
    let previous_phone: Option<String> = account.get(2);
    let value = value?;
    let data = input(&value, setup).ok_or(Error::Invalid)?;
    let row = tx.query_opt(
        r#"SELECT password FROM neon_auth.account WHERE "userId"::text = $1 AND "providerId" = 'credential' LIMIT 1 FOR UPDATE"#,
        &[&user_id],
    ).await?.ok_or(Error::Invalid)?;
    let stored = row.get::<_, Option<String>>(0).ok_or(Error::Invalid)?;
    let current = data.current.map(str::to_owned);
    let new = data.new.map(str::to_owned);
    let hash = tokio::task::spawn_blocking(move || {
        if current
            .as_deref()
            .is_some_and(|p| !credentials::verify_password(p, &stored))
            || new
                .as_deref()
                .is_some_and(|p| credentials::verify_password(p, &stored))
        {
            return Err(Error::Invalid);
        }
        new.map(|password| {
            credentials::hash_password(&password).map_err(|error| Error::Other(error.into()))
        })
        .transpose()
    })
    .await
    .map_err(|error| Error::Other(error.into()))??;
    let phone = data.phone.or(previous_phone.as_deref());
    tx.execute("UPDATE app_accounts SET invitation_phone = $2, invitation_requires_setup = false WHERE auth_user_id = $1", &[&user_id, &phone]).await?;
    let rotated = hash.is_some();
    if let Some(hash) = hash {
        tx.execute(r#"UPDATE neon_auth.account SET password = $2, "updatedAt" = now() WHERE "userId"::text = $1 AND "providerId" = 'credential'"#, &[&user_id, &hash]).await?;
        tx.execute(r#"DELETE FROM neon_auth.session WHERE "userId"::text = $1 AND id IS DISTINCT FROM $2::text::uuid"#, &[&user_id, &session_id]).await?;
        let suffix = format!(":{user_id}");
        tx.execute("DELETE FROM neon_auth.verification WHERE starts_with(identifier, $1) AND right(identifier, length($2)) = $2", &[&"wartaq:telegram:", &suffix]).await?;
    }
    Ok(json!({"saved":true,"sessionRevoked":rotated && session_id.is_none()}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_and_profile_validate_separately() {
        assert!(input(&json!({"newPassword":"password-123"}), true).is_some());
        assert!(
            input(
                &json!({"newPassword":"password-123", "verifiedPhone":"+12025550101"}),
                true
            )
            .is_none()
        );
        assert!(
            input(
                &json!({"phone":" +1 (202) 555-0101 ","currentPassword":"old"}),
                false
            )
            .is_some()
        );
        assert!(
            input(
                &json!({"phone":"+1+2025550101","currentPassword":"old"}),
                false
            )
            .is_none()
        );
        assert!(
            input(
                &json!({"phone":"1234567","currentPassword":"old","newPassword":null}),
                false
            )
            .is_none()
        );
    }
}
