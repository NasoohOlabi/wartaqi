use crate::{oauth_client, oauth_grant, session_identity};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use deadpool_postgres::Pool;
use ring::{
    digest, hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::OnceCell;
use tokio_postgres::GenericClient;
use url::{Url, form_urlencoded};

pub(crate) use oauth_grant::{Grant, validate_resource};

type RateLimits = Arc<Mutex<HashMap<(String, String), (Instant, u16)>>>;

#[derive(Clone)]
pub(crate) struct App {
    pub(crate) pool: Pool,
    pub(crate) origin: String,
    pub(crate) issuer: String,
    pub(crate) resource: String,
    pub(crate) secret: Arc<str>,
    secure: bool,
    seeded: Arc<OnceCell<()>>,
    limits: Option<RateLimits>,
}

pub(crate) fn router(pool: Pool, origin: HeaderValue, secret: Arc<str>, secure: bool) -> Router {
    let origin = origin
        .to_str()
        .expect("configured origin is ASCII")
        .to_owned();
    Router::new()
        .route("/api/auth/error", get(error_page))
        .route("/api/auth/oauth2/register", post(machine))
        .route("/api/auth/oauth2/token", post(machine))
        .route("/api/auth/oauth2/revoke", post(machine))
        .route("/api/auth/oauth2/introspect", post(machine))
        .route("/api/auth/oauth2/authorize", get(browser).post(browser))
        .route("/api/auth/oauth2/continue", post(browser))
        .route("/api/auth/oauth2/consent", post(browser))
        .with_state(App {
            pool,
            issuer: format!("{origin}/api/auth"),
            resource: format!("{origin}/mcp"),
            origin,
            secret,
            secure,
            seeded: Arc::new(OnceCell::new()),
            limits: (std::env::var("NODE_ENV").as_deref() == Ok("production"))
                .then(Default::default),
        })
}

impl App {
    async fn ensure_resource(&self, db: &impl GenericClient) -> Result<(), Error> {
        self.seeded.get_or_try_init(|| async {
            db.execute(
                r#"INSERT INTO neon_auth."oauthResource" (identifier, name, "createdAt", "updatedAt")
                   VALUES ($1, $1, now(), now()) ON CONFLICT (identifier) DO NOTHING"#,
                &[&self.resource],
            ).await.map(|_| ())
        }).await?;
        Ok(())
    }

    fn rate_limit(&self, path: &str, headers: &HeaderMap) -> Result<(), Error> {
        let Some(limits) = &self.limits else {
            return Ok(());
        };
        let (budget, window) = match path {
            "/api/auth/oauth2/register" => (5, 60),
            "/api/auth/oauth2/token" => (20, 60),
            "/api/auth/oauth2/authorize" | "/api/auth/oauth2/revoke" => (30, 60),
            "/api/auth/oauth2/introspect" => (100, 60),
            _ => (100, 10),
        };
        let forwarded = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|raw| raw.trim().parse::<std::net::IpAddr>().ok());
        let ip = match forwarded {
            Some(std::net::IpAddr::V4(ip)) => ip.to_string(),
            Some(std::net::IpAddr::V6(ip)) => ip
                .to_ipv4_mapped()
                .map_or_else(|| format!("{:?}", &ip.segments()[..4]), |ip| ip.to_string()),
            None => String::new(),
        };
        let now = Instant::now();
        let mut limits = limits.lock().unwrap_or_else(|err| err.into_inner());
        limits.retain(|_, (touched, _)| now.duration_since(*touched).as_secs() < 60);
        if limits.len() >= 10_000 {
            return Err(Error::Protocol(
                StatusCode::TOO_MANY_REQUESTS,
                "slow_down",
                "Too many OAuth requests",
            ));
        }
        let (last, count) = limits.entry((path.to_owned(), ip)).or_insert((now, 0));
        if now.duration_since(*last).as_secs() >= window {
            *count = 0;
        }
        if *count >= budget {
            return Err(Error::Protocol(
                StatusCode::TOO_MANY_REQUESTS,
                "slow_down",
                "Too many OAuth requests",
            ));
        }
        *last = now;
        *count += 1;
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    Protocol(StatusCode, &'static str, &'static str),
    Internal,
}

impl Error {
    pub(crate) fn bad(code: &'static str, description: &'static str) -> Self {
        Self::Protocol(StatusCode::BAD_REQUEST, code, description)
    }
    fn denied() -> Self {
        Self::Protocol(StatusCode::UNAUTHORIZED, "Unauthorized", "")
    }
    pub(crate) fn response(self) -> Response {
        match self {
            Self::Protocol(status, error, description) => json_response(
                status,
                if description.is_empty() {
                    json!({"error":error})
                } else {
                    json!({"error":error,"error_description":description})
                },
            ),
            Self::Internal => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error":"server_error"}),
            ),
        }
    }
}

