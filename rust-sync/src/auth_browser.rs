use crate::{
    auth_get_session,
    auth_sign_in::{self, SignInOutcome},
    device_sessions::{self, Account, SwitchOutcome},
    ledger::js_trim,
    session_identity,
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ring::hmac;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::form_urlencoded;

#[derive(Clone)]
struct App {
    pool: deadpool_postgres::Pool,
    origin: HeaderValue,
    secret: Arc<str>,
    secure: bool,
    rate_limit: Arc<Mutex<RateLimit>>,
    rate_limit_enabled: bool,
    use_loopback_ip: bool,
}

pub(crate) fn router(
    pool: deadpool_postgres::Pool,
    origin: HeaderValue,
    secret: Arc<str>,
    secure: bool,
) -> Router {
    let node_env = std::env::var("NODE_ENV");
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
            rate_limit: Arc::new(Mutex::new(RateLimit::default())),
            rate_limit_enabled: node_env.as_deref() == Ok("production"),
            use_loopback_ip: matches!(node_env.as_deref(), Ok("dev" | "development" | "test"))
                || std::env::var("TEST").is_ok_and(|value| !value.is_empty() && value != "false"),
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
    let mut origins = headers.get_all(header::ORIGIN).iter();
    origins.next() == Some(&app.origin) && origins.next().is_none()
}
fn unavailable() -> Response {
    err(StatusCode::INTERNAL_SERVER_ERROR, "Server error")
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum AuthRoute {
    SignIn,
    SignOut,
}

// Better Auth counts per route and IP in a rolling inactivity window.
#[derive(Default)]
struct RateLimit {
    entries: HashMap<(AuthRoute, Option<String>), (u8, Instant, u64)>,
    next_order: u64,
}
impl RateLimit {
    fn consume(&mut self, route: AuthRoute, ip: Option<String>, now: Instant) -> Option<u64> {
        const WINDOW: Duration = Duration::from_secs(10);
        let max = match route {
            AuthRoute::SignIn => 3, // The legacy handler rewrites phone sign-in to /sign-in/email.
            AuthRoute::SignOut => 100,
        };
        self.entries
            .retain(|_, (_, last, _)| now.duration_since(*last) < WINDOW);
        if self.entries.len() > 100_000
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, order))| *order)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest);
        }
        let key = (route, ip);
        if let Some((count, last, _)) = self.entries.get_mut(&key) {
            if *count >= max {
                let remaining = WINDOW - now.duration_since(*last);
                return Some(remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0));
            }
            *count += 1;
            *last = now;
        } else {
            self.entries.insert(key, (1, now, self.next_order));
            self.next_order = self.next_order.wrapping_add(1);
        }
        None
    }
}
fn client_ip(headers: &HeaderMap) -> Option<String> {
    // Without configured trusted proxies Better Auth rejects multi-hop headers.
    let mut ip = None;
    for value in headers.get_all("x-forwarded-for") {
        for address in value.to_str().ok()?.split(',').map(js_trim) {
            if !address.is_empty() {
                if ip.is_some() {
                    return None;
                }
                ip = Some(address.parse::<IpAddr>().ok()?);
            }
        }
    }
    match ip? {
        IpAddr::V4(ip) => Some(ip.to_string()),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => Some(ip.to_string()),
            None => {
                let segments = ip.segments();
                Some(format!(
                    "{:04x}:{:04x}:{:04x}:{:04x}:0000:0000:0000:0000",
                    segments[0], segments[1], segments[2], segments[3]
                ))
            }
        },
    }
}
fn request_ip(app: &App, headers: &HeaderMap) -> Option<String> {
    client_ip(headers).or_else(|| app.use_loopback_ip.then(|| "127.0.0.1".to_owned()))
}

