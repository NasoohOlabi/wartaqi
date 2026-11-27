use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use deadpool_postgres::{Manager, Pool, Runtime};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_postgres::NoTls;
use url::Url;

use crate::{
    access, auth_browser, auth_get_session, auth_invitation_profile, auth_profile, catalog,
    catalog_legacy, grade_sync, ledger, ledger_writes, roster_admin, session_identity, staff_reads,
    student_writes, telegram_conversations, telegram_student, writes::Error,
};

const WRITE_PATHS: &[&str] = &[
    "/api/events",
    "/api/attendance/sync",
    "/api/student/update",
    "/api/student/sync",
    "/api/admin/courses",
    "/api/admin/courses/settings",
    "/api/admin/courses/status",
    "/api/admin/students",
    "/api/admin/students/enroll",
    "/api/admin/enrollments",
    "/api/admin/enrollments/decision",
    "/api/admin/enrollments/handbook",
    "/api/admin/enrollments/grades",
    "/api/admin/enrollments/membership",
    "/api/admin/enrollments/assignment",
];
const REPLAY_PATH: &str = "/api/diagnostics/replay";
const RETIRED_DIAGNOSTICS_PATH: &str = "/api/diagnostics";
const JOURNEYS_PATH: &str = "/api/diagnostics/journeys";
const DIAGNOSTIC_EVENTS_PATH: &str = "/api/diagnostics/events";
const TELEGRAM_INGEST_PATH: &str = "/api/telegram/conversations";
const TELEGRAM_CONVERSATIONS_PATH: &str = "/api/diagnostics/telegram/conversations";
const TELEGRAM_TURNS_PATH: &str = "/api/diagnostics/telegram/turns";

pub(crate) fn retired_website_path(path: &str) -> bool {
    matches!(
        path,
        REPLAY_PATH | RETIRED_DIAGNOSTICS_PATH | JOURNEYS_PATH | DIAGNOSTIC_EVENTS_PATH
    )
}

#[derive(Clone)]
pub(crate) struct Api {
    pool: Pool,
    origin: HeaderValue,
    sessions: Sessions,
    telegram_worker_secret: Option<Arc<str>>,
}

#[derive(Clone)]
enum Sessions {
    Native { secret: Arc<str>, secure: bool },
    Bun { url: Url, http: reqwest::Client },
}

pub(crate) fn create(
    database_url: &str,
    origin: String,
    bun_origin: Option<&Url>,
    http: reqwest::Client,
    secret: Option<String>,
    telegram_worker_secret: Option<String>,
) -> Result<Api, Box<dyn std::error::Error>> {
    let mut db: tokio_postgres::Config = database_url.parse()?;
    db.connect_timeout(Duration::from_secs(10))
        .application_name("wartaq-rust-api")
        .options("-c statement_timeout=15000");
    let pool = Pool::builder(Manager::new(db, NoTls))
        .max_size(5)
        .wait_timeout(Some(Duration::from_secs(10)))
        .runtime(Runtime::Tokio1)
        .build()?;
    let sessions = match secret {
        Some(secret) if secret.encode_utf16().count() >= 32 => Sessions::Native {
            secret: secret.into(),
            secure: origin.starts_with("https://"),
        },
        Some(_) => return Err("BETTER_AUTH_SECRET must contain at least 32 characters".into()),
        None => Sessions::Bun {
            url: bun_origin
                .ok_or("BUN_ORIGIN is required without BETTER_AUTH_SECRET")?
                .join("/api/auth/get-session")?,
            http,
        },
    };
    Ok(Api {
        pool,
        origin: origin.parse()?,
        sessions,
        telegram_worker_secret: telegram_worker_secret.map(Into::into),
    })
}

impl Api {
    pub(crate) fn pool(&self) -> &Pool {
        &self.pool
    }

