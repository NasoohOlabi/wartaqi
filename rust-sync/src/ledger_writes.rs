use std::collections::HashSet;

use crate::{access::Grant, writes::Error};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

fn allowed(p: &Value, snap: &Value) -> bool {
    let students = snap["students"].as_array().unwrap();
    let events = snap["events"].as_array().unwrap();
    let assigned = |id: &Value| students.iter().any(|s| s["studentId"] == *id);
    match p["kind"].as_str() {
        Some("note") => assigned(&p["studentId"]),
        Some("attendance") => {
            assigned(&p["studentId"])
                && p.get("enrollmentId").is_some()
                && snap["enrollments"].as_array().unwrap().iter().any(|e| {
                    e["id"] == p["enrollmentId"]
                        && e["studentId"] == p["studentId"]
                        && e["active"] == true
                        && snap["courses"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|c| c["id"] == e["courseId"] && c["open"] == true)
                })
        }
        Some("reversal") => events.iter().any(|e| {
            e["id"] == p["targetId"]
                && matches!(e["payload"]["kind"].as_str(), Some("attendance" | "note"))
                && assigned(&e["payload"]["studentId"])
        }),
        _ => false,
    }
}
fn version(events: &[Value], p: &Value) -> Value {
    let mut seen = HashSet::new();
    let mut reversed = HashSet::new();
    for event in events {
        let payload = &event["payload"];
        if payload["kind"] == "reversal" {
            if seen.contains(&payload["targetId"]) {
                reversed.insert(&payload["targetId"]);
            }
        } else {
            seen.insert(&event["id"]);
        }
    }
    let latest = events.iter().rev().find(|event| {
        let payload = &event["payload"];
        payload["kind"] == "attendance"
            && payload["studentId"] == p["studentId"]
            && payload.get("enrollmentId") == p.get("enrollmentId")
            && payload["date"] == p["date"]
            && !reversed.contains(&event["id"])
    });
    latest.map_or(Value::Null, |event| {
        let status = &event["payload"]["status"];
        let status = if status == "early" || status == "late" {
            json!("present")
        } else {
            status.clone()
        };
        json!({"eventId": event["id"], "status": status})
    })
}
async fn insert(
    tx: &Transaction<'_>,
    grant: &Grant,
    event: &Value,
    student: &Value,
    batch: bool,
) -> Result<u64, Error> {
    let id = event["id"].as_str().unwrap();
    let teacher = grant.id();
    let student_id = student["studentId"].as_str().ok_or(Error::Forbidden)?;
    let time = event["clientTime"].as_str().unwrap();
    let payload = &event["payload"];
    let name = grant.name();
    let enrollment = if student["kind"] == "attendance" {
        student.get("enrollmentId").and_then(Value::as_str)
    } else {
        None
    };
    let conflict = if batch {
        " ON CONFLICT (id) DO NOTHING"
    } else {
        ""
    };
    let sql = format!(
        "INSERT INTO events (id, teacher_id, student_id, actor, client_time, payload, enrollment_id) VALUES ($1::text::uuid,$2::text::uuid,$3::text::uuid,$4,$5::text::timestamptz,$6,$7::text::uuid){conflict}"
    );
    Ok(tx
        .execute(
            &sql,
            &[
                &id,
                &teacher,
                &student_id,
                &name,
                &time,
                &payload,
                &enrollment,
            ],
        )
        .await?)
}

