use axum::{
    Json,
    body::to_bytes,
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use deadpool_postgres::Pool;
use serde_json::{Value, json};
use url::Url;

use crate::{
    mcp_operations::{self, Identity},
    mcp_tools, oauth, telegram_teacher,
};

const PROTOCOL_VERSIONS: &[&str] = &[
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
    "2024-10-07",
];
const INSTRUCTIONS: &str = "Tools follow the signed-in Wartaq account and its granted scopes. Start with get_account when available. Staff can use list_courses and find_students to resolve names to IDs. Read the selected session before syncing attendance. Reuse the same IDs, times and payloads on retries. Open private Wartaq links in a browser for setup, passwords and invitation messages.";
const MAX_RPC_BODY: usize = 4 * 1024 * 1024;
const MAX_BATCH_SIZE: usize = 100;
const INVITATION_URI: &str = "ui://wartaq/invitation.html";
const INVITATION_MIME: &str = "text/html;profile=mcp-app";

pub(crate) async fn public(
    pool: &Pool,
    origin: &str,
    secret: Option<&str>,
    request: Request,
) -> Response {
    if !same_site(origin, &request) {
        return rpc_error(StatusCode::FORBIDDEN, -32000, "Forbidden");
    }
    let challenge =
        format!("Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource/mcp\"");
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .filter(|value| !value.is_empty());
    let validated = if authorization.is_some() {
        match pool.get().await {
            Ok(db) => oauth::validate_resource(&db, origin, request.headers(), request.method())
                .await
                .ok()
                .flatten(),
            Err(_) => None,
        }
    } else {
        None
    };
    let Some(grant) = validated else {
        let invalid = if authorization.is_some() {
            ", error=\"invalid_token\""
        } else {
            ""
        };
        let challenge = format!(
            "{challenge}, scope=\"mcp:read mcp:write offline_access\"{invalid}, DPoP algs=\"EdDSA ES256 ES512 PS256 RS256\""
        );
        return with_challenge(
            rpc_error(StatusCode::UNAUTHORIZED, -32000, "Unauthorized"),
            challenge,
        );
    };
    if !grant
        .scopes
        .iter()
        .any(|scope| matches!(scope.as_str(), "mcp:read" | "mcp:write"))
    {
        return with_challenge(
            rpc_error(StatusCode::FORBIDDEN, -32000, "Insufficient scope"),
            format!("{challenge}, error=\"insufficient_scope\", scope=\"mcp:read\""),
        );
    }
    serve(
        request,
        Context {
            pool,
            origin,
            secret,
            identity: Identity::OAuth(&grant),
        },
    )
    .await
}

fn with_challenge(mut response: Response, challenge: String) -> Response {
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

pub(crate) async fn telegram(
    pool: &Pool,
    origin: &str,
    worker_secret: Option<&str>,
    request: Request,
) -> Response {
    let Some(prefix) = telegram_teacher::grant_prefix(request.headers(), worker_secret) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Ok(db) = pool.get().await else {
        return rpc_error(StatusCode::INTERNAL_SERVER_ERROR, -32000, "Server error");
    };
    let found = telegram_teacher::identify(&db, prefix).await;
    drop(db);
    let credential = match found {
        Ok(Some(credential)) => credential,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => return rpc_error(StatusCode::INTERNAL_SERVER_ERROR, -32000, "Server error"),
    };
    if !same_site(origin, &request) {
        return rpc_error(StatusCode::FORBIDDEN, -32000, "Forbidden");
    }
    serve(
        request,
        Context {
            pool,
            origin,
            secret: None,
            identity: Identity::Telegram(&credential),
        },
    )
    .await
}

struct Context<'a> {
    pool: &'a Pool,
    origin: &'a str,
    secret: Option<&'a str>,
    identity: Identity<'a>,
}

async fn serve(request: Request, context: Context<'_>) -> Response {
    if request.method() != Method::POST {
        let mut response = rpc_error(StatusCode::METHOD_NOT_ALLOWED, -32000, "Method not allowed");
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
        return response;
    }
    let session = match mcp_operations::open(context.pool, context.identity).await {
        Ok(Some(session)) => session,
        Ok(None) => return rpc_error(StatusCode::FORBIDDEN, -32000, "Forbidden"),
        Err(_) => return rpc_error(StatusCode::INTERNAL_SERVER_ERROR, -32000, "Server error"),
    };
    rpc(request, &context, &session).await
}

fn same_site(origin: &str, request: &Request) -> bool {
    let Ok(expected) = Url::parse(origin) else {
        return false;
    };
    let host = request
        .uri()
        .authority()
        .map(|host| host.as_str())
        .or_else(|| {
            request
                .headers()
                .get(header::HOST)
                .and_then(|host| host.to_str().ok())
        });
    let host_matches = host
        .and_then(|host| Url::parse(&format!("{}://{host}", expected.scheme())).ok())
        .is_some_and(|actual| {
            actual.origin() == expected.origin()
                && actual.path() == "/"
                && actual.username().is_empty()
                && actual.password().is_none()
                && actual.query().is_none()
                && actual.fragment().is_none()
        });
    let headers = request.headers();
    let origins = headers.get_all(header::ORIGIN);
    let mut origins = origins.iter();
    host_matches
        && headers.get_all(header::HOST).iter().nth(1).is_none()
        && origins
            .next()
            .is_none_or(|value| value.is_empty() || value.as_bytes() == origin.as_bytes())
        && origins.next().is_none()
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn json_reply(status: StatusCode, value: Value) -> Response {
    no_store((status, Json(value)).into_response())
}

fn error_value(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "error":{"code":code, "message":message}})
}

