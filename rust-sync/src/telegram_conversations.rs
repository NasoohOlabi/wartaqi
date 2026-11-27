use axum::{
    Json,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::GenericClient;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Turn {
    trace: String,
    conversation_id: String,
    question: String,
    reply: String,
    delivery: String,
    outcome: String,
    prompt_version: String,
    model: String,
    elapsed_ms: f64,
}

fn matches(pattern: &str, value: &str) -> bool {
    Regex::new(pattern).unwrap().is_match(value)
}
fn valid(turn: &Turn) -> bool {
    matches(r"^[A-Za-z0-9_-]{12}$", &turn.trace)
        && matches(r"^[a-f0-9]{64}$", &turn.conversation_id)
        && (1..=1000).contains(&turn.question.encode_utf16().count())
        && (1..=3000).contains(&turn.reply.encode_utf16().count())
        && ["sent", "failed"].contains(&turn.delivery.as_str())
        && ([
            "ok",
            "unconfigured",
            "malformed",
            "tool_request",
            "output_limit",
            "incomplete",
            "empty",
            "link_output",
            "timeout",
            "network",
        ]
        .contains(&turn.outcome.as_str())
            || matches(r"^http_[1-5][0-9]{2}$", &turn.outcome))
        && matches(r"^wartaq-help-v[0-9]{1,8}$", &turn.prompt_version)
        && matches(r"^[A-Za-z0-9_./:-]{1,100}$", &turn.model)
        && (0.0..=600_000.0).contains(&turn.elapsed_ms)
        && turn.elapsed_ms.fract() == 0.0
}
fn redact(text: &str, limit: usize) -> String {
    let patterns = [
        r"(?i)(?:https?://|www\.)[^\s<>()]+",
        r"(?i)[A-Za-z0-9_.!#$%&'*+/=?^`{|}~-]+@[A-Za-z0-9_.-]+\.[a-z]{2,}",
        r"(?-u:\b)eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+(?-u:\b)",
        r"\+?(?:\p{Nd}[ ()+\-]*){6,}\p{Nd}",
    ];
    let mut result = text.to_owned();
    for pattern in patterns {
        result = Regex::new(pattern)
            .unwrap()
            .replace_all(&result, "[redacted]")
            .into_owned();
    }
    let secrets = Regex::new(r"(?i)((?:\b(?:password|passcode|(?:access[_ -]?)?token|secret|(?:verification )?code)\b|كلمة (?:المرور|السر)|رمز التحقق)\s*(?::|=|：|\bis\b|هو)\s*)[^\s,;]+").unwrap();
    result = secrets.replace_all(&result, "${1}[redacted]").into_owned();
    char::decode_utf16(result.encode_utf16().take(limit))
        .map(|unit| unit.unwrap_or('\u{fffd}'))
        .map(|c| if c == '\0' { '\u{fffd}' } else { c })
        .collect()
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
    response(status, json!({"error": message}))
}

pub(crate) fn check_ingest(
    method: &Method,
    headers: &HeaderMap,
    secret: Option<&str>,
) -> Result<(), StatusCode> {
    let supplied = headers
        .get(header::AUTHORIZATION)
        .map(|v| v.as_bytes())
        .unwrap_or_default();
    let expected = format!("Bearer {}", secret.unwrap_or_default());
    #[allow(deprecated)]
    if secret.is_none_or(str::is_empty)
        || ring::constant_time::verify_slices_are_equal(supplied, expected.as_bytes()).is_err()
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if *method != Method::POST {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    if !headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| {
            s.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

pub async fn ingest<C: GenericClient + Sync>(
    db: &C,
    body: &[u8],
) -> Result<Response, tokio_postgres::Error> {
    let Ok(mut turn) = serde_json::from_slice::<Turn>(body) else {
        return Ok(error(StatusCode::BAD_REQUEST, "Invalid request"));
    };
    if !valid(&turn) {
        return Ok(error(StatusCode::BAD_REQUEST, "Invalid request"));
    }
    turn.question = redact(&turn.question, 1000);
    turn.reply = redact(&turn.reply, 3000);
    let value = serde_json::to_value(&turn).expect("turn serializes");
    let rows = db.query("INSERT INTO telegram_conversation_turns (trace, conversation_id, turn) VALUES ($1, $2, $3::jsonb) ON CONFLICT (trace) DO UPDATE SET trace = EXCLUDED.trace WHERE telegram_conversation_turns.conversation_id = EXCLUDED.conversation_id AND telegram_conversation_turns.turn = EXCLUDED.turn RETURNING sequence", &[&turn.trace, &turn.conversation_id, &value]).await?;
    Ok(if rows.is_empty() {
        error(StatusCode::CONFLICT, "Conflict")
    } else {
        response(StatusCode::OK, json!({"ok": true}))
    })
}

pub async fn read<C: GenericClient + Sync>(
    db: &C,
    method: &Method,
    uri: &Uri,
) -> Result<Response, tokio_postgres::Error> {
    let params: Vec<(String, String)> =
        url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    let param = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let before = param("before");
    if before.is_some_and(|s| !matches(r"^[1-9][0-9]{0,18}$", s) || s.parse::<i64>().is_err()) {
        return Ok(error(StatusCode::BAD_REQUEST, "Invalid cursor"));
    }
    if *method != Method::GET {
        return Ok(error(StatusCode::NOT_FOUND, "Not found"));
    }
    if uri.path().ends_with("/conversations") {
        let rows = db.query("SELECT conversation_id, to_char(max(received_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') last_seen, count(*)::int turn_count, max(sequence)::text cursor FROM telegram_conversation_turns WHERE received_at >= now() - interval '3 days' GROUP BY conversation_id HAVING ($1::text IS NULL OR max(sequence) < $1::bigint) ORDER BY max(sequence) DESC LIMIT 31", &[&before]).await?;
        let items: Vec<Value> = rows.iter().take(30).map(|r| json!({"conversationId": r.get::<_, String>(0), "lastSeenAt": r.get::<_, String>(1), "turnCount": r.get::<_, i32>(2)})).collect();
        let cursor = if rows.len() > 30 {
            Some(rows[29].get::<_, String>(3))
        } else {
            None
        };
        return Ok(response(
            StatusCode::OK,
            json!({"items": items, "nextCursor": cursor}),
        ));
    }
    let Some(conversation) = param("conversation").filter(|s| matches(r"^[a-f0-9]{64}$", s)) else {
        return Ok(error(StatusCode::BAD_REQUEST, "Invalid conversation"));
    };
    let rows = db.query("SELECT turn, to_char(received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') received_at, sequence::text cursor FROM telegram_conversation_turns WHERE conversation_id = $1 AND received_at >= now() - interval '3 days' AND ($2::text IS NULL OR sequence < $2::bigint) ORDER BY sequence DESC LIMIT 51", &[&conversation, &before]).await?;
    let mut items = Vec::new();
    for row in rows.iter().take(50) {
        let value: Value = row.get(0);
        let Ok(turn) = serde_json::from_value::<Turn>(value) else {
            return Ok(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error",
            ));
        };
        if !valid(&turn) {
            return Ok(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error",
            ));
        }
        let mut value = serde_json::to_value(turn).expect("turn serializes");
        let object = value.as_object_mut().expect("turn is an object");
        object.remove("conversationId");
        object.insert("receivedAt".to_owned(), json!(row.get::<_, String>(1)));
        items.push(value);
    }
    let cursor = if rows.len() > 50 {
        Some(rows[49].get::<_, String>(2))
    } else {
        None
    };
    Ok(response(
        StatusCode::OK,
        json!({"items": items, "nextCursor": cursor}),
    ))
}