async fn same_event(
    tx: &Transaction<'_>,
    grant: &Grant,
    event: &Value,
    student: &Value,
    actor_check: bool,
) -> Result<bool, Error> {
    let id = event["id"].as_str().ok_or(Error::Invalid)?;
    let teacher = grant.id();
    let student_id = student["studentId"].as_str().ok_or(Error::Forbidden)?;
    let time = event["clientTime"].as_str().ok_or(Error::Invalid)?;
    let payload = &event["payload"];
    let name = grant.name();
    let same = tx.query_opt("SELECT 1 FROM events WHERE id=$1::text::uuid AND teacher_id=$2::text::uuid AND student_id=$3::text::uuid AND client_time=$4::text::timestamptz AND payload=$5 AND (NOT $6::boolean OR actor=$7)", &[&id,&teacher,&student_id,&time,&payload,&actor_check,&name]).await?;
    Ok(same.is_some())
}
pub(crate) async fn apply(
    tx: &Transaction<'_>,
    path: &str,
    subject: &str,
    grant: &Grant,
    input: Value,
) -> Result<Value, Error> {
    let sync = path == "/api/attendance/sync";
    let records = input[if sync { "records" } else { "events" }]
        .as_array()
        .ok_or(Error::Invalid)?;
    if !(1..=500).contains(&records.len()) {
        return Err(Error::Invalid);
    }
    let mut parsed = Vec::new();
    for record in records {
        let event = crate::ledger::decode_input(if sync { &record["event"] } else { record })
            .map_err(|_| Error::Invalid)?;
        if sync {
            let p = &event["payload"];
            if p["kind"] != "attendance"
                || p.get("enrollmentId").is_none()
                || matches!(p["status"].as_str(), Some("early" | "late"))
                || !(record.get("expectedEventId").is_some_and(|v| {
                    v.is_null() || v.as_str().is_some_and(crate::ledger::valid_uuid)
                }))
                || record
                    .get("expectedRevision")
                    .is_some_and(|v| v != "unknown")
            {
                return Err(Error::Invalid);
            }
        }
        parsed.push(event);
    }
    if sync
        && input["accountId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(Error::Invalid)?
            != subject
    {
        return Err(Error::Forbidden);
    }
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtextextended('attendance-ledger', 0))",
        &[],
    )
    .await?;
    let mut snap = crate::staff_reads::snapshot(tx, grant, false)
        .await
        .map_err(Error::Other)?;
    if sync {
        let mut outcomes = Vec::new();
        for (event, record) in parsed.iter().zip(records) {
            let p = &event["payload"];
            let current = version(snap["events"].as_array().unwrap(), p);
            if !allowed(p, &snap) {
                outcomes.push(json!({"id":event["id"],"kind":"forbidden","current":null}));
                continue;
            }
            let id = event["id"].as_str().unwrap();
            let exists = tx
                .query_opt("SELECT 1 FROM events WHERE id=$1::text::uuid", &[&id])
                .await?
                .is_some();
            if exists {
                let same = same_event(tx, grant, event, p, false).await?;
                outcomes.push(json!({"id":id,"kind":if same {"saved"} else {"id_conflict"},"current":current}));
                continue;
            }
            let stale = if record["expectedRevision"] == "unknown" {
                !current.is_null()
            } else {
                current.get("eventId").unwrap_or(&Value::Null) != &record["expectedEventId"]
            };
            if stale && current.get("status").unwrap_or(&Value::Null) != &p["status"] {
                outcomes.push(json!({"id":id,"kind":"conflict","current":current}));
                continue;
            }
            insert(tx, grant, event, p, false).await?;
            snap["events"].as_array_mut().unwrap().push(event.clone());
            outcomes.push(
                json!({"id":id,"kind":"saved","current":{"eventId":id,"status":p["status"]}}),
            );
        }
        return Ok(json!({"outcomes":outcomes}));
    }
    if parsed.iter().any(|event| event["actor"] != grant.name()) {
        return Err(Error::Forbidden);
    }
    for event in &parsed {
        if !snap["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == event["id"])
            && !allowed(&event["payload"], &snap)
        {
            return Err(Error::Forbidden);
        }
    }
    for event in &parsed {
        let p = &event["payload"];
        let student = if p["kind"] == "reversal" {
            &snap["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == p["targetId"])
                .ok_or(Error::Forbidden)?["payload"]
        } else {
            p
        };
        if insert(tx, grant, event, student, true).await? == 0
            && !same_event(tx, grant, event, student, true).await?
        {
            return Err(Error::Conflict);
        }
    }
    if grant.can_read() {
        crate::staff_reads::snapshot(tx, grant, true)
            .await
            .map_err(Error::Other)
    } else {
        Ok(json!({"saved":parsed.len()}))
    }
}
