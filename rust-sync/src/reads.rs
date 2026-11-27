use std::time::Duration;

use axum::{
    Json, Router,
    extract::{OriginalUri, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use deadpool_postgres::{Manager, Pool, Runtime};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_postgres::NoTls;
use url::Url;

use crate::{access, ledger, staff_reads};

#[derive(Clone)]
struct ReadState {
    pool: Pool,
    origin: HeaderValue,
    session_url: Url,
    http: reqwest::Client,
}

pub(crate) fn router(
    database_url: &str,
    origin: String,
    bun_origin: &Url,
    http: reqwest::Client,
) -> Result<Router, Box<dyn std::error::Error>> {
    let mut db: tokio_postgres::Config = database_url.parse()?;
    db.connect_timeout(Duration::from_secs(10))
        .application_name("wartaq-rust-api")
        .options("-c statement_timeout=15000");
    let pool = Pool::builder(Manager::new(db, NoTls))
        .max_size(5)
        .wait_timeout(Some(Duration::from_secs(10)))
        .runtime(Runtime::Tokio1)
        .build()?;
    let state = ReadState {
        pool,
        origin: origin.parse()?,
        session_url: bun_origin.join("/api/auth/get-session")?,
        http,
    };
    Ok(Router::new()
        .route("/api/me", any(read))
        .route("/api/notebook", any(read))
        .route("/api/student/attendance", any(read))
        .route("/api/admin", any(read))
        .with_state(state))
}

#[derive(Deserialize)]
struct Session {
    user: User,
}

#[derive(Deserialize)]
struct User {
    id: String,
}

async fn subject(state: &ReadState, headers: &HeaderMap) -> Option<String> {
    let expected = headers.get("x-user-id")?.to_str().ok()?;
    let cookie = headers.get(header::COOKIE)?;
    let session: Session = state
        .http
        .get(state.session_url.clone())
        .header(header::COOKIE, cookie)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    (session.user.id == expected && !expected.is_empty()).then_some(session.user.id)
}

#[derive(Clone, Copy)]
enum ReadRoute {
    Me,
    Notebook,
    StudentAttendance,
    Admin,
}

async fn read(
    State(state): State<ReadState>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let from = headers
        .get(header::ORIGIN)
        .filter(|value| !value.is_empty());
    if method != Method::GET && method != Method::HEAD && from != Some(&state.origin) {
        return error(StatusCode::FORBIDDEN, "Forbidden");
    }
    if from.is_some_and(|value| value != state.origin) {
        return error(StatusCode::FORBIDDEN, "Forbidden");
    }
    let mut response = if method == Method::OPTIONS {
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
    } else if method != Method::GET {
        error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
    } else {
        let route = match uri.path() {
            "/api/me" => ReadRoute::Me,
            "/api/notebook" => ReadRoute::Notebook,
            "/api/student/attendance" => ReadRoute::StudentAttendance,
            "/api/admin" => ReadRoute::Admin,
            _ => return error(StatusCode::NOT_FOUND, "Not found"),
        };
        match read_authorized(&state, &headers, route).await {
            Ok(response) => response,
            Err(problem) if problem.is::<ledger::InvalidData>() => {
                error(StatusCode::BAD_REQUEST, "Invalid request")
            }
            Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "Server error"),
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

async fn read_authorized(
    state: &ReadState,
    headers: &HeaderMap,
    route: ReadRoute,
) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
    let Some(subject) = subject(state, headers).await else {
        return Ok(error(StatusCode::UNAUTHORIZED, "Unauthorized"));
    };
    let mut client = state.pool.get().await?;
    let tx = client.transaction().await?;
    let Some(grant) = access::load(&tx, &subject).await? else {
        return Ok(error(StatusCode::FORBIDDEN, "Forbidden"));
    };
    let response: Value = match route {
        ReadRoute::Me => grant.me(),
        _ if grant.requires_setup() => return Ok(error(StatusCode::FORBIDDEN, "Forbidden")),
        ReadRoute::Notebook if !grant.is_student() && grant.can_read() => {
            staff_reads::notebook(&tx, &grant).await?
        }
        ReadRoute::Admin if grant.is_admin() && grant.can_read() && grant.can_write() => {
            staff_reads::admin_board(&tx).await?
        }
        ReadRoute::StudentAttendance => {
            let Some((name, group)) = grant.student_name_and_group() else {
                return Ok(error(StatusCode::FORBIDDEN, "Forbidden"));
            };
            ledger::student_attendance(&tx, grant.id(), name, group).await?
        }
        _ => return Ok(error(StatusCode::FORBIDDEN, "Forbidden")),
    };
    tx.commit().await?;
    Ok(json_reply(StatusCode::OK, response))
}