    pub(crate) fn router(&self) -> Router {
        let mut router = Router::new()
            .route("/api/me", any(handle))
            .route("/api/notebook", any(handle))
            .route("/api/student/attendance", any(handle))
            .route("/api/admin", any(handle))
            .route(REPLAY_PATH, any(handle))
            .route(RETIRED_DIAGNOSTICS_PATH, any(handle))
            .route(JOURNEYS_PATH, any(handle))
            .route(DIAGNOSTIC_EVENTS_PATH, any(handle))
            .route(TELEGRAM_CONVERSATIONS_PATH, any(handle))
            .route(TELEGRAM_TURNS_PATH, any(handle))
            .route("/api/telegram/student", any(telegram_student_request))
            .route(TELEGRAM_INGEST_PATH, any(telegram_ingest));
        if matches!(self.sessions, Sessions::Native { .. }) {
            router = router.route("/api/auth/get-session", get(current_session));
        }
        for path in WRITE_PATHS {
            router = router.route(path, any(handle));
        }
        let router = router.with_state(self.clone());
        match &self.sessions {
            Sessions::Native { secret, secure } => router
                .merge(auth_browser::router(
                    self.pool.clone(),
                    self.origin.clone(),
                    secret.clone(),
                    *secure,
                ))
                .merge(auth_profile::router(
                    self.pool.clone(),
                    self.origin.clone(),
                    secret.clone(),
                    *secure,
                ))
                .merge(auth_invitation_profile::router(
                    self.pool.clone(),
                    self.origin.clone(),
                    secret.clone(),
                    *secure,
                )),
            Sessions::Bun { .. } => router,
        }
    }

    pub(crate) async fn authorize_socket(&self, headers: &HeaderMap, expected: &str) -> bool {
        let Ok(mut client) = self.pool.get().await else {
            return false;
        };
        if subject(self, &client, headers, expected).await.as_deref() != Some(expected) {
            return false;
        }
        let Ok(tx) = client.transaction().await else {
            return false;
        };
        let Ok(Some(grant)) = access::load(&tx, expected).await else {
            return false;
        };
        !grant.requires_setup() && tx.commit().await.is_ok()
    }
}

async fn current_session(State(state): State<Api>, request: Request) -> Response {
    let Sessions::Native { ref secret, secure } = state.sessions else {
        return error(StatusCode::NOT_FOUND, "Not found");
    };
    let disable_refresh =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .any(|(key, value)| key == "disableRefresh" && !value.is_empty());
    match state.pool.get().await {
        Ok(client) => {
            auth_get_session::get_session(
                &client,
                request.headers(),
                secret,
                secure,
                disable_refresh,
            )
            .await
        }
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to get session"),
    }
}

#[derive(Deserialize)]
struct Session {
    user: User,
}

#[derive(Deserialize)]
struct User {
    id: String,
}

async fn subject(
    state: &Api,
    db: &tokio_postgres::Client,
    headers: &HeaderMap,
    expected: &str,
) -> Option<String> {
    let user_id = match &state.sessions {
        Sessions::Native { secret, secure } => {
            let token = session_identity::token(headers, secret, *secure)?;
            session_identity::user_id(db, &token).await.ok()??
        }
        Sessions::Bun { url, http } => {
            let cookie = headers.get(header::COOKIE)?;
            let session: Session = http
                .get(url.clone())
                .header(header::COOKIE, cookie)
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .json()
                .await
                .ok()?;
            session.user.id
        }
    };
    (user_id == expected && !expected.is_empty()).then_some(user_id)
}

#[derive(Clone, Copy)]
enum Route {
    Me,
    Notebook,
    StudentAttendance,
    Admin,
    Write,
    Diagnostics,
}

