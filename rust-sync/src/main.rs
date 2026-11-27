use std::{env, time::Duration};

use axum::{
    Router,
    extract::Request,
    http::{StatusCode, header},
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Auth {
    user_id: String,
}

#[derive(Deserialize)]
struct Grant {
    account: Account,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    #[serde(default, deserialize_with = "present_profile")]
    invitation_profile: Option<InvitationProfile>,
}
fn present_profile<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<InvitationProfile>, D::Error> {
    InvitationProfile::deserialize(deserializer).map(Some)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InvitationProfile {
    requires_setup: bool,
}

async fn gate(req: Request, next: Next, origin: String) -> Response {
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
                        let _ = io.emit("workspace:changed", &[(); 0]).await;
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
    let origin = env::var("WORKSPACE_ORIGIN")?;
    let upstream = url::Url::parse(&env::var("BUN_ORIGIN")?)?.join("/api/me")?;
    let database_url = env::var("DATABASE_URL")?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let (layer, io) = SocketIo::builder().max_payload(4096).build_layer();
    io.ns(
        "/",
        (async || {}).with(move |socket: SocketRef, TryData(auth): TryData<Auth>| {
            let http = http.clone();
            let upstream = upstream.clone();
            async move {
                let auth = auth.map_err(|_| "Unauthorized")?;
                if !(1..=128).contains(&auth.user_id.encode_utf16().count()) {
                    return Err("Unauthorized");
                }
                let cookie = socket
                    .req_parts()
                    .headers
                    .get(header::COOKIE)
                    .ok_or("Unauthorized")?
                    .to_str()
                    .map_err(|_| "Unauthorized")?;
                let response = http
                    .get(upstream)
                    .header(header::COOKIE, cookie)
                    .header("x-user-id", &auth.user_id)
                    .send()
                    .await
                    .map_err(|_| "Unauthorized")?;
                if !response.status().is_success() {
                    return Err("Unauthorized");
                }
                let grant: Grant = response.json().await.map_err(|_| "Unauthorized")?;
                if grant
                    .account
                    .invitation_profile
                    .is_some_and(|p| p.requires_setup)
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
    let app = Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(serde_json::json!({"ok": true})) }),
        )
        .layer(layer)
        .layer(middleware::from_fn(move |req, next| {
            gate(req, next, origin.clone())
        }));
    println!("Listening on http://127.0.0.1:{}", address.port());
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            task.abort();
            io.close().await;
        })
        .await?;
    Ok(())
}
