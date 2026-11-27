use axum::{
    Json,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_postgres::GenericClient;

fn reply(status: StatusCode, value: Value) -> Response {
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
fn one(v: &Value, choices: &[&str]) -> bool {
    v.as_str().is_some_and(|s| choices.contains(&s))
}
fn uuid(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        let b = s.as_bytes();
        b.len() == 36
            && b.iter().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    *c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
            && ((b[14] == b'0'
                && b[19] == b'0'
                && b.iter()
                    .enumerate()
                    .all(|(i, c)| [8, 13, 18, 23].contains(&i) || *c == b'0'))
                || (b[14] == b'f'
                    && b[19] == b'f'
                    && b.iter().enumerate().all(|(i, c)| {
                        [8, 13, 18, 23].contains(&i) || c.eq_ignore_ascii_case(&b'f')
                    }))
                || ((b'1'..=b'8').contains(&b[14]) && b"89abAB".contains(&b[19])))
    })
}
fn event(v: &Value) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    let extras: &[&str] = match v["kind"].as_str() {
        Some("screen") => &[],
        Some("action") => &["action", "outcome"],
        Some("vital") => &["metric", "value"],
        Some("error") => &["code"],
        _ => return false,
    };
    let common = ["id", "journeyId", "step", "screen", "kind"];
    if obj.len() != common.len() + extras.len()
        || !common.iter().chain(extras).all(|k| obj.contains_key(*k))
        || !uuid(&v["id"])
        || !uuid(&v["journeyId"])
        || !v["step"]
            .as_f64()
            .is_some_and(|n| n.fract() == 0.0 && (1.0..=1_000_000.0).contains(&n))
        || !one(
            &v["screen"],
            &[
                "dashboard",
                "attendance",
                "roster",
                "summary",
                "student",
                "student-portal",
                "courses",
                "course-create",
                "course-edit",
                "course-students",
                "course-grades",
                "settings-accounts",
                "settings-students",
                "add-student",
                "profile",
                "history",
                "changes",
                "invitation",
            ],
        )
    {
        return false;
    }
    match v["kind"].as_str() {
        Some("screen") => true,
        Some("action") => {
            one(
                &v["action"],
                &[
                    "attendance",
                    "note",
                    "correction",
                    "student",
                    "account",
                    "course",
                    "enrollment",
                    "grade",
                    "profile",
                    "sync",
                    "filter",
                    "export",
                    "course-switch",
                    "sign-out",
                    "account-switch",
                ],
            ) && one(
                &v["outcome"],
                &["activated", "saved", "queued", "failed", "synced"],
            )
        }
        Some("vital") => {
            one(&v["metric"], &["LCP", "INP", "CLS"])
                && v["value"]
                    .as_f64()
                    .is_some_and(|n| (0.0..=600_000.0).contains(&n))
        }
        Some("error") => one(&v["code"], &["runtime", "promise"]),
        _ => false,
    }
}
static WRITES: OnceLock<Mutex<(u64, HashMap<String, u8>)>> = OnceLock::new();
fn accept(actor: &str) -> bool {
    let minute = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 60;
    let mut guard = WRITES
        .get_or_init(|| Mutex::new((minute, HashMap::new())))
        .lock()
        .unwrap();
    if guard.0 != minute {
        guard.0 = minute;
        guard.1.clear();
    }
    let count = guard.1.entry(actor.to_owned()).or_default();
    if *count >= 20 {
        false
    } else {
        *count += 1;
        true
    }
}
pub(crate) async fn ingest<C: GenericClient + Sync>(
    client: &C,
    actor_id: &str,
    role: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, tokio_postgres::Error> {
    let invalid = || reply(StatusCode::BAD_REQUEST, json!({"error": "Invalid request"}));
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .is_none_or(|s| !s.trim().eq_ignore_ascii_case("application/json"))
        || body.len() > 16384
    {
        return Ok(invalid());
    }
    let Ok(batch) = serde_json::from_slice::<Value>(body) else {
        return Ok(invalid());
    };
    if !batch
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.contains_key("events"))
        || !batch["events"]
            .as_array()
            .is_some_and(|events| (1..=30).contains(&events.len()) && events.iter().all(event))
    {
        return Ok(invalid());
    }
    if !accept(actor_id) {
        return Ok(reply(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": "Too many diagnostic requests"}),
        ));
    }
    client.execute("INSERT INTO website_diagnostics (actor_id, event_id, journey_id, role, event) SELECT $1, (e->>'id')::uuid, (e->>'journeyId')::uuid, $2, e FROM jsonb_array_elements($3::jsonb) e ON CONFLICT (actor_id, event_id) DO NOTHING", &[&actor_id, &role, &batch["events"]]).await?;
    Ok(reply(StatusCode::OK, json!({"ok": true})))
}

