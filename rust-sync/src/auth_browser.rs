use crate::{
    auth_get_session,
    auth_sign_in::{self, SignInOutcome},
    device_sessions::{self, Account, SwitchOutcome},
    session_identity,
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
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
        .route("/api/auth/sign-in/phone", post(sign_in))
        .route("/api/device-accounts", get(list))
        .route("/api/device-accounts/switch", post(switch))
        .route("/api/auth/sign-out", post(sign_out))
        .with_state(App {
            pool,
            origin,
            secret,
            secure,
        })
}

fn response(status: StatusCode, body: Value, cookies: &[String]) -> Response {
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json;charset=utf-8"),
    );
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}
fn err(status: StatusCode, message: &str) -> Response {
    response(status, json!({"error":message}), &[])
}
fn allowed(headers: &HeaderMap, app: &App) -> bool {
    headers.get(header::ORIGIN) == Some(&app.origin)
}
fn unavailable() -> Response {
    err(StatusCode::INTERNAL_SERVER_ERROR, "Server error")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Login {
    phone: String,
    password: String,
    remember_me: Option<bool>,
    #[serde(rename = "oauth_query")]
    oauth_query: Option<String>,
}
fn phone(value: &str) -> Option<String> {
    let input = value.trim();
    if input.encode_utf16().count() > 40 {
        return None;
    }
    let mut compact = String::new();
    for c in input.chars() {
        match c {
            '\u{0660}'..='\u{0669}' => compact.push(char::from(b'0' + (c as u32 - 0x660) as u8)),
            '\u{06f0}'..='\u{06f9}' => compact.push(char::from(b'0' + (c as u32 - 0x6f0) as u8)),
            ' ' | '(' | ')' | '-' => (),
            _ => compact.push(c),
        }
    }
    if compact.starts_with("00") {
        compact = format!("+{}", &compact[2..]);
    } else if compact.len() == 10
        && compact.starts_with("09")
        && compact.bytes().all(|b| b.is_ascii_digit())
    {
        compact = format!("+963{}", &compact[1..]);
    }
    let bytes = compact.as_bytes();
    (bytes.len() >= 8
        && bytes.len() <= 16
        && bytes[0] == b'+'
        && (b'1'..=b'9').contains(&bytes[1])
        && bytes[2..].iter().all(u8::is_ascii_digit))
    .then_some(compact)
}
async fn accounts(
    db: &tokio_postgres::Client,
    headers: &HeaderMap,
    app: &App,
) -> Result<(Option<Account>, Vec<Account>), tokio_postgres::Error> {
    let active = device_sessions::active(db, headers, &app.secret, app.secure).await?;
    let all = device_sessions::list(db, headers, &app.secret, app.secure, active.as_ref()).await?;
    Ok((active, all))
}
async fn list(State(app): State<App>, headers: HeaderMap) -> Response {
    let Ok(db) = app.pool.get().await else {
        return unavailable();
    };
    let Ok((_, all)) = accounts(&db, &headers, &app).await else {
        return unavailable();
    };
    response(
        StatusCode::OK,
        json!({"accounts": all.iter().map(|a| json!({"userId":a.user_id,"name":a.name,"identifier":a.identifier})).collect::<Vec<_>>()}),
        &[],
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Selection {
    user_id: String,
}
async fn switch(State(app): State<App>, request: Request) -> Response {
    if !allowed(request.headers(), &app) {
        return err(StatusCode::FORBIDDEN, "Forbidden");
    }
    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, 4096).await else {
        return err(StatusCode::BAD_REQUEST, "Invalid account");
    };
    let Ok(input) = serde_json::from_slice::<Selection>(&bytes) else {
        return err(StatusCode::BAD_REQUEST, "Invalid account");
    };
    if input.user_id.is_empty() || input.user_id.encode_utf16().count() > 128 {
        return err(StatusCode::BAD_REQUEST, "Invalid account");
    }
    let Ok(db) = app.pool.get().await else {
        return unavailable();
    };
    let Ok((active, all)) = accounts(&db, &parts.headers, &app).await else {
        return unavailable();
    };
    match device_sessions::switch(
        &parts.headers,
        active.as_ref(),
        &all,
        &input.user_id,
        app.secure,
    ) {
        SwitchOutcome::Selected(cookies) => response(StatusCode::OK, json!({"ok":true}), &cookies),
        SwitchOutcome::Expired => err(StatusCode::NOT_FOUND, "Account session expired"),
        SwitchOutcome::LimitReached => err(StatusCode::CONFLICT, "Device account limit reached"),
    }
}
async fn sign_in(State(app): State<App>, request: Request) -> Response {
    if !allowed(request.headers(), &app) {
        return err(StatusCode::FORBIDDEN, "Forbidden");
    }
    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, 8192).await else {
        return err(StatusCode::BAD_REQUEST, "Invalid credentials");
    };
    let Ok(input) = serde_json::from_slice::<Login>(&bytes) else {
        return err(StatusCode::BAD_REQUEST, "Invalid credentials");
    };
    let Some(phone) = phone(&input.phone) else {
        return err(StatusCode::BAD_REQUEST, "Invalid credentials");
    };
    if input.password.is_empty()
        || input.password.encode_utf16().count() > 128
        || input
            .oauth_query
            .as_ref()
            .is_some_and(|q| q.encode_utf16().count() > 6000)
    {
        return err(StatusCode::BAD_REQUEST, "Invalid credentials");
    }
    // OAuth query verification and continuation require the separate OAuth port.
    if input.oauth_query.is_some() {
        return err(
            StatusCode::NOT_IMPLEMENTED,
            "OAuth sign-in is not available locally",
        );
    }
    let Ok(mut db) = app.pool.get().await else {
        return unavailable();
    };
    let Ok((active, before)) = accounts(&db, &parts.headers, &app).await else {
        return unavailable();
    };
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
    let Ok(result) = auth_sign_in::sign_in(
        &mut db,
        &phone,
        &input.password,
        input.remember_me.unwrap_or(true),
        ip,
        agent,
    )
    .await
    else {
        return unavailable();
    };
    let SignInOutcome::SignedIn {
        token,
        user,
        remember,
        ..
    } = result
    else {
        return response(
            StatusCode::UNAUTHORIZED,
            json!({"code":"INVALID_EMAIL_OR_PASSWORD","message":"Invalid email or password"}),
            &[],
        );
    };
    let session_cookie =
        auth_get_session::signed_cookie("session_token", &token, &app.secret, app.secure, remember);
    let marker_cookie =
        auth_get_session::signed_cookie("dont_remember", "true", &app.secret, app.secure, false);
    let value = session_cookie
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1;
    let marker = (!remember).then(|| {
        marker_cookie
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1
    });
    let Ok(Some(current)) =
        device_sessions::remembered(&db, value, marker, &app.secret, app.secure).await
    else {
        return unavailable();
    };
    let mut all = vec![current.clone()];
    all.extend(before.into_iter().filter(|a| a.user_id != current.user_id));
    if all.len() > device_sessions::SLOT_COUNT {
        if db
            .execute("DELETE FROM neon_auth.session WHERE token = $1", &[&token])
            .await
            .is_err()
        {
            return unavailable();
        }
        return err(StatusCode::CONFLICT, "Device account limit reached");
    }
    let mut cookies = vec![session_cookie];
    if !remember {
        cookies.push(marker_cookie);
    } else if auth_get_session::marker(&parts.headers, &app.secret, app.secure) {
        cookies.push(device_sessions::remove_cookie(
            &auth_get_session::cookie_name(app.secure, "dont_remember"),
            app.secure,
        ));
    }
    if let Some(active) = active.filter(|a| a.user_id != current.user_id) {
        cookies.push(device_sessions::save_cookie(&active, app.secure));
    }
    cookies.push(device_sessions::save_cookie(&current, app.secure));
    for (name, _) in device_sessions::slot_cookies(&parts.headers, app.secure) {
        if !all
            .iter()
            .any(|a| device_sessions::slot_name(&a.user_id, app.secure) == name)
        {
            cookies.push(device_sessions::remove_cookie(&name, app.secure));
        }
    }
    response(
        StatusCode::OK,
        json!({"redirect":false,"token":token,"user":user}),
        &cookies,
    )
}
async fn sign_out(State(app): State<App>, request: Request) -> Response {
    if !allowed(request.headers(), &app) {
        return err(StatusCode::FORBIDDEN, "Forbidden");
    }
    let headers = request.headers();
    let Ok(mut db) = app.pool.get().await else {
        return unavailable();
    };
    let Ok(active) = device_sessions::active(&db, headers, &app.secret, app.secure).await else {
        return unavailable();
    };
    if let Some(expected) = headers.get("x-user-id")
        && active
            .as_ref()
            .is_none_or(|a| expected.as_bytes() != a.user_id.as_bytes())
    {
        return err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
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
    if let Some(token) = session_identity::token(headers, &app.secret, app.secure)
        && tx
            .execute("DELETE FROM neon_auth.session WHERE token = $1", &[&token])
            .await
            .is_err()
    {
        return unavailable();
    }
    if tx.commit().await.is_err() {
        return unavailable();
    }
    let mut cookies = auth_get_session::expired_cookies(app.secure);
    if let Some(active) = active {
        cookies.push(device_sessions::remove_cookie(
            &device_sessions::slot_name(&active.user_id, app.secure),
            app.secure,
        ));
    }
    response(StatusCode::OK, json!({"success":true}), &cookies)
}
#[cfg(test)]
mod tests {
    use super::phone;
    #[test]
    fn normalizes_phones() {
        assert_eq!(phone(" 0912 345-678 ").as_deref(), Some("+963912345678"));
        assert_eq!(phone("\u{06f0}\u{06f0}\u{06f9}\u{06f6}\u{06f3}\u{06f9}\u{06f1}\u{06f2}\u{06f3}\u{06f4}\u{06f5}\u{06f6}\u{06f7}\u{06f8}").as_deref(), Some("+963912345678"));
        assert_eq!(phone("+01234567"), None);
    }
}