fn rpc_error(status: StatusCode, code: i32, message: &str) -> Response {
    json_reply(status, error_value(Value::Null, code, message))
}

fn header_text(headers: &HeaderMap, name: impl axum::http::header::AsHeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

enum Message {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Other,
}

impl Message {
    fn parse(value: Value) -> Option<Self> {
        let value = value.as_object()?;
        if value.get("jsonrpc")?.as_str()? != "2.0" {
            return None;
        }
        let id = value.get("id");
        if let Some(id) = id
            && !(id.is_string() || id.is_number())
        {
            return None;
        }
        if let Some(method) = value.get("method") {
            let method = method.as_str()?;
            let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
            if !params.is_object() {
                return None;
            }
            return Some(match id {
                Some(id) => Self::Request {
                    id: id.clone(),
                    method: method.to_owned(),
                    params,
                },
                None => Self::Other,
            });
        }
        (id.is_some()
            && (value.get("result").is_some_and(Value::is_object)
                || value.get("error").is_some_and(|error| {
                    error["code"].is_number() && error["message"].is_string()
                })))
        .then_some(Self::Other)
    }

    fn initialization(&self) -> bool {
        matches!(self, Self::Request { method, params, .. } if method == "initialize" && initialize_version(params).is_some())
    }
}

fn initialize_version(params: &Value) -> Option<&str> {
    let requested = params["protocolVersion"].as_str()?;
    params["capabilities"].as_object()?;
    params["clientInfo"]["name"].as_str()?;
    params["clientInfo"]["version"].as_str()?;
    Some(if PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        PROTOCOL_VERSIONS[0]
    })
}

async fn rpc(
    request: Request,
    context: &Context<'_>,
    session: &mcp_operations::Session,
) -> Response {
    let (parts, body) = request.into_parts();
    let accepts = |mime| {
        parts
            .headers
            .get_all(header::ACCEPT)
            .iter()
            .filter_map(|header| header.to_str().ok())
            .any(|value| value.contains(mime))
    };
    if !accepts("application/json") || !accepts("text/event-stream") {
        return rpc_error(
            StatusCode::NOT_ACCEPTABLE,
            -32000,
            "Not Acceptable: Client must accept both application/json and text/event-stream",
        );
    }
    let media_type = header_text(&parts.headers, header::CONTENT_TYPE)
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default();
    if !media_type.eq_ignore_ascii_case("application/json")
        || parts
            .headers
            .get_all(header::CONTENT_TYPE)
            .iter()
            .nth(1)
            .is_some()
    {
        return rpc_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            -32000,
            "Unsupported Media Type: Content-Type must be application/json",
        );
    }
    let Ok(bytes) = to_bytes(body, MAX_RPC_BODY).await else {
        return rpc_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            -32000,
            &format!("Payload Too Large: Request body must not exceed {MAX_RPC_BODY} bytes"),
        );
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return rpc_error(StatusCode::BAD_REQUEST, -32700, "Parse error: Invalid JSON");
    };
    let values = match value {
        Value::Array(values) => values,
        value => vec![value],
    };
    if values.len() > MAX_BATCH_SIZE {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            -32600,
            &format!("Invalid Request: Batch must not exceed {MAX_BATCH_SIZE} messages"),
        );
    }
    let Some(messages) = values
        .into_iter()
        .map(Message::parse)
        .collect::<Option<Vec<_>>>()
    else {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            -32700,
            "Parse error: Invalid JSON-RPC message",
        );
    };
    if messages.iter().any(Message::initialization) {
        if messages.len() > 1 {
            return rpc_error(
                StatusCode::BAD_REQUEST,
                -32600,
                "Invalid Request: Only one initialization request is allowed",
            );
        }
    } else if let Some(version) = header_text(&parts.headers, "mcp-protocol-version")
        && !PROTOCOL_VERSIONS.contains(&version)
    {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            -32000,
            &format!(
                "Bad Request: Unsupported protocol version: {version} (supported versions: {})",
                PROTOCOL_VERSIONS.join(", ")
            ),
        );
    }
    let mut responses = Vec::new();
    for message in messages {
        if let Message::Request { id, method, params } = message {
            responses.push(dispatch(context, session, id, &method, &params).await);
        }
    }
    match responses.len() {
        0 => no_store(StatusCode::ACCEPTED.into_response()),
        1 => json_reply(StatusCode::OK, responses.remove(0)),
        _ => json_reply(StatusCode::OK, json!(responses)),
    }
}