fn throttled(app: &App, route: AuthRoute, ip: Option<String>) -> Option<Response> {
    if !app.rate_limit_enabled {
        return None;
    }
    let retry = app
        .rate_limit
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .consume(route, ip, Instant::now())?;
    let mut result = response(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"message":"Too many requests. Please try again later."}),
        &[],
    );
    result.headers_mut().insert(
        "x-retry-after",
        HeaderValue::from_str(&retry.to_string()).unwrap(),
    );
    result.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Some(result)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Login {
    phone: String,
    password: String,
    #[serde(default, deserialize_with = "present")]
    remember_me: Option<bool>,
    #[serde(default, rename = "oauth_query", deserialize_with = "present")]
    oauth_query: Option<String>,
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

pub(crate) fn phone(value: &str) -> Option<String> {
    let input = js_trim(value);
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

#[derive(Clone)]
pub(crate) struct OAuthContinuation {
    pub(crate) uri: Uri,
    pub(crate) cookie: HeaderValue,
}

struct SignedOAuthQuery(Vec<(String, String)>);

pub(crate) fn verify_oauth_query(input: &str, secret: &str) -> bool {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    SignedOAuthQuery::verify(input, secret, now_ms).is_some()
}

impl SignedOAuthQuery {
    fn verify(input: &str, secret: &str, now_ms: u128) -> Option<Self> {
        let query: Vec<_> =
            form_urlencoded::parse(input.strip_prefix('?').unwrap_or(input).as_bytes())
                .into_owned()
                .collect();
        let mut signatures = query.iter().filter(|(key, _)| key == "sig");
        let signature = STANDARD.decode(&signatures.next()?.1).ok()?;
        if signatures.next().is_some() {
            return None;
        }
        let expires = query
            .iter()
            .find(|(key, _)| key == "exp")?
            .1
            .parse::<u64>()
            .ok()?;
        if u128::from(expires) * 1000 < now_ms {
            return None;
        }
        let mut canonical: Vec<_> = query.iter().filter(|(key, _)| key != "sig").collect();
        canonical.sort_by(|(key_a, value_a), (key_b, value_b)| {
            key_a
                .encode_utf16()
                .cmp(key_b.encode_utf16())
                .then_with(|| value_a.encode_utf16().cmp(value_b.encode_utf16()))
        });
        let canonical = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(canonical.iter().map(|(key, value)| (key, value)))
            .finish();
        hmac::verify(
            &hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes()),
            canonical.as_bytes(),
            &signature,
        )
        .ok()?;
        Some(Self(query))
    }

    fn continue_with(self, created_at_ms: i64, cookie: HeaderValue) -> OAuthContinuation {
        let fresh = self
            .0
            .iter()
            .find(|(key, _)| key == "ba_iat")
            .and_then(|(_, value)| value.parse::<i64>().ok())
            .is_some_and(|time| time > 0 && created_at_ms >= time);
        let mut query = form_urlencoded::Serializer::new(String::new());
        for (key, value) in self.0 {
            match key.as_str() {
                "sig" | "exp" | "ba_iat" | "ba_pl" => continue,
                "max_age" if fresh => continue,
                "prompt" => {
                    let mut prompts: Vec<_> = value.split(' ').collect();
                    if let Some(index) = prompts.iter().position(|prompt| *prompt == "login") {
                        prompts.remove(index);
                    }
                    if !prompts.is_empty() {
                        query.append_pair(&key, &prompts.join(" "));
                    }
                }
                _ => {
                    query.append_pair(&key, &value);
                }
            }
        }
        let uri = format!("/api/auth/oauth2/authorize?{}", query.finish())
            .parse()
            .expect("form encoding emits a valid HTTP query");
        OAuthContinuation { uri, cookie }
    }
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
    let ip = request_ip(&app, &parts.headers);
    if let Some(response) = throttled(&app, AuthRoute::SignIn, ip.clone()) {
        return response;
    }
    let oauth = match input
        .oauth_query
        .as_deref()
        .filter(|query| !query.is_empty())
    {
        Some(query) => {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let Some(query) = SignedOAuthQuery::verify(query, &app.secret, now_ms) else {
                return response(
                    StatusCode::BAD_REQUEST,
                    json!({"error":"invalid_signature"}),
                    &[],
                );
            };
            Some(query)
        }
        None => None,
    };
    let Ok(mut db) = app.pool.get().await else {
        return unavailable();
    };
    let Ok((active, before)) = accounts(&db, &parts.headers, &app).await else {
        return unavailable();
    };
    let remember = input.remember_me.unwrap_or(true);
    let agent = parts
        .headers
        .get(header::USER_AGENT)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let Ok(result) = auth_sign_in::sign_in(
        &mut db,
        &phone,
        &input.password,
        remember,
        ip.as_deref().unwrap_or_default(),
        agent,
        &before,
    )
    .await
    else {
        return unavailable();
    };
    let session = match result {
        SignInOutcome::SignedIn(session) => session,
        SignInOutcome::InvalidCredentials => {
            return response(
                StatusCode::UNAUTHORIZED,
                json!({"code":"INVALID_EMAIL_OR_PASSWORD","message":"Invalid email or password"}),
                &[],
            );
        }
        SignInOutcome::DeviceLimitReached => {
            return err(StatusCode::CONFLICT, "Device account limit reached");
        }
    };
    let session_cookie = auth_get_session::signed_cookie(
        "session_token",
        &session.token,
        &app.secret,
        app.secure,
        remember,
    );
    let marker_cookie = (!remember).then(|| {
        auth_get_session::signed_cookie("dont_remember", "true", &app.secret, app.secure, false)
    });
    let pair = cookie_pair(&session_cookie);
    let continuation = oauth.map(|query| {
        query.continue_with(
            session.created_at_ms,
            continuation_cookie(pair, marker_cookie.as_deref()),
        )
    });
    let current = Account {
        value: cookie_pair_value(pair).to_owned(),
        marker: marker_cookie
            .as_deref()
            .map(cookie_pair)
            .map(cookie_pair_value)
            .map(str::to_owned),
        user_id: session.user_id,
        name: session.name,
        identifier: phone,
        expires_http: session.expires_http,
    };
    let slots = device_sessions::slot_cookies(&parts.headers, app.secure);
    let mut cookies = vec![session_cookie];
    if let Some(marker_cookie) = marker_cookie {
        cookies.push(marker_cookie);
    } else if auth_get_session::marker(&parts.headers, &app.secret, app.secure) {
        cookies.push(device_sessions::remove_cookie(
            &auth_get_session::cookie_name(app.secure, "dont_remember"),
            app.secure,
        ));
    }
    if let Some(active) = active.filter(|a| a.user_id != current.user_id) {
        let name = device_sessions::slot_name(&active.user_id, app.secure);
        let value = format!(
            "{}~{}",
            active.marker.as_deref().unwrap_or_default(),
            active.value
        );
        if !slots.iter().any(|slot| slot.0 == name && slot.1 == value) {
            cookies.push(device_sessions::save_cookie(&active, app.secure));
        }
    }
    cookies.push(device_sessions::save_cookie(&current, app.secure));
    for (name, _) in slots {
        if !before
            .iter()
            .chain(std::iter::once(&current))
            .any(|a| device_sessions::slot_name(&a.user_id, app.secure) == name)
        {
            cookies.push(device_sessions::remove_cookie(&name, app.secure));
        }
    }
    let mut response = response(
        StatusCode::OK,
        json!({"redirect":false,"token":session.token,"user":session.user}),
        &cookies,
    );
    if let Some(continuation) = continuation {
        response.extensions_mut().insert(continuation);
    }
    response
}

fn cookie_pair(cookie: &str) -> &str {
    cookie
        .split(';')
        .next()
        .expect("signed cookie includes exactly one name/value pair")
}

fn cookie_pair_value(pair: &str) -> &str {
    pair.split_once('=')
        .expect("signed cookie includes a value")
        .1
}

fn continuation_cookie(session_pair: &str, marker: Option<&str>) -> HeaderValue {
    // The provider independently looks up the session on continuation. Without
    // its new marker it may renew a short-lived session as a remembered one.
    let value = marker.map_or_else(
        || session_pair.to_owned(),
        |marker| format!("{session_pair}; {}", cookie_pair(marker)),
    );
    let mut cookie = HeaderValue::from_str(&value).expect("signed session cookies are ASCII");
    cookie.set_sensitive(true);
    cookie
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
    let mut expected_users = headers.get_all("x-user-id").iter();
    if let Some(expected) = expected_users.next()
        && (expected_users.next().is_some()
            || active
                .as_ref()
                .is_none_or(|a| expected.as_bytes() != a.user_id.as_bytes()))
    {
        return err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    if let Some(response) = throttled(&app, AuthRoute::SignOut, request_ip(&app, headers)) {
        return response;
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
    use super::*;

    #[test]
    fn production_limit_and_ip_matching() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("2001:db8::1"));
        assert_eq!(
            client_ip(&headers).as_deref(),
            Some("2001:0db8:0000:0000:0000:0000:0000:0000")
        );
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("192.0.2.1, 192.0.2.2"),
        );
        assert_eq!(client_ip(&headers), None);
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("::ffff:192.0.2.1"),
        );
        assert_eq!(client_ip(&headers).as_deref(), Some("192.0.2.1"));
        headers.append("x-forwarded-for", HeaderValue::from_static("192.0.2.2"));
        assert_eq!(client_ip(&headers), None);
        let mut limit = RateLimit::default();
        let now = Instant::now();
        for seconds in [0, 1, 2] {
            assert_eq!(
                limit.consume(AuthRoute::SignIn, None, now + Duration::from_secs(seconds)),
                None
            );
        }
        assert_eq!(
            limit.consume(AuthRoute::SignIn, None, now + Duration::from_secs(3)),
            Some(9)
        );
        assert_eq!(
            limit.consume(
                AuthRoute::SignIn,
                Some("192.0.2.2".into()),
                now + Duration::from_secs(3)
            ),
            None
        );
        assert_eq!(
            limit.consume(AuthRoute::SignOut, None, now + Duration::from_secs(3)),
            None
        );
        assert_eq!(
            limit.consume(
                AuthRoute::SignIn,
                None,
                now + Duration::from_secs(12) - Duration::from_nanos(1)
            ),
            Some(1)
        );
        assert_eq!(
            limit.consume(AuthRoute::SignIn, None, now + Duration::from_secs(12)),
            None
        );
    }

    #[test]
    fn oauth_continuation_requires_a_valid_signature_and_uses_only_the_new_session() {
        const SECRET: &str = "synthetic-secret-for-session-fixture";
        // Generated by URLSearchParams and Web Crypto's HMAC-SHA256 format.
        const SIGNED: &str = concat!(
            "state=%CE%A9%F0%9F%98%80+%2B&redirect_uri=http%3A%2F%2Flocalhost%3A8000%2Fcallback%23a&",
            "resource=https%3A%2F%2Fexample.test%2Fsecond&ba_iat=1999999999500&ba_pl=fixture&client_id=fixture&",
            "exp=2000000000&prompt=login+consent&max_age=0&resource=https%3A%2F%2Fexample.test%2Ffirst&",
            "sig=zQ%2F2qy5FZ4F3EeuZS1wiX7R4BWyqx8426DrRG5LOM58%3D",
        );
        const EXPIRES: u128 = 2_000_000_000_000;
        let verify = |input: &str| SignedOAuthQuery::verify(input, SECRET, EXPIRES);
        assert!(verify(SIGNED.split("&sig=").next().unwrap()).is_none());
        assert!(verify(&SIGNED.replace("client_id=fixture", "client_id=untrusted")).is_none());
        assert!(verify(&format!("{SIGNED}&sig=duplicate")).is_none());
        assert!(SignedOAuthQuery::verify(SIGNED, SECRET, EXPIRES + 1).is_none());

        let issued_at = 1_999_999_999_500;
        let raw_cookie =
            auth_get_session::signed_cookie("session_token", "new-token", SECRET, true, false);
        let marker = auth_get_session::signed_cookie("dont_remember", "true", SECRET, true, false);
        let cookie = continuation_cookie(cookie_pair(&raw_cookie), Some(&marker));
        let continuation = verify(SIGNED)
            .unwrap()
            .continue_with(issued_at, cookie.clone());
        assert_eq!(continuation.uri.path(), "/api/auth/oauth2/authorize");
        assert_eq!(
            continuation.uri.query().unwrap(),
            concat!(
                "state=%CE%A9%F0%9F%98%80+%2B&redirect_uri=http%3A%2F%2Flocalhost%3A8000%2Fcallback%23a&",
                "resource=https%3A%2F%2Fexample.test%2Fsecond&client_id=fixture&prompt=consent&resource=https%3A%2F%2Fexample.test%2Ffirst",
            )
        );
        assert_eq!(continuation.cookie, cookie);
        assert!(continuation.cookie.is_sensitive());
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, continuation.cookie);
        assert_eq!(
            session_identity::token(&headers, SECRET, true).as_deref(),
            Some("new-token")
        );
        assert!(auth_get_session::marker(&headers, SECRET, true));
        let stale = verify(SIGNED).unwrap().continue_with(issued_at - 1, cookie);
        assert!(stale.uri.query().unwrap().contains("&max_age=0&"));
    }

    #[test]
    fn login_json_rejects_explicit_null_for_optional_fields() {
        for extra in [",\"rememberMe\":null", ",\"oauth_query\":null"] {
            let data = format!("{{\"phone\":\"+12025550123\",\"password\":\"x\"{extra}}}");
            assert!(serde_json::from_str::<Login>(&data).is_err());
        }
        assert!(
            serde_json::from_str::<Login>(r#"{"phone":"+12025550123","password":"x"}"#).is_ok()
        );
    }

    #[test]
    fn normalizes_phones() {
        assert_eq!(phone(" 0912 345-678 ").as_deref(), Some("+963912345678"));
        assert_eq!(phone("\u{06f0}\u{06f0}\u{06f9}\u{06f6}\u{06f3}\u{06f9}\u{06f1}\u{06f2}\u{06f3}\u{06f4}\u{06f5}\u{06f6}\u{06f7}\u{06f8}").as_deref(), Some("+963912345678"));
        assert_eq!(phone("+01234567"), None);
    }
}