impl From<tokio_postgres::Error> for Error {
    fn from(_: tokio_postgres::Error) -> Self {
        Self::Internal
    }
}

pub(crate) fn json_response(status: StatusCode, value: Value) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(value),
    )
        .into_response()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn now_seconds() -> u64 {
    (now_ms() / 1000) as u64
}

pub(crate) fn hash(input: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, input).as_ref())
}

pub(crate) fn random_token() -> Result<String, Error> {
    let mut bytes = [0; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Internal)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(crate) fn hex_bytes(bytes: impl IntoIterator<Item = u8>) -> String {
    bytes
        .into_iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn scopes(value: &str) -> Vec<String> {
    value
        .split(' ')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Default)]
struct Pairs(Vec<(String, String)>);
impl Pairs {
    fn parse(input: &str) -> Self {
        Self(
            form_urlencoded::parse(input.strip_prefix('?').unwrap_or(input).as_bytes())
                .into_owned()
                .collect(),
        )
    }
    fn get(&self, field: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == field)
            .map(|(_, value)| value.as_str())
    }
    fn append(&mut self, field: &str, value: impl Into<String>) {
        self.0.push((field.to_owned(), value.into()));
    }
    fn serialize(&self) -> String {
        encode(self.0.iter())
    }
    fn without(&self, field: &str) -> Vec<(&String, &String)> {
        self.0
            .iter()
            .filter(|(key, _)| key != field)
            .map(|(key, value)| (key, value))
            .collect()
    }
    fn canonical(&self) -> String {
        let mut ordered = self.without("sig");
        ordered.sort_by(|(ka, va), (kb, vb)| {
            ka.encode_utf16()
                .cmp(kb.encode_utf16())
                .then_with(|| va.encode_utf16().cmp(vb.encode_utf16()))
        });
        form_urlencoded::Serializer::new(String::new())
            .extend_pairs(ordered)
            .finish()
    }
    fn signed(mut self, secret: &str) -> String {
        self.append("exp", (now_seconds() + 600).to_string());
        self.append("ba_iat", now_ms().to_string());
        let mut names: BTreeSet<String> = self.0.iter().map(|(name, _)| name.clone()).collect();
        names.insert("ba_param".to_owned());
        for name in names {
            self.append("ba_param", name)
        }
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let signature = STANDARD.encode(hmac::sign(&key, self.canonical().as_bytes()).as_ref());
        self.append("sig", signature);
        self.serialize()
    }
    fn verify(&self, secret: &str) -> bool {
        let Some(exp) = self.get("exp").and_then(|s| s.parse::<u128>().ok()) else {
            return false;
        };
        let Some(signature) = self.get("sig").and_then(|s| STANDARD.decode(s).ok()) else {
            return false;
        };
        exp.saturating_mul(1000) >= now_ms()
            && ["sig", "exp"]
                .iter()
                .all(|name| self.0.iter().filter(|(key, _)| key == name).count() == 1)
            && hmac::verify(
                &hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes()),
                self.canonical().as_bytes(),
                &signature,
            )
            .is_ok()
    }
    fn object(self) -> Result<Value, Error> {
        let mut map = serde_json::Map::new();
        let mut resources = Vec::new();
        for (key, value) in self.0 {
            if key == "resource" {
                resources.push(value);
            } else if key == "ba_param"
                || key == "ba_iat"
                || key == "ba_pl"
                || key == "exp"
                || key == "sig"
            {
                continue;
            } else if map.insert(key, json!(value)).is_some() {
                return Err(Error::bad(
                    "invalid_request",
                    "Duplicate OAuth request parameter",
                ));
            }
        }
        if !resources.is_empty() {
            map.insert("resource".to_owned(), json!(resources));
        }
        Ok(Value::Object(map))
    }
}

