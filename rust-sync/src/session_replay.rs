use axum::{
    Json,
    http::{HeaderMap, Method, StatusCode, Uri, header},
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
fn invalid() -> Response {
    reply(StatusCode::BAD_REQUEST, json!({"error":"Invalid request"}))
}
fn object(v: &Value, required: &[&str], optional: &[&str]) -> bool {
    v.as_object().is_some_and(|m| {
        required.iter().all(|k| m.contains_key(*k))
            && m.keys()
                .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
    })
}
fn num(v: &Value, min: u64, max: u64) -> bool {
    v.as_u64().is_some_and(|n| (min..=max).contains(&n))
}
fn text(v: &Value, min: usize, max: usize) -> bool {
    v.as_str()
        .is_some_and(|s| (min..=max).contains(&s.encode_utf16().count()))
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
            && (b[14] == b'0'
                && b[19] == b'0'
                && b.iter()
                    .enumerate()
                    .all(|(i, c)| [8, 13, 18, 23].contains(&i) || *c == b'0')
                || b[14] == b'f'
                    && b[19] == b'f'
                    && b.iter().enumerate().all(|(i, c)| {
                        [8, 13, 18, 23].contains(&i) || c.eq_ignore_ascii_case(&b'f')
                    })
                || (b'1'..=b'8').contains(&b[14]) && b"89abAB".contains(&b[19]))
    })
}
fn status(v: &Value) -> bool {
    v.is_null() || one(v, &["present", "absent", "excused"])
}
fn date(v: &Value) -> bool {
    let Some(s) = v.as_str() else { return false };
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let y: u32 = s[0..4].parse().unwrap();
    let m: u32 = s[5..7].parse().unwrap();
    let d: u32 = s[8..10].parse().unwrap();
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) => 29,
        2 => 28,
        _ => 0,
    };
    d >= 1 && d <= days
}
fn attendance(v: &Value) -> bool {
    if !object(
        v,
        &[
            "courseName",
            "date",
            "rows",
            "totalRows",
            "pendingCount",
            "unmarkedCount",
            "unmarkedOnly",
            "canCorrect",
            "dialog",
        ],
        &[],
    ) || !text(&v["courseName"], 0, 200)
        || !date(&v["date"])
        || !num(&v["totalRows"], 0, u64::MAX)
        || !num(&v["pendingCount"], 0, u64::MAX)
        || !num(&v["unmarkedCount"], 0, u64::MAX)
        || !v["unmarkedOnly"].is_boolean()
        || !v["canCorrect"].is_boolean()
    {
        return false;
    }
    let Some(rows) = v["rows"].as_array() else {
        return false;
    };
    if rows.len() > 150 || v["totalRows"].as_u64().unwrap() < rows.len() as u64 {
        return false;
    }
    if !rows.iter().all(|r| {
        object(
            r,
            &["id", "name", "level", "teacherName", "saved", "draft"],
            &[],
        ) && uuid(&r["id"])
            && text(&r["name"], 1, 200)
            && text(&r["level"], 0, 200)
            && text(&r["teacherName"], 0, 200)
            && status(&r["saved"])
            && status(&r["draft"])
    }) {
        return false;
    }
    let d = &v["dialog"];
    d.is_null()
        || (object(d, &["kind", "name"], &[])
            && one(&d["kind"], &["note", "correction"])
            && text(&d["name"], 1, 200))
}
fn action(v: &Value) -> bool {
    match v["kind"].as_str() {
        Some("mark") => {
            object(v, &["kind", "studentId", "status"], &[])
                && uuid(&v["studentId"])
                && !v["status"].is_null()
                && status(&v["status"])
        }
        Some("student" | "note" | "correction") => {
            object(v, &["kind", "studentId"], &[]) && uuid(&v["studentId"])
        }
        Some("save" | "bulk" | "unmarked" | "date") => object(v, &["kind"], &[]),
        _ => false,
    }
}
fn frame(v: &Value) -> bool {
    let click = v["kind"] == "click";
    if !object(
        v,
        &[
            "kind", "step", "timeMs", "screen", "width", "height", "theme", "language", "boxes",
        ],
        if click {
            &["click", "attendance", "attendanceAction"]
        } else {
            &["attendance"]
        },
    ) || !one(&v["kind"], &["view", "click"])
        || !num(&v["step"], 1, 600)
        || !num(&v["timeMs"], 0, 86400000)
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
        || !num(&v["width"], 1, 8192)
        || !num(&v["height"], 1, 8192)
        || !one(&v["theme"], &["light", "dark"])
        || !one(&v["language"], &["en", "ar"])
    {
        return false;
    }
    let w = v["width"].as_u64().unwrap();
    let h = v["height"].as_u64().unwrap();
    let Some(boxes) = v["boxes"].as_array() else {
        return false;
    };
    if boxes.len() > 120
        || !boxes.iter().all(|b| {
            object(b, &["x", "y", "w", "h", "kind"], &["fill", "label"])
                && num(&b["x"], 0, 8192)
                && num(&b["y"], 0, 8192)
                && num(&b["w"], 1, 8192)
                && num(&b["h"], 1, 8192)
                && b["x"].as_u64().unwrap() + b["w"].as_u64().unwrap() <= w
                && b["y"].as_u64().unwrap() + b["h"].as_u64().unwrap() <= h
                && one(
                    &b["kind"],
                    &["surface", "line", "text", "control", "field", "image"],
                )
                && b.get("fill").is_none_or(|f| {
                    f.as_str().is_some_and(|s| {
                        s.len() == 7
                            && s.starts_with('#')
                            && s.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
                    })
                })
                && b.get("label").is_none_or(|l| {
                    one(
                        l,
                        &[
                            "dashboard",
                            "attendance",
                            "students",
                            "courses",
                            "settings",
                            "profile",
                            "take-attendance",
                            "add-student",
                            "add-note",
                            "save-note",
                            "save-changes",
                            "choose-course",
                            "find-student",
                            "attendance-and-notes",
                            "sign-out",
                            "back",
                            "cancel",
                            "refresh",
                            "filter",
                            "sort",
                            "present",
                            "absent",
                            "excused",
                        ],
                    )
                })
        })
    {
        return false;
    }
    if let Some(a) = v.get("attendance")
        && (v["screen"] != "attendance" || !attendance(a))
    {
        return false;
    }
    if click {
        let c = &v["click"];
        if !object(c, &["x", "y"], &[])
            || !num(&c["x"], 0, w)
            || !num(&c["y"], 0, h)
            || v.get("attendanceAction")
                .is_some_and(|a| v.get("attendance").is_none() || !action(a))
        {
            return false;
        }
    }
    true
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
    if *count >= 60 {
        false
    } else {
        *count += 1;
        true
    }
}
pub(crate) async fn handle<C: GenericClient + Sync>(
    client: &C,
    actor_id: &str,
    role: &str,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, tokio_postgres::Error> {
    if method == Method::POST {
        if headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(';').next())
            .is_none_or(|s| !s.trim().eq_ignore_ascii_case("application/json"))
            || body.len() > 48 * 1024
        {
            return Ok(invalid());
        }
        let Ok(batch) = serde_json::from_slice::<Value>(body) else {
            return Ok(invalid());
        };
        if !object(&batch, &["journeyId", "frames"], &[])
            || !uuid(&batch["journeyId"])
            || !batch["frames"]
                .as_array()
                .is_some_and(|f| (1..=3).contains(&f.len()) && f.iter().all(frame))
        {
            return Ok(invalid());
        }
        if !accept(actor_id) {
            return Ok(reply(
                StatusCode::TOO_MANY_REQUESTS,
                json!({"error":"Too many replay requests"}),
            ));
        }
        let journey = batch["journeyId"].as_str().unwrap();
        client.execute("INSERT INTO website_session_replay (actor_id, journey_id, step, frame, attendance_payload) SELECT $1, $2::text::uuid, (f->>'step')::int, f - 'attendance' - 'attendanceAction', CASE WHEN f ? 'attendance' THEN jsonb_build_object('attendance', f->'attendance') || CASE WHEN f ? 'attendanceAction' THEN jsonb_build_object('attendanceAction', f->'attendanceAction') ELSE '{}'::jsonb END END FROM jsonb_array_elements($3::jsonb) f ON CONFLICT (actor_id, journey_id, step) DO NOTHING", &[&actor_id,&journey,&batch["frames"]]).await?;
        return Ok(reply(StatusCode::OK, json!({"ok":true})));
    }
    if method != Method::GET {
        return Ok(reply(StatusCode::NOT_FOUND, json!({"error":"Not found"})));
    }
    if role != "administrator" {
        return Ok(reply(StatusCode::FORBIDDEN, json!({"error":"Forbidden"})));
    }
    let mut params = HashMap::new();
    for (k, v) in url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()) {
        if !["actor", "journey", "after"].contains(&k.as_ref())
            || params.insert(k.into_owned(), v.into_owned()).is_some()
        {
            return Ok(reply(
                StatusCode::BAD_REQUEST,
                json!({"error":"Invalid query"}),
            ));
        }
    }
    let (Some(actor), Some(journey)) = (params.get("actor"), params.get("journey")) else {
        return Ok(reply(
            StatusCode::BAD_REQUEST,
            json!({"error":"Invalid query"}),
        ));
    };
    let after = match params.get("after") {
        None => 0,
        Some(s)
            if !s.is_empty()
                && s.len() <= 3
                && !s.starts_with('0')
                && s.bytes().all(|b| b.is_ascii_digit()) =>
        {
            s.parse::<i32>().unwrap_or(0)
        }
        _ => -1,
    };
    if !text(&json!(actor), 1, 256) || !uuid(&json!(journey)) || !(0..=600).contains(&after) {
        return Ok(reply(
            StatusCode::BAD_REQUEST,
            json!({"error":"Invalid query"}),
        ));
    }
    let rows=client.query("SELECT frame || COALESCE(attendance_payload, '{}'::jsonb) AS frame FROM website_session_replay WHERE actor_id = $1 AND journey_id = $2::text::uuid AND step > $3 AND NOT (received_at < now() - interval '3 days') ORDER BY step LIMIT 21", &[actor,journey,&after]).await?;
    let items: Vec<Value> = rows.iter().take(20).map(|r| r.get("frame")).collect();
    if items.iter().any(|item| !frame(item)) {
        return Ok(reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Diagnostics unavailable"}),
        ));
    }
    let next = if rows.len() > 20 {
        items.last().unwrap()["step"].clone()
    } else {
        Value::Null
    };
    Ok(reply(
        StatusCode::OK,
        json!({"items":items,"nextAfter":next}),
    ))
}