async fn handle(State(state): State<Api>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let headers = &parts.headers;
    let method = &parts.method;
    let from = headers
        .get(header::ORIGIN)
        .filter(|value| !value.is_empty());
    if *method != Method::GET && *method != Method::HEAD && from != Some(&state.origin) {
        return error(StatusCode::FORBIDDEN, "Forbidden");
    }
    if from.is_some_and(|value| value != state.origin) {
        return error(StatusCode::FORBIDDEN, "Forbidden");
    }
    if matches!(path, RETIRED_DIAGNOSTICS_PATH | REPLAY_PATH) {
        let status = if *method == Method::POST {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::NOT_FOUND
        };
        let message = if path == REPLAY_PATH {
            "Session replay retired"
        } else {
            "Website diagnostics retired"
        };
        return error(status, message);
    }
    if matches!(path, JOURNEYS_PATH | DIAGNOSTIC_EVENTS_PATH) {
        return error(StatusCode::NOT_FOUND, "Not found");
    }
    if matches!(path, TELEGRAM_CONVERSATIONS_PATH | TELEGRAM_TURNS_PATH) {
        return match authorized(&state, &parts, Route::Diagnostics, body).await {
            Ok(response) => response,
            Err(Error::Forbidden) => write_error(Error::Forbidden),
            Err(Error::Invalid | Error::TooLarge) => write_error(Error::Invalid),
            Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Diagnostics unavailable"),
        };
    }
    let mut response = if *method == Method::OPTIONS {
        (
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
            .into_response()
    } else {
        let route = match path {
            "/api/me" => Route::Me,
            "/api/notebook" => Route::Notebook,
            "/api/student/attendance" => Route::StudentAttendance,
            "/api/admin" => Route::Admin,
            path if WRITE_PATHS.contains(&path) => Route::Write,
            _ => return error(StatusCode::NOT_FOUND, "Not found"),
        };
        let required = if matches!(route, Route::Write) {
            Method::POST
        } else {
            Method::GET
        };
        if *method != required {
            error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
        } else {
            match authorized(&state, &parts, route, body).await {
                Ok(response) => response,
                Err(problem) => write_error(problem),
            }
        }
    };
    if from.is_some() {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, state.origin);
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}

async fn telegram_ingest(State(state): State<Api>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    if let Err(status) = telegram_conversations::check_ingest(
        &parts.method,
        &parts.headers,
        state.telegram_worker_secret.as_deref(),
    ) {
        let message = match status {
            StatusCode::UNAUTHORIZED => "Unauthorized",
            StatusCode::METHOD_NOT_ALLOWED => "Method not allowed",
            _ => "Invalid request",
        };
        return error(status, message);
    }
    let Ok(bytes) = bounded_body(body, 65_536).await else {
        return error(StatusCode::BAD_REQUEST, "Invalid request");
    };
    if let Ok(client) = state.pool.get().await
        && let Ok(response) = telegram_conversations::ingest(&**client, &bytes).await
    {
        return response;
    }
    error(StatusCode::SERVICE_UNAVAILABLE, "Unavailable")
}

async fn telegram_student_request(State(state): State<Api>, request: Request) -> Response {
    telegram_student::handle(
        &state.pool,
        request,
        state.telegram_worker_secret.as_deref(),
    )
    .await
}

fn error(status: StatusCode, message: &str) -> Response {
    json_reply(status, json!({ "error": message }))
}

fn json_reply(status: StatusCode, value: Value) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json;charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Json(value),
    )
        .into_response()
}

fn write_error(problem: Error) -> Response {
    let (status, message) = match problem {
        Error::Invalid => (StatusCode::BAD_REQUEST, "Invalid request"),
        Error::Forbidden => (StatusCode::FORBIDDEN, "Forbidden"),
        Error::Conflict => (StatusCode::CONFLICT, "Conflict"),
        Error::NotFound => (StatusCode::NOT_FOUND, "Server error"),
        Error::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "Request too large"),
        Error::Other(problem) if problem.is::<ledger::InvalidData>() => {
            (StatusCode::BAD_REQUEST, "Invalid request")
        }
        Error::Other(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Server error"),
    };
    error(status, message)
}