fn encode<'a>(pairs: impl Iterator<Item = &'a (String, String)>) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.map(|(key, value)| (key, value)))
        .finish()
}

async fn body(request: Request) -> Result<Value, Error> {
    let is_json = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        });
    let bytes = to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|_| Error::bad("invalid_request", "Request body is too large"))?;
    match is_json.as_deref() {
        Some("application/json") => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| Error::bad("invalid_request", "Invalid JSON request")),
        Some("application/x-www-form-urlencoded") => Pairs::parse(
            std::str::from_utf8(&bytes)
                .map_err(|_| Error::bad("invalid_request", "Invalid form body"))?,
        )
        .object(),
        _ => Err(Error::bad("invalid_request", "Unsupported request format")),
    }
}

async fn machine(State(app): State<App>, request: Request) -> Response {
    let result = machine_request(&app, request).await;
    match result {
        Ok(response) => response,
        Err(error) => error.response(),
    }
}
async fn machine_request(app: &App, request: Request) -> Result<Response, Error> {
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    app.rate_limit(&path, &headers)?;
    let raw = body(request).await?;
    let mut db = app.pool.get().await.map_err(|_| Error::Internal)?;
    app.ensure_resource(&**db).await?;
    if path.ends_with("/register") {
        return Ok(json_response(
            StatusCode::CREATED,
            oauth_client::register(&mut db, app, &headers, raw).await?,
        ));
    }
    oauth_grant::machine(&mut db, app, &path, &headers, raw).await
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Query {
    #[serde(skip_serializing_if = "Option::is_none")]
    response_type: Option<String>,
    pub(crate) client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) redirect_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_age: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resource: Option<Resources>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) code_challenge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) code_challenge_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dpop_jkt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claims: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_uri: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Resources {
    Single(String),
    Multiple(Vec<String>),
}

impl Resources {
    pub(crate) fn list(&self) -> &[String] {
        match self {
            Self::Single(value) => std::slice::from_ref(value),
            Self::Multiple(values) => values,
        }
    }
}

impl Query {
    fn to_pairs(&self) -> Result<Pairs, Error> {
        let value = serde_json::to_value(self).map_err(|_| Error::Internal)?;
        let object = value.as_object().ok_or(Error::Internal)?;
        let mut pairs = Pairs::default();
        for (key, value) in object {
            if let Some(items) = value.as_array() {
                for item in items {
                    if let Some(item) = item.as_str() {
                        pairs.append(key, item)
                    }
                }
            } else if let Some(value) = value.as_str() {
                pairs.append(key, value);
            } else if !value.is_null() {
                pairs.append(key, value.to_string());
            }
        }
        Ok(pairs)
    }

    fn prompts(&self) -> impl Iterator<Item = &str> {
        self.prompt
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
    }

    fn has_prompt(&self, prompt: &str) -> bool {
        self.prompts().any(|p| p == prompt)
    }

    fn clear_prompt(&mut self, value: &str) {
        let retained: Vec<_> = self.prompts().filter(|p| *p != value).collect();
        self.prompt = (!retained.is_empty()).then(|| retained.join(" "));
    }

