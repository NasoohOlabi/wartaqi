use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use tokio_postgres::{Row, Transaction};

type Error = Box<dyn std::error::Error + Send + Sync>;
pub(crate) const VISIBLE_LEDGER_EVENT: &str = "(e.payload->>'kind' = 'contact_receipt' AND e.payload = jsonb_build_object('kind', 'contact_receipt', 'fingerprint', e.payload->>'fingerprint') AND e.payload->>'fingerprint' ~ '^[a-f0-9]{64}$') IS NOT TRUE";

#[derive(Debug)]
pub(crate) struct InvalidData;

impl std::fmt::Display for InvalidData {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid notebook data")
    }
}

impl std::error::Error for InvalidData {}

pub(crate) fn js_trim(value: &str) -> &str {
    value.trim_matches(|character: char| {
        (character.is_whitespace() && character != '\u{0085}') || character == '\u{feff}'
    })
}

pub(crate) fn valid_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    (s == "00000000-0000-0000-0000-000000000000" || s == "ffffffff-ffff-ffff-ffff-ffffffffffff")
        || b.len() == 36
            && b.iter().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    *c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
            && matches!(b[14], b'1'..=b'8')
            && matches!(b[19], b'8' | b'9' | b'a' | b'b' | b'A' | b'B')
}
fn date(s: &str) -> bool {
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
    let year: u32 = s[0..4].parse().unwrap_or(0);
    let month: usize = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    let days = [
        0,
        31,
        if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) {
            29
        } else {
            28
        },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    month > 0 && month <= 12 && day > 0 && day <= days[month]
}
pub(crate) fn iso_datetime(value: &str) -> bool {
    let Some((day, time)) = value.strip_suffix('Z').and_then(|v| v.split_once('T')) else {
        return false;
    };
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    if !date(day) || fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut sections = clock.split(':');
    [23, 59, 59].into_iter().all(|max| {
        sections.next().is_some_and(|section| {
            section.len() == 2
                && section.bytes().all(|b| b.is_ascii_digit())
                && section.parse::<u8>().is_ok_and(|n| n <= max)
        })
    }) && sections.next().is_none()
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str, InvalidData> {
    v.get(key).and_then(Value::as_str).ok_or(InvalidData)
}
fn bounded(s: &str, min: usize, max: usize) -> bool {
    let n = s.encode_utf16().count();
    n >= min && n <= max
}
fn clean(v: &Value, key: &str, min: usize, max: usize) -> Result<String, InvalidData> {
    let s = js_trim(str_field(v, key)?);
    if bounded(s, min, max) {
        Ok(s.to_owned())
    } else {
        Err(InvalidData)
    }
}
fn id(v: &Value, key: &str) -> Result<String, InvalidData> {
    let s = str_field(v, key)?;
    if valid_uuid(s) {
        Ok(s.to_owned())
    } else {
        Err(InvalidData)
    }
}
fn payload(v: &Value) -> Result<Value, InvalidData> {
    let kind = str_field(v, "kind")?;
    match kind {
        "student" => Ok(
            json!({"kind":kind,"studentId":id(v,"studentId")?,"name":clean(v,"name",1,200)?,"group":clean(v,"group",0,100)?,"active":v.get("active").and_then(Value::as_bool).ok_or(InvalidData)?}),
        ),
        "attendance" => {
            let day = str_field(v, "date")?;
            if !date(day) {
                return Err(InvalidData);
            }
            let status = v.get("status").ok_or(InvalidData)?;
            if !status.is_null()
                && !matches!(
                    status.as_str(),
                    Some("present" | "absent" | "excused" | "early" | "late")
                )
            {
                return Err(InvalidData);
            }
            let mut result =
                json!({"kind":kind,"studentId":id(v,"studentId")?,"date":day,"status":status});
            if v.get("enrollmentId").is_some() {
                result["enrollmentId"] = id(v, "enrollmentId")?.into()
            }
            Ok(result)
        }
        "note" => {
            let student = str_field(v, "studentId")?;
            let day = str_field(v, "date")?;
            let tags = v.get("tags").and_then(Value::as_array).ok_or(InvalidData)?;
            if !(student.is_empty() || valid_uuid(student))
                || !date(day)
                || !(1..=20).contains(&tags.len())
            {
                return Err(InvalidData);
            }
            let mut cleaned = Vec::new();
            for tag in tags {
                let s = js_trim(tag.as_str().ok_or(InvalidData)?);
                if !bounded(s, 1, 60) {
                    return Err(InvalidData);
                }
                cleaned.push(s.to_owned());
            }
            Ok(
                json!({"kind":kind,"studentId":student,"date":day,"tags":cleaned,"text":clean(v,"text",1,10000)?}),
            )
        }
        "reversal" => {
            Ok(json!({"kind":kind,"targetId":id(v,"targetId")?,"reason":clean(v,"reason",1,1000)?}))
        }
        _ => Err(InvalidData),
    }
}

pub(crate) fn decode_input(input: &Value) -> Result<Value, InvalidData> {
    let client_time = str_field(input, "clientTime")?;
    if !iso_datetime(client_time) {
        return Err(InvalidData);
    }
    Ok(json!({
        "id": id(input, "id")?,
        "actor": clean(input, "actor", 1, 100)?,
        "clientTime": client_time,
        "receivedAt": str_field(input, "receivedAt")?,
        "payload": payload(input.get("payload").ok_or(InvalidData)?)?,
    }))
}

pub(crate) fn decode_event(row: &Row) -> Result<Value, Error> {
    let event_id: String = row.try_get("id")?;
    let actor: String = row.try_get("actor")?;
    let client_time: String = row.try_get("client_time")?;
    let received_at: String = row.try_get("received_at")?;
    let raw: Value = row.try_get("payload")?;
    Ok(decode_input(
        &json!({"id":event_id,"actor":actor,"clientTime":client_time,"receivedAt":received_at,"payload":raw}),
    )?)
}

fn project(events: &[Value], courses: &HashMap<String, String>) -> Vec<Value> {
    let mut unique: HashMap<&str, &Value> = HashMap::new();
    let mut ordered = Vec::new();
    let mut reversed = HashSet::new();
    for event in events {
        let event_id = event["id"].as_str().unwrap();
        if unique.contains_key(event_id) {
            continue;
        }
        let p = &event["payload"];
        if p["kind"] == "reversal"
            && let Some(target_id) = p["targetId"].as_str()
            && unique
                .get(target_id)
                .is_some_and(|target| target["payload"]["kind"] != "reversal")
        {
            reversed.insert(target_id);
        }
        unique.insert(event_id, event);
        ordered.push(event);
    }
    let mut versions: Vec<Value> = Vec::new();
    let mut indices = HashMap::new();
    for event in ordered {
        let p = &event["payload"];
        if p["kind"] != "attendance" || reversed.contains(event["id"].as_str().unwrap()) {
            continue;
        }
        let key = (
            p["studentId"].as_str().unwrap(),
            p.get("enrollmentId").and_then(Value::as_str).unwrap_or(""),
            p["date"].as_str().unwrap(),
        );
        let status = p["status"].as_str();
        let mut entry = if let Some(status) = status {
            json!({"date":key.2,"status":if status == "early" || status == "late" {"present"} else {status}})
        } else {
            Value::Null
        };
        if !entry.is_null() && !key.1.is_empty() {
            entry["enrollmentId"] = key.1.into();
            entry["course"] = courses.get(key.1).map_or(Value::Null, |name| json!(name));
        }
        if let Some(&index) = indices.get(&key) {
            versions[index] = entry
        } else {
            indices.insert(key, versions.len());
            versions.push(entry)
        }
    }
    versions.into_iter().filter(|v| !v.is_null()).collect()
}
pub(crate) async fn student_attendance(
    tx: &Transaction<'_>,
    student_id: &str,
    name: &str,
    group: &str,
) -> Result<Value, Error> {
    let sql = format!(
        "SELECT e.id::text AS id, e.actor::text AS actor, to_char(e.client_time AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS client_time, to_char(e.received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS received_at, e.payload FROM events e WHERE e.student_id = $1::text::uuid AND {VISIBLE_LEDGER_EVENT} ORDER BY e.sequence"
    );
    let rows = tx.query(&sql, &[&student_id]).await?;
    let events = rows
        .iter()
        .map(decode_event)
        .collect::<Result<Vec<_>, _>>()?;
    let rows = tx.query("SELECT ce.id::text AS id, c.name FROM course_enrollments ce JOIN courses c ON c.id = ce.course_id WHERE ce.student_id = $1::text::uuid", &[&student_id]).await?;
    let mut courses = HashMap::new();
    for row in rows {
        courses.insert(row.try_get("id")?, row.try_get("name")?);
    }
    Ok(
        json!({"student":{"studentId":student_id,"name":name,"group":group},"attendance":project(&events,&courses)}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_resolves_reversals_clears_and_scopes() {
        let e = |id: &str, payload: Value| json!({"id":id,"payload":payload});
        let mark = |status: Value, enrollment: Option<&str>| {
            let mut p =
                json!({"kind":"attendance","studentId":"s","date":"2026-01-01","status":status});
            if let Some(enrollment) = enrollment {
                p["enrollmentId"] = enrollment.into()
            }
            p
        };
        let events = vec![
            e("1", mark(json!("late"), None)),
            e("2", mark(json!("absent"), None)),
            e("3", json!({"kind":"reversal","targetId":"2"})),
            e("4", mark(json!("excused"), Some("c"))),
            e("5", mark(Value::Null, Some("c"))),
            e("6", json!({"kind":"reversal","targetId":"5"})),
            e("7", json!({"kind":"note","text":"private"})),
        ];
        assert_eq!(
            project(&events, &HashMap::from([("c".into(), "Course".into())])),
            vec![
                json!({"date":"2026-01-01","status":"present"}),
                json!({"date":"2026-01-01","status":"excused","enrollmentId":"c","course":"Course"})
            ]
        );
        assert!(payload(&json!({"kind":"attendance","studentId":"bad","date":"2026-01-01","status":"present"})).is_err());
    }
}
