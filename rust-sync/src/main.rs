mod access;
mod api;
mod assets;
mod auth_browser;
mod auth_get_session;
mod auth_invitation_profile;
mod auth_profile;
mod auth_sign_in;
mod catalog;
mod catalog_legacy;
mod compat_http;
mod compat_process;
mod credentials;
mod device_sessions;
mod grade_sync;
mod ledger;
mod ledger_writes;
#[cfg(test)]
mod mcp_discovery;
#[cfg(test)]
mod mcp_tools;
mod notebook_focus;
mod notebook_history;
mod notebook_student;
mod notebook_summary;
mod production_db;
mod roster_admin;
mod session_identity;
mod staff_reads;
mod student_writes;
mod telegram_conversations;
mod telegram_student;
#[cfg(test)]
mod telegram_teacher;
mod writes;

use std::{env, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::Request,
    http::{Extensions, HeaderMap, StatusCode, Version, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::future::poll_fn;
use serde::Deserialize;
use socketioxide::{
    SocketIo,
    extract::{SocketRef, TryData},
    handler::ConnectHandler,
};
use tokio_postgres::{AsyncMessage, NoTls};
use tower_http::compression::{
    CompressionLayer, CompressionLevel,
    predicate::{Predicate, SizeAbove},
};

fn native_hybrid_route(method: &axum::http::Method, path: &str) -> bool {
    match *method {
        axum::http::Method::GET => matches!(
            path,
            "/api/me"
                | "/api/notebook"
                | "/api/admin"
                | "/api/student/attendance"
                | "/api/auth/get-session"
                | "/api/device-accounts"
        ),
        axum::http::Method::POST => matches!(
            path,
            "/api/device-accounts/switch"
                | "/api/attendance/sync"
                | "/api/student/sync"
                | "/api/events/correct"
        ),
        _ => false,
    }
}

#[derive(Clone)]
struct PrivateProjection;

async fn mark_private_projection(req: Request, next: Next) -> Response {
    let projection = req.method() == axum::http::Method::GET
        && matches!(
            req.uri().path(),
            "/api/notebook" | "/api/admin" | "/api/student/attendance"
        );
    let mut response = next.run(req).await;
    if projection && response.status() == StatusCode::OK {
        response.extensions_mut().insert(PrivateProjection);
    }
    response
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Auth {
    user_id: String,
}

async fn gate(
    req: Request,
    next: Next,
    origin: String,
    mut shutdown: tokio::sync::watch::Receiver<()>,
) -> Response {
    if req.uri().path().starts_with("/socket.io") && req.uri().path() != "/socket.io/" {
        return StatusCode::NOT_FOUND.into_response();
    }
    if req.uri().path() == "/socket.io/" {
        let h = req.headers();
        if h.get(header::ORIGIN)
            .is_some_and(|v| !v.is_empty() && v.as_bytes() != origin.as_bytes())
            || h.get("sec-fetch-site")
                .is_some_and(|v| !v.is_empty() && v.as_bytes() != b"same-origin")
        {
            return (StatusCode::FORBIDDEN, "Forbidden").into_response();
        }
        if !h.contains_key(header::COOKIE) {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({"code": 4, "message": {}})),
            )
                .into_response();
        }
        if req.method() == axum::http::Method::GET {
            // A pending Engine.IO poll can outlive its disconnected Socket.IO namespace.
            return tokio::select! {
                response = next.run(req) => response,
                _ = shutdown.changed() => StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
        }
    }
    next.run(req).await
}

async fn listen(database_url: String, io: SocketIo) {
    let mut delay = 250;
    loop {
        let result = async {
            let mut config: tokio_postgres::Config = database_url.parse()?;
            let url = url::Url::parse(&database_url)?;
            if !url.query_pairs().any(|(key, _)| key == "application_name") {
                config.application_name("wartaq-rust-workspace-listener");
            }
            let (client, mut connection) =
                tokio::time::timeout(Duration::from_secs(10), config.connect(NoTls)).await??;
            let startup = async {
                let query = client.batch_execute("LISTEN wartaq_workspace_changed");
                tokio::pin!(query);
                loop {
                    tokio::select! {
                        result = &mut query => break result.map_err(|_| "LISTEN failed"),
                        message = poll_fn(|cx| connection.poll_message(cx)) => {
                            if !matches!(message, Some(Ok(_))) { break Err("listener disconnected"); }
                        }
                    }
                }
            };
            tokio::time::timeout(Duration::from_secs(10), startup).await??;
            delay = 250;
            let _ = io.emit("workspace:changed", &[(); 0]).await;
            while let Some(message) = poll_fn(|cx| connection.poll_message(cx)).await {
                match message {
                    Ok(AsyncMessage::Notification(n)) if n.channel() == "wartaq_workspace_changed" => {
                        if n.payload() == "ledger" {
                            let _ = io.emit("workspace:changed", &"ledger").await;
                        } else {
                            let _ = io.emit("workspace:changed", &[(); 0]).await;
                        }
                    }
                    Ok(_) => {},
                    Err(_) => break,
                }
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        }
        .await;
        let _ = result;
        eprintln!("Workspace notification listener disconnected");
        tokio::time::sleep(Duration::from_millis(delay)).await;
        delay = (delay * 2).min(10_000);
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("signal handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = env::var("PORT").unwrap_or_else(|_| "3001".into()).parse()?;
    let hybrid = env::var("WARTAQ_WEB_RUNTIME").as_deref() == Ok("hybrid");
    let origin = if hybrid {
        let url = url::Url::parse(&env::var("BETTER_AUTH_URL")?)?;
        url.origin().ascii_serialization()
    } else {
        env::var("WORKSPACE_ORIGIN")?
    };
    let bun_origin = env::var("BUN_ORIGIN")
        .ok()
        .map(|origin| url::Url::parse(&origin))
        .transpose()?;
    let database_url = env::var("DATABASE_URL")?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()?;
    let api = api::create(
        &database_url,
        origin.clone(),
        bun_origin.as_ref(),
        http,
        env::var("BETTER_AUTH_SECRET").ok(),
        env::var("TELEGRAM_WORKER_SECRET").ok(),
    )?;
    let routes = api.router();
    let pool = api.pool().clone();
    let (socket_service, io) = SocketIo::builder().max_payload(4096).build_svc();
    io.ns(
        "/",
        (async || {}).with(move |socket: SocketRef, TryData(auth): TryData<Auth>| {
            let api = api.clone();
            async move {
                let auth = auth.map_err(|_| "Unauthorized")?;
                if !(1..=128).contains(&auth.user_id.encode_utf16().count()) {
                    return Err("Unauthorized");
                }
                if !api
                    .authorize_socket(&socket.req_parts().headers, &auth.user_id)
                    .await
                {
                    return Err("Unauthorized");
                }
                Ok(())
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(listen(database_url, io.clone()));
    let mut app = Router::new().merge(routes);
    if hybrid {
        tokio::spawn(production_db::maintain(pool.clone()));
        app = app.route(
            "/health",
            get(move || async move { production_db::health(&pool).await }),
        );
    } else {
        app = app.route(
            "/health",
            get(|| async { axum::Json(serde_json::json!({"ok": true})) }),
        );
    }
    if let Some(root) = env::var_os("ASSET_DIR") {
        let root = Arc::new(PathBuf::from(root).canonicalize()?);
        if !root.is_dir() {
            return Err("ASSET_DIR must be a directory".into());
        }
        let static_files = Router::new().fallback(move |request: Request| {
            let root = root.clone();
            async move {
                let path = request.uri().path();
                if path.starts_with("/api/")
                    || path.starts_with("/.well-known/")
                    || path == "/mcp"
                    || path.starts_with("/mcp/")
                {
                    return StatusCode::NOT_FOUND.into_response();
                }
                let path = path.to_owned();
                let method = request.method().clone();
                let headers = request.into_parts().0.headers;
                tokio::task::spawn_blocking(move || assets::serve(&root, &method, &path, &headers))
                    .await
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            }
        });
        app = app.merge(static_files.layer(tower_http::compression::CompressionLayer::new()));
    }
    if hybrid {
        let supervisor = compat_process::CompatProcess::new();
        let client = compat_http::client()?;
        app = app.layer(middleware::from_fn(move |request: Request, next: Next| {
            let supervisor = supervisor.clone();
            let client = client.clone();
            async move {
                let path = request.uri().path();
                let api = path.starts_with("/api/")
                    || path == "/mcp"
                    || path.starts_with("/mcp/")
                    || path.starts_with("/.well-known/");
                if api
                    && !native_hybrid_route(request.method(), path)
                    && !api::retired_website_path(path)
                {
                    compat_http::forward(&client, &supervisor, request).await
                } else {
                    next.run(request).await
                }
            }
        }));
    }
    app = app
        .layer(middleware::from_fn(mark_private_projection))
        .layer(
            CompressionLayer::new()
                .quality(CompressionLevel::Fastest)
                .compress_when(SizeAbove::new(1024).and(
                    |_: StatusCode, _: Version, _: &HeaderMap, extensions: &Extensions| {
                        extensions.get::<PrivateProjection>().is_some()
                    },
                )),
        );
    let (stop_polls, shutdown) = tokio::sync::watch::channel(());
    let app = app
        .route_service("/socket.io/", socket_service)
        .layer(middleware::from_fn(move |req, next| {
            gate(req, next, origin.clone(), shutdown.clone())
        }));
    if hybrid {
        eprintln!("Rust web runtime ready with on-demand Bun compatibility");
    }
    println!("Listening on http://127.0.0.1:{}", address.port());
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            drop(stop_polls);
            task.abort();
            io.close().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod hybrid_route_tests {
    use super::native_hybrid_route;
    use axum::http::Method;

    #[test]
    fn classroom_routes_are_native_but_auth_and_other_writes_use_bun() {
        for (method, path) in [
            (Method::GET, "/api/device-accounts"),
            (Method::POST, "/api/device-accounts/switch"),
            (Method::POST, "/api/attendance/sync"),
            (Method::POST, "/api/student/sync"),
        ] {
            assert!(native_hybrid_route(&method, path), "{method} {path}");
        }
        for (method, path) in [
            (Method::POST, "/api/auth/sign-in/phone"),
            (Method::POST, "/api/auth/sign-out"),
            (Method::POST, "/api/events"),
            (Method::GET, "/api/device-accounts/switch"),
            (Method::POST, "/api/device-accounts"),
        ] {
            assert!(!native_hybrid_route(&method, path), "{method} {path}");
        }
    }
}