    fn age(&self) -> Option<Result<u64, Error>> {
        self.max_age.as_ref().map(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str()?.trim().parse().ok())
                .ok_or_else(|| Error::bad("invalid_request", "Invalid max_age"))
        })
    }

    fn resources(&self) -> &[String] {
        self.resource
            .as_ref()
            .map(Resources::list)
            .unwrap_or_default()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Code {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) query: Query,
    pub(crate) user_id: String,
    pub(crate) session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reference_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) auth_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resource: Option<Vec<String>>,
}

struct Session {
    user: String,
    id: String,
    created_ms: i64,
}

async fn session(
    db: &impl GenericClient,
    app: &App,
    headers: &HeaderMap,
) -> Result<Option<Session>, Error> {
    let Some(token) = session_identity::token(headers, &app.secret, app.secure) else {
        return Ok(None);
    };
    Ok(db.query_opt(
        r#"SELECT s.id::text, s."userId"::text, (EXTRACT(EPOCH FROM s."createdAt") * 1000)::bigint
           FROM neon_auth.session s JOIN neon_auth."user" u ON u.id=s."userId"
           WHERE s.token=$1 AND s."expiresAt" > statement_timestamp()"#,
        &[&token],
    ).await?.map(|row| Session { id: row.get(0), user: row.get(1), created_ms: row.get(2) }))
}

fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"))
        || headers.get("sec-fetch-mode").is_some_and(|mode| {
            ["cors", "same-origin", "no-cors"]
                .iter()
                .any(|value| mode == *value)
        })
}

fn navigation(url: &str, as_json: bool) -> Response {
    if as_json {
        return json_response(StatusCode::OK, json!({"redirect":true,"url":url}));
    }
    let Ok(location) = HeaderValue::from_str(url) else {
        return Error::Internal.response();
    };
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, location),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
    )
        .into_response()
}

fn callback(app: &App, uri: &str, query: &Query, fields: &[(&str, &str)]) -> Result<String, Error> {
    let mut callback =
        Url::parse(uri).map_err(|_| Error::bad("invalid_request", "Invalid redirect URI"))?;
    let mut keys = vec!["code", "error", "error_description", "state", "iss"];
    keys.extend(fields.iter().map(|(name, _)| *name));
    let kept: Vec<_> = callback
        .query_pairs()
        .filter(|(key, _)| !keys.contains(&key.as_ref()))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    callback.set_query(None);
    let mut output = callback.query_pairs_mut();
    output.extend_pairs(kept).extend_pairs(fields);
    if let Some(state) = query.state.as_deref().filter(|state| !state.is_empty()) {
        output.append_pair("state", state);
    }
    output.append_pair("iss", &app.issuer);
    drop(output);
    Ok(callback.to_string())
}

fn local_error(app: &App, code: &str, description: &str) -> String {
    let mut query = Pairs::default();
    query.append("error", code);
    query.append("error_description", description);
    format!("{}/api/auth/error?{}", app.origin, query.serialize())
}

async fn error_page(request: Request) -> Response {
    let query = Pairs::parse(request.uri().query().unwrap_or_default());
    let escaped = |value: &str| -> String {
        value
            .chars()
            .take(1024)
            .fold(String::new(), |mut text, character| {
                match character {
                    '&' => text.push_str("&amp;"),
                    '<' => text.push_str("&lt;"),
                    '>' => text.push_str("&gt;"),
                    '"' => text.push_str("&quot;"),
                    '\'' => text.push_str("&#39;"),
                    _ => text.push(character),
                }
                text
            })
    };
    let error = escaped(query.get("error").unwrap_or("OAuth error"));
    let description = escaped(
        query
            .get("error_description")
            .unwrap_or("Start again in your MCP client."),
    );
    let html = format!(
        r#"<!DOCTYPE html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Error</title><body><main><h1>{error}</h1><p>{description}</p><a href="/">Go Home</a></main></body></html>"#
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; frame-ancestors 'none'; base-uri 'none'",
            ),
        ],
        html,
    )
        .into_response()
}