async fn bounded_body(body: Body, limit: usize) -> Result<Vec<u8>, Error> {
    let mut chunks = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|error| Error::Other(error.into()))?;
        if chunk.len() > limit - bytes.len() {
            return Err(Error::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_body(body: Body) -> Result<Value, Error> {
    let bytes = bounded_body(body, 2 * 1024 * 1024).await?;
    serde_json::from_str(&String::from_utf8_lossy(&bytes)).map_err(|_| Error::Invalid)
}

async fn authorized(
    state: &Api,
    parts: &Parts,
    route: Route,
    body: Body,
) -> Result<Response, Error> {
    let headers = &parts.headers;
    let path = parts.uri.path();
    let Some(expected) = headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(error(StatusCode::UNAUTHORIZED, "Unauthorized"));
    };
    let mut client = state
        .pool
        .get()
        .await
        .map_err(|error| Error::Other(error.into()))?;
    let Some(subject) = subject(state, &client, headers, expected).await else {
        let message = if matches!(route, Route::Diagnostics) {
            "Forbidden"
        } else {
            "Unauthorized"
        };
        return Ok(error(StatusCode::UNAUTHORIZED, message));
    };
    let tx = client.transaction().await?;
    if headers.contains_key("x-edit-time") || headers.contains_key("x-edit-id") {
        let field = |name| {
            headers
                .get(name)
                .and_then(|h| h.to_str().ok())
                .ok_or(Error::Invalid)
        };
        let time = field("x-edit-time")?;
        let id = field("x-edit-id")?;
        if !ledger::iso_datetime(time) || !ledger::valid_uuid(id) {
            return Err(Error::Invalid);
        }
        if tx
            .query_one(
                "SELECT $1::text::timestamptz > clock_timestamp() + interval '5 minutes'",
                &[&time],
            )
            .await?
            .get::<_, bool>(0)
        {
            return Err(Error::Conflict);
        }
        tx.query_one("SELECT set_config('wartaq.edit_time', $1, true), set_config('wartaq.edit_id', $2, true)", &[&time, &id]).await?;
    }
    let Some(grant) = access::load(&tx, &subject).await? else {
        return Ok(error(StatusCode::FORBIDDEN, "Forbidden"));
    };
    let response: Value = match route {
        Route::Me => grant.me(),
        _ if grant.requires_setup() => return Err(Error::Forbidden),
        Route::Diagnostics => {
            if !grant.is_admin() {
                return Err(Error::Forbidden);
            }
            tx.commit().await?;
            return Ok(telegram_conversations::read(&**client, &parts.method, &parts.uri).await?);
        }
        Route::Notebook if !grant.is_student() && grant.can_read() => {
            staff_reads::notebook(&tx, &grant)
                .await
                .map_err(Error::Other)?
        }
        Route::Admin if grant.is_admin() && grant.can_read() && grant.can_write() => {
            staff_reads::admin_board(&tx).await.map_err(Error::Other)?
        }
        Route::StudentAttendance => {
            let Some((name, group)) = grant.student_name_and_group() else {
                return Err(Error::Forbidden);
            };
            ledger::student_attendance(&tx, grant.id(), name, group)
                .await
                .map_err(Error::Other)?
        }
        Route::Write if path.starts_with("/api/admin/") => {
            if !grant.is_admin() || !grant.can_read() || !grant.can_write() {
                return Err(Error::Forbidden);
            }
            let input = read_body(body).await?;
            if matches!(path, "/api/admin/students" | "/api/admin/students/enroll") {
                roster_admin::apply(&tx, path, input).await?
            } else if matches!(
                path,
                "/api/admin/enrollments"
                    | "/api/admin/enrollments/decision"
                    | "/api/admin/courses/status"
            ) {
                catalog_legacy::apply(&tx, path, &subject, input).await?
            } else if path == "/api/admin/enrollments/grades" {
                if let Some(response) = grade_sync::apply(&tx, &subject, &grant, &input).await? {
                    response
                } else {
                    catalog::apply(&tx, path, &subject, input).await?
                }
            } else {
                catalog::apply(&tx, path, &subject, input).await?
            }
        }
        Route::Write
            if !grant.is_student()
                && grant.can_write()
                && (!matches!(path, "/api/attendance/sync" | "/api/student/sync")
                    || grant.can_read()) =>
        {
            let input = read_body(body).await?;
            match path {
                "/api/student/update" => {
                    student_writes::apply(&tx, path, &subject, &grant, input).await?
                }
                "/api/student/sync" => {
                    let result = match input["operation"]["kind"].as_str() {
                        Some("note") => {
                            student_writes::sync_note(&tx, &subject, &grant, &input).await?
                        }
                        Some("contact") => {
                            student_writes::sync_contact(&tx, &subject, &grant, &input).await?
                        }
                        Some("profile-v1") => {
                            student_writes::sync_profile(&tx, &subject, &grant, &input).await?
                        }
                        _ => return Err(Error::Invalid),
                    };
                    result.ok_or(Error::Invalid)?
                }
                _ => ledger_writes::apply(&tx, path, &subject, &grant, input).await?,
            }
        }
        _ => return Err(Error::Forbidden),
    };
    tx.commit().await?;
    Ok(json_reply(StatusCode::OK, response))
}