pub(crate) async fn read<C: GenericClient + Sync>(
    client: &C,
    uri: &axum::http::Uri,
) -> Result<Response, tokio_postgres::Error> {
    let mut params = HashMap::new();
    for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()) {
        params
            .entry(key.into_owned())
            .or_insert_with(|| value.into_owned());
    }
    let before: Option<i64> = match params.get("before") {
        None => None,
        Some(s)
            if !s.is_empty()
                && s.len() <= 19
                && !s.starts_with('0')
                && s.bytes().all(|b| b.is_ascii_digit()) =>
        {
            match s.parse::<i64>() {
                Ok(n) => Some(n),
                Err(_) => {
                    return Ok(reply(
                        StatusCode::BAD_REQUEST,
                        json!({"error":"Invalid cursor"}),
                    ));
                }
            }
        }
        _ => {
            return Ok(reply(
                StatusCode::BAD_REQUEST,
                json!({"error":"Invalid cursor"}),
            ));
        }
    };
    let actor = params.get("actor");
    let valid_actor = |s: &String| (1..=256).contains(&s.encode_utf16().count());
    if uri.path() == "/api/diagnostics/journeys" {
        if actor.is_some_and(|s| !valid_actor(s)) {
            return Ok(reply(
                StatusCode::BAD_REQUEST,
                json!({"error":"Invalid account"}),
            ));
        }
        let role = params.get("role");
        if role.is_some_and(|s| !["administrator", "teacher", "student"].contains(&s.as_str())) {
            return Ok(reply(
                StatusCode::BAD_REQUEST,
                json!({"error":"Invalid role"}),
            ));
        }
        let rows = client.query("WITH recent AS (SELECT d.actor_id, d.journey_id, (array_agg(d.role ORDER BY d.sequence DESC))[1] role, max(d.sequence) cursor, max(d.received_at) last_seen, count(*)::int event_count FROM website_diagnostics d WHERE NOT (received_at < now() - interval '30 days') AND ($1::text IS NULL OR d.actor_id = $1) GROUP BY d.actor_id, d.journey_id HAVING ($3::bigint IS NULL OR max(d.sequence) < $3::bigint) AND ($2::text IS NULL OR (array_agg(d.role ORDER BY d.sequence DESC))[1] = $2) ORDER BY max(d.sequence) DESC LIMIT 31) SELECT recent.actor_id, recent.journey_id::text journey_id, recent.role, recent.cursor::text cursor, to_char(recent.last_seen AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') last_seen, recent.event_count, u.name FROM recent LEFT JOIN neon_auth.\"user\" u ON u.id::text = recent.actor_id ORDER BY recent.cursor DESC", &[&actor, &role, &before]).await?;
        let items: Vec<Value> = rows.iter().take(30).map(|r| json!({"journeyId":r.get::<_, String>("journey_id"), "actorId":r.get::<_, String>("actor_id"), "actorName":r.get::<_, Option<String>>("name"), "role":r.get::<_, String>("role"), "lastSeenAt":r.get::<_, String>("last_seen"), "eventCount":r.get::<_, i32>("event_count")})).collect();
        let next: Option<String> = if rows.len() > 30 {
            Some(rows[29].get("cursor"))
        } else {
            None
        };
        return Ok(reply(
            StatusCode::OK,
            json!({"items":items, "nextCursor":next}),
        ));
    }
    if uri.path() == "/api/diagnostics/events" {
        let journey = params.get("journey");
        if !actor.is_some_and(valid_actor) || !journey.is_some_and(|s| uuid(&json!(s))) {
            return Ok(reply(
                StatusCode::BAD_REQUEST,
                json!({"error":"Invalid journey"}),
            ));
        }
        let rows = client.query("SELECT event, to_char(received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') received_at, sequence::text cursor FROM website_diagnostics WHERE actor_id = $1 AND journey_id = $2::text::uuid AND NOT (received_at < now() - interval '30 days') AND ($3::bigint IS NULL OR sequence < $3::bigint) ORDER BY sequence DESC LIMIT 51", &[&actor, &journey, &before]).await?;
        let items: Vec<Value> = rows.iter().take(50).map(|r| json!({"event":r.get::<_, Value>("event"), "receivedAt":r.get::<_, String>("received_at")})).collect();
        let next: Option<String> = if rows.len() > 50 {
            Some(rows[49].get("cursor"))
        } else {
            None
        };
        return Ok(reply(
            StatusCode::OK,
            json!({"items":items, "nextCursor":next}),
        ));
    }
    Ok(reply(StatusCode::NOT_FOUND, json!({"error":"Not found"})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_diagnostic_variants_and_rejects_extra_data() {
        let mut e = json!({"id":"5104c698-df87-4c15-8321-a3c7d12f3ba1", "journeyId":"aab5cc11-2dd2-451c-9411-84cfc46b7992", "step":1, "screen":"attendance", "kind":"screen"});
        assert!(event(&e));
        e["private"] = json!("name");
        assert!(!event(&e));
        e.as_object_mut().unwrap().remove("private");
        e["kind"] = json!("action");
        e["action"] = json!("attendance");
        e["outcome"] = json!("saved");
        assert!(event(&e));
        e["outcome"] = json!("unknown");
        assert!(!event(&e));
        e.as_object_mut().unwrap().remove("action");
        e.as_object_mut().unwrap().remove("outcome");
        e["kind"] = json!("vital");
        e["metric"] = json!("CLS");
        e["value"] = json!(0.1);
        assert!(event(&e));
        e["value"] = json!(600001);
        assert!(!event(&e));
    }
}