async fn browser(State(app): State<App>, request: Request) -> Response {
    match browser_request(&app, request).await {
        Ok(response) => response,
        Err(error) => error.response(),
    }
}

#[derive(Deserialize)]
struct BrowserAction {
    oauth_query: String,
    selected: Option<bool>,
    accept: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

enum BrowserStep {
    Authorize,
    Continue,
    Consent {
        accept: bool,
        scopes: Option<Vec<String>>,
    },
}

async fn browser_request(app: &App, request: Request) -> Result<Response, Error> {
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    app.rate_limit(&path, &headers)?;
    let mut as_json = wants_json(&headers);
    let is_get = request.method() == Method::GET || request.method() == Method::HEAD;
    if !is_get
        && headers
            .get(header::ORIGIN)
            .is_none_or(|origin| origin.as_bytes() != app.origin.as_bytes())
    {
        return Err(Error::Protocol(StatusCode::FORBIDDEN, "Forbidden", ""));
    }
    let mut issued_at = None;
    let (raw, step) = if path.ends_with("/authorize") {
        let raw = if is_get {
            let raw = request.uri().query().unwrap_or_default();
            if raw.len() > 8192 {
                return Err(Error::bad(
                    "invalid_request",
                    "Authorization request is too large",
                ));
            }
            Pairs::parse(raw).object()?
        } else {
            body(request).await?
        };
        (raw, BrowserStep::Authorize)
    } else {
        as_json = true;
        let action: BrowserAction = serde_json::from_value(body(request).await?)
            .map_err(|_| Error::bad("invalid_request", "Missing authorization continuation"))?;
        let params = Pairs::parse(&action.oauth_query);
        if action.oauth_query.len() > 8192 || !params.verify(&app.secret) {
            return Err(Error::bad(
                "invalid_signature",
                "Invalid or expired authorization continuation",
            ));
        }
        issued_at = params
            .get("ba_iat")
            .and_then(|value| value.parse::<i64>().ok());
        let step = if path.ends_with("/continue") && action.selected == Some(true) {
            BrowserStep::Continue
        } else if path.ends_with("/consent") {
            BrowserStep::Consent {
                accept: action.accept == Some(true),
                scopes: action.scope.as_deref().map(scopes),
            }
        } else {
            return Err(Error::bad(
                "invalid_request",
                "Missing continuation parameters",
            ));
        };
        (params.object()?, step)
    };
    let Ok(mut query) = serde_json::from_value::<Query>(raw) else {
        return Ok(navigation(
            &local_error(app, "invalid_request", "Invalid authorization parameters"),
            as_json,
        ));
    };
    let mut db = app.pool.get().await.map_err(|_| Error::Internal)?;
    app.ensure_resource(&**db).await?;
    let current = session(&**db, app, &headers).await?;
    if matches!(step, BrowserStep::Consent { .. })
        && current.as_ref().is_none_or(|session| {
            headers
                .get("x-user-id")
                .is_none_or(|id| id.as_bytes() != session.user.as_bytes())
        })
    {
        return Err(Error::denied());
    }
    let Some(client) =
        oauth_client::lookup(&**db, &app.origin, &app.resource, &query.client_id).await?
    else {
        return Ok(navigation(
            &local_error(app, "invalid_client", "client not found"),
            as_json,
        ));
    };
    if client.disabled == Some(true) || !client.allows_grant("authorization_code") {
        return Ok(navigation(
            &local_error(app, "unauthorized_client", "Client cannot authorize"),
            as_json,
        ));
    }
    let Some(redirect) = query
        .redirect_uri
        .clone()
        .filter(|uri| client.valid_redirect(uri))
    else {
        return Ok(navigation(
            &local_error(app, "invalid_redirect", "Invalid redirect URI"),
            as_json,
        ));
    };
    let invalid = |query: &Query, error, description| -> Result<Response, Error> {
        let url = callback(
            app,
            &redirect,
            query,
            &[("error", error), ("error_description", description)],
        )?;
        Ok(navigation(&url, as_json))
    };
    if query.response_type.as_deref() != Some("code") {
        return invalid(
            &query,
            "unsupported_response_type",
            "Only the code response type is supported",
        );
    }
    if query.request.is_some()
        || query.request_uri.is_some()
        || query.claims.is_some()
        || query
            .extra
            .get("response_mode")
            .is_some_and(|v| v.as_str() != Some("query"))
        || query.prompts().any(|p| {
            !matches!(
                p,
                "none" | "login" | "consent" | "select_account" | "create"
            )
        })
        || (query.has_prompt("none") && query.prompts().any(|p| p != "none"))
        || matches!(query.age(), Some(Err(_)))
    {
        return invalid(
            &query,
            "invalid_request",
            "Invalid authorization parameters",
        );
    }
    let mut allowed_scopes = query.scope.as_deref().map(scopes).unwrap_or_else(|| {
        client.scopes.clone().unwrap_or_else(|| {
            oauth_client::SCOPES
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect()
        })
    });
    if !client.allows_scopes(&allowed_scopes) {
        return invalid(&query, "invalid_scope", "Unsupported scope");
    }
    if let Err(error) = oauth_grant::policy(
        &**db,
        app,
        &client.client_id,
        query.resources(),
        &allowed_scopes,
    )
    .await
    {
        return match error {
            Error::Protocol(_, code, description) => invalid(&query, code, description),
            e => Err(e),
        };
    }
    let challenge_ok = match (
        &query.code_challenge,
        query.code_challenge_method.as_deref(),
    ) {
        (None, None) => !client.requires_pkce(&allowed_scopes),
        (Some(challenge), Some("S256")) => valid_sha256(challenge),
        _ => false,
    };
    if !challenge_ok
        || query
            .dpop_jkt
            .as_deref()
            .is_some_and(|jkt| !valid_sha256(jkt))
    {
        return invalid(&query, "invalid_request", "Invalid PKCE or DPoP binding");
    }
    query.scope = Some(allowed_scopes.join(" "));
    if matches!(step, BrowserStep::Continue) {
        query.clear_prompt("select_account");
    }
    if let BrowserStep::Consent { accept: false, .. } = step {
        return invalid(&query, "access_denied", "User denied access");
    }
    if matches!(step, BrowserStep::Consent { .. })
        && current.as_ref().is_some_and(|session| {
            issued_at.is_some_and(|iat| iat > 0 && iat <= session.created_ms)
        })
    {
        query.clear_prompt("login");
        query.max_age = None;
    }
    let prompt_none = query.has_prompt("none");
    let age_ok = |session: &Session| {
        query.age().transpose().ok().flatten().is_none_or(|age| {
            now_ms().saturating_sub(session.created_ms.max(0) as u128) <= u128::from(age) * 1000
        })
    };
    let Some(current) = current.filter(|session| {
        age_ok(session) && !query.has_prompt("login") && !query.has_prompt("create")
    }) else {
        if prompt_none {
            return invalid(&query, "login_required", "Authentication required");
        }
        return Ok(navigation(
            &format!("/mcp/login?{}", query.to_pairs()?.signed(&app.secret)),
            as_json,
        ));
    };
    query.max_age = None;
    if matches!(step, BrowserStep::Authorize) && query.has_prompt("select_account") {
        return Ok(navigation(
            &format!("/mcp/login?{}", query.to_pairs()?.signed(&app.secret)),
            as_json,
        ));
    }
    if let BrowserStep::Consent {
        scopes: Some(scopes),
        ..
    } = &step
    {
        if scopes.iter().any(|scope| !allowed_scopes.contains(scope)) {
            return Err(Error::bad(
                "invalid_request",
                "Scope not originally requested",
            ));
        }
        allowed_scopes.clone_from(scopes);
        query.scope = Some(scopes.join(" "));
    }
    let tx = db.transaction().await?;
    let locked = tx
        .query_opt(
            r#"SELECT id FROM neon_auth.session WHERE id=$1::text::uuid AND "userId"=$2::text::uuid
           AND "expiresAt">statement_timestamp() FOR KEY SHARE"#,
            &[&current.id, &current.user],
        )
        .await?;
    if locked.is_none() {
        return Err(Error::denied());
    }
    let resources = query.resources().to_vec();
    let recorded: bool = if matches!(step, BrowserStep::Consent { .. }) {
        tx.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            &[&format!(
                "oauth-consent:{}:{}",
                query.client_id, current.user
            )],
        )
        .await?;
        let existing = tx
            .execute(
                r#"UPDATE neon_auth."oauthConsent" SET scopes=$3, resources=$4, "updatedAt"=now()
               WHERE "clientId"=$1 AND "userId"=$2::text::uuid AND "referenceId" IS NULL"#,
                &[
                    &query.client_id,
                    &current.user,
                    &json!(allowed_scopes),
                    &json!(resources),
                ],
            )
            .await?;
        if existing == 0 {
            tx.execute(
                r#"INSERT INTO neon_auth."oauthConsent" ("clientId", "userId", scopes, resources, "createdAt", "updatedAt")
                   VALUES ($1, $2::text::uuid, $3, $4, now(), now())"#,
                &[&query.client_id, &current.user, &json!(allowed_scopes), &json!(resources)],
            ).await?;
        }
        query.clear_prompt("consent");
        true
    } else if client.skip_consent == Some(true) {
        true
    } else {
        tx.query_one(
            r#"SELECT EXISTS (SELECT 1 FROM neon_auth."oauthConsent"
               WHERE "clientId"=$1 AND "userId"=$2::text::uuid AND "referenceId" IS NULL
                 AND scopes @> $3 AND COALESCE(resources, '[]'::jsonb) @> $4)"#,
            &[
                &query.client_id,
                &current.user,
                &json!(allowed_scopes),
                &json!(resources),
            ],
        )
        .await?
        .get(0)
    };
    if query.has_prompt("consent") || !recorded {
        if prompt_none {
            return invalid(&query, "consent_required", "Consent is required");
        }
        return Ok(navigation(
            &format!("/mcp/consent?{}", query.to_pairs()?.signed(&app.secret)),
            as_json,
        ));
    }
    let code = random_token()?;
    let value = serde_json::to_string(&Code {
        kind: "authorization_code".to_owned(),
        query: query.clone(),
        user_id: current.user,
        session_id: current.id,
        reference_id: None,
        auth_time: Some(current.created_ms),
        resource: Some(resources),
    })
    .map_err(|_| Error::Internal)?;
    tx.execute(
        r#"INSERT INTO neon_auth.verification (identifier, value, "expiresAt")
           VALUES ($1, $2, now() + interval '10 minutes')"#,
        &[&hash(code.as_bytes()), &value],
    )
    .await?;
    tx.commit().await?;
    let callback = callback(app, &redirect, &query, &[("code", &code)])?;
    Ok(navigation(&callback, as_json))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_query_keeps_repeated_resources_and_fails_after_tampering() {
        let secret = "synthetic signing secret";
        let mut pairs = Pairs::default();
        pairs.append("client_id", "sample");
        pairs.append("resource", "https://example.test/mcp");
        pairs.append("resource", "https://example.test/other");
        let issued = pairs.signed(secret);
        assert!(Pairs::parse(&issued).verify(secret));
        assert!(!Pairs::parse(&issued.replace("sample", "forged")).verify(secret));
        assert!(!Pairs::parse(&format!("{issued}&exp=9999999999")).verify(secret));
        assert_eq!(
            Pairs::parse(&issued).object().ok().unwrap()["resource"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