async fn dispatch(
    context: &Context<'_>,
    session: &mcp_operations::Session,
    id: Value,
    method: &str,
    params: &Value,
) -> Value {
    let origin = context.origin;
    let has_resource = mcp_tools::admin_write(&session.access);
    let result = match method {
        "initialize" => {
            let Some(version) = initialize_version(params) else {
                return error_value(id, -32602, "Invalid params");
            };
            let mut capabilities = json!({"tools":{"listChanged":true}});
            if has_resource {
                capabilities["resources"] = json!({"listChanged":true});
            }
            json!({"protocolVersion":version, "capabilities":capabilities, "serverInfo":{"name":"Wartaq","title":"Wartaq attendance notebook","version":"0.1.0","websiteUrl":origin}, "instructions":INSTRUCTIONS})
        }
        "ping" => json!({}),
        "tools/list" => {
            if params
                .get("cursor")
                .is_some_and(|cursor| !cursor.is_string())
            {
                return error_value(id, -32602, "Invalid params");
            }
            json!({"tools":mcp_tools::discovery(&session.access, session.year)})
        }
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return error_value(id, -32602, "Invalid params");
            };
            let empty = json!({});
            let arguments = params.get("arguments").unwrap_or(&empty);
            if !arguments.is_object() {
                return error_value(id, -32602, "Invalid params");
            }
            match mcp_tools::operation(&session.access, name, arguments, session.year) {
                Ok(operation) => {
                    mcp_operations::call(
                        context.pool,
                        origin,
                        context.secret,
                        context.identity,
                        operation,
                    )
                    .await
                }
                Err(()) => {
                    mcp_operations::text(&format!("Invalid tool or arguments: {name}"), true)
                }
            }
        }
        "resources/list" if has_resource => {
            json!({"resources":[{"name":"Private invitation","uri":INVITATION_URI,"mimeType":INVITATION_MIME}]})
        }
        "resources/templates/list" if has_resource => json!({"resourceTemplates":[]}),
        "resources/read" if has_resource => {
            if params["uri"] != INVITATION_URI {
                return error_value(id, -32602, "Resource not found");
            }
            let origin_literal = serde_json::to_string(origin).expect("valid origin string");
            let html =
                include_str!("mcp_invitation.html").replace("WARTAQ_ORIGIN", &origin_literal);
            json!({"contents":[{"uri":INVITATION_URI,"mimeType":INVITATION_MIME,"text":html,"_meta":{"openai/widgetCSP":{"redirect_domains":[origin]}}}]})
        }
        _ => return error_value(id, -32601, "Method not found"),
    };
    json!({"jsonrpc":"2.0", "id":id, "result":result})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_requires_canonical_host_and_same_origin() {
        let mut req = Request::builder()
            .header(header::HOST, "WARTAQ.EXAMPLE:443")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(same_site("https://wartaq.example", &req));
        req.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://other.example"),
        );
        assert!(!same_site("https://wartaq.example", &req));
        req.headers_mut().clear();
        assert!(!same_site("https://wartaq.example", &req));
    }

    #[test]
    fn initialization_negotiates_supported_version() {
        let params = json!({"protocolVersion":"1990-01-01","capabilities":{},"clientInfo":{"name":"test","version":"0"}});
        assert_eq!(initialize_version(&params), Some("2025-11-25"));
        assert!(
            Message::parse(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_some()
        );
        assert!(Message::parse(json!({"jsonrpc":"2.0","id":null,"method":"ping"})).is_none());
    }
}
