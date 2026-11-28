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
async fn sync_scope(
    tx: &Transaction<'_>,
    grant: &Grant,
    parsed: &[Value],
) -> Result<HashSet<(String, String)>, Error> {
    let students: Vec<&str> = parsed
        .iter()
        .map(|e| e["payload"]["studentId"].as_str().unwrap())
        .collect();
    let enrollments: Vec<&str> = parsed
        .iter()
        .map(|e| e["payload"]["enrollmentId"].as_str().unwrap())
        .collect();
    let teacher = grant.id();
    let admin = grant.is_admin();
    // Match the snapshot lock order: students, enrollments, then courses.
    tx.query("SELECT id FROM students WHERE id::text = ANY($1) AND active AND ($2 OR teacher_id=$3::text::uuid) ORDER BY name,id FOR SHARE", &[&students, &admin, &teacher]).await?;
    let rows = tx.query("SELECT ce.id::text, ce.student_id::text, ce.course_id::text, ce.active FROM course_enrollments ce JOIN students s ON s.id=ce.student_id WHERE ce.id::text = ANY($1) AND s.id::text = ANY($2) AND s.active AND ($3 OR s.teacher_id=$4::text::uuid) ORDER BY ce.id FOR SHARE OF ce", &[&enrollments, &students, &admin, &teacher]).await?;
    let courses: Vec<String> = rows.iter().map(|r| r.get(2)).collect();
    let open: HashSet<String> = tx.query("SELECT id::text, open FROM courses WHERE id::text = ANY($1) ORDER BY btrim(name) COLLATE \"en-US-x-icu\", id FOR SHARE", &[&courses]).await?.iter().filter(|r| r.get::<_, bool>(1)).map(|r| r.get(0)).collect();
    Ok(rows
        .iter()
        .filter(|r| r.get::<_, bool>(3) && open.contains(&r.get::<_, String>(2)))
        .map(|r| (r.get(1), r.get(0)))
        .collect())
}

async fn sync_version(tx: &Transaction<'_>, p: &Value) -> Result<Value, Error> {
    let student = p["studentId"].as_str().unwrap();
    let enrollment = p["enrollmentId"].as_str().unwrap();
    let date = p["date"].as_str().unwrap();
    let rows = tx.query("WITH candidates AS (SELECT id FROM events WHERE student_id=$1::text::uuid AND enrollment_id=$2::text::uuid AND payload->>'kind'='attendance' AND payload->>'date'=$3) SELECT e.id::text AS id, e.actor::text AS actor, to_char(e.client_time AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS client_time, to_char(e.received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS received_at, wartaq_event_payload(e.payload,e.payload_packed,e.student_id,e.enrollment_id) AS payload FROM events e WHERE e.student_id=$1::text::uuid AND (e.id IN (SELECT id FROM candidates) OR (e.payload->>'kind'='reversal' AND e.payload->>'targetId' IN (SELECT id::text FROM candidates))) ORDER BY e.sequence", &[&student, &enrollment, &date]).await?;
    let events = rows
        .iter()
        .map(crate::ledger::decode_event)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| Error::Other(e.into()))?;
    Ok(version(&events, p))
}
async fn insert(
    tx: &Transaction<'_>,
    grant: &Grant,
    event: &Value,
    student: &Value,
    batch: bool,
    edit_time: Option<&str>,
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
        "INSERT INTO events (id, teacher_id, student_id, actor, client_time, payload, enrollment_id, edit_time) VALUES ($1::text::uuid,$2::text::uuid,$3::text::uuid,$4,$5::text::timestamptz,$6,$7::text::uuid,$8::text::timestamptz){conflict}"
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
                &edit_time,
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
    edit_time: Option<&str>,
) -> Result<bool, Error> {
    let id = event["id"].as_str().ok_or(Error::Invalid)?;
    let teacher = grant.id();
    let student_id = student["studentId"].as_str().ok_or(Error::Forbidden)?;
    let time = event["clientTime"].as_str().ok_or(Error::Invalid)?;
    let payload = &event["payload"];
    let name = grant.name();
    let same = tx.query_opt("SELECT 1 FROM events WHERE id=$1::text::uuid AND teacher_id=$2::text::uuid AND student_id=$3::text::uuid AND client_time=$4::text::timestamptz AND wartaq_event_payload(payload, payload_packed, student_id, enrollment_id)=$5 AND (NOT $6::boolean OR actor=$7) AND (($6::boolean AND edit_time IS NULL) OR edit_time IS NOT DISTINCT FROM $8::text::timestamptz)", &[&id,&teacher,&student_id,&time,&payload,&actor_check,&name,&edit_time]).await?;
    Ok(same.is_some())
}
async fn stale_edit(
    tx: &Transaction<'_>,
    event: &Value,
    student: &Value,
    edit_time: &str,
) -> Result<bool, Error> {
    let id = event["id"].as_str().unwrap();
    let student_id = student["studentId"].as_str().unwrap();
    let enrollment = student.get("enrollmentId").and_then(Value::as_str);
    let date = student["date"].as_str().unwrap();
    Ok(tx.query_one("SELECT $1::text::timestamptz > clock_timestamp() + interval '5 minutes' OR EXISTS (SELECT 1 FROM events e LEFT JOIN events target ON e.payload->>'kind' = 'reversal' AND target.id::text = e.payload->>'targetId' WHERE e.student_id = $2::text::uuid AND e.enrollment_id IS NOT DISTINCT FROM $3::text::uuid AND COALESCE(target.payload, e.payload)->>'kind' = 'attendance' AND COALESCE(target.payload, e.payload)->>'date' = $4 AND (COALESCE(e.edit_time, e.received_at), e.id) >= ($1::text::timestamptz, $5::text::uuid))", &[&edit_time, &student_id, &enrollment, &date, &id]).await?.get(0))
}
pub(crate) async fn correct(
    tx: &Transaction<'_>,
    grant: &Grant,
    input: Value,
) -> Result<Value, Error> {
    if !grant.is_admin() || !grant.can_read() || !grant.can_write() {
        return Err(Error::Forbidden);
    }
    let records = input["events"].as_array().ok_or(Error::Invalid)?;
    if !(1..=500).contains(&records.len()) {
        return Err(Error::Invalid);
    }
    let mut parsed = Vec::with_capacity(records.len());
    for record in records {
        let event = crate::ledger::decode_input(record).map_err(|_| Error::Invalid)?;
        if event["payload"]["kind"] != "reversal" {
            return Err(Error::Invalid);
        }
        if event["actor"] != grant.name() {
            return Err(Error::Forbidden);
        }
        parsed.push(event);
    }
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtextextended('attendance-ledger', 0))",
        &[],
    )
    .await?;
    let targets: Vec<&str> = parsed
        .iter()
        .map(|e| e["payload"]["targetId"].as_str().unwrap())
        .collect();
    // Lock target students in the same order as the legacy snapshot, including inactive rows.
    tx.query("SELECT s.id FROM students s WHERE EXISTS (SELECT 1 FROM events e WHERE e.id::text=ANY($1) AND (e.student_id=s.id OR wartaq_event_payload(e.payload,e.payload_packed,e.student_id,e.enrollment_id)->>'studentId'=s.id::text)) ORDER BY s.name,s.id FOR SHARE", &[&targets]).await?;
    let rows = tx.query("SELECT e.id::text AS id, e.actor::text AS actor, to_char(e.client_time AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS client_time, to_char(e.received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS received_at, wartaq_event_payload(e.payload,e.payload_packed,e.student_id,e.enrollment_id) AS payload, (s.active AND EXISTS (SELECT 1 FROM students assigned WHERE assigned.id::text=wartaq_event_payload(e.payload,e.payload_packed,e.student_id,e.enrollment_id)->>'studentId' AND assigned.active)) AS student_active FROM events e JOIN students s ON s.id=e.student_id WHERE e.id::text=ANY($1)", &[&targets]).await?;
    let mut targets_by_id = std::collections::HashMap::new();
    for row in &rows {
        let event = crate::ledger::decode_event(row).map_err(Error::Other)?;
        targets_by_id.insert(
            event["id"].as_str().unwrap().to_owned(),
            (
                event["payload"].clone(),
                row.get::<_, bool>("student_active"),
            ),
        );
    }
    for event in &parsed {
        let id = event["id"].as_str().unwrap();
        let target_id = event["payload"]["targetId"].as_str().unwrap();
        let (target, active) = targets_by_id.get(target_id).ok_or(Error::Forbidden)?;
        if !matches!(target["kind"].as_str(), Some("attendance" | "note")) {
            return Err(Error::Forbidden);
        }
        let edit_time = event.get("editTime").and_then(Value::as_str);
        if let Some(existing) = tx
            .query_opt("SELECT actor FROM events WHERE id=$1::text::uuid", &[&id])
            .await?
        {
            if existing.get::<_, String>(0) != grant.name()
                || !same_event(tx, grant, event, target, false, edit_time).await?
            {
                return Err(Error::Conflict);
            }
            continue;
        }
        if !active {
            return Err(Error::Forbidden);
        }
        if target["kind"] == "attendance"
            && let Some(time) = edit_time
            && stale_edit(tx, event, target, time).await?
        {
            return Err(Error::Conflict);
        }
        if insert(tx, grant, event, target, true, edit_time).await? == 0
            && !same_event(tx, grant, event, target, false, edit_time).await?
        {
            return Err(Error::Conflict);
        }
    }
    Ok(json!({"saved": parsed.len()}))
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
            if record
                .get("editTime")
                .is_some_and(|v| !v.as_str().is_some_and(crate::ledger::iso_datetime))
            {
                return Err(Error::Invalid);
            }
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
    if sync {
        let scope = sync_scope(tx, grant, &parsed).await?;
        let mut outcomes = Vec::new();
        for (event, record) in parsed.iter().zip(records) {
            let p = &event["payload"];
            let current = sync_version(tx, p).await?;
            let edit_time = record.get("editTime").and_then(Value::as_str);
            if !scope.contains(&(
                p["studentId"].as_str().unwrap().to_owned(),
                p["enrollmentId"].as_str().unwrap().to_owned(),
            )) {
                outcomes.push(json!({"id":event["id"],"kind":"forbidden","current":null}));
                continue;
            }
            let id = event["id"].as_str().unwrap();
            let exists = tx
                .query_opt("SELECT 1 FROM events WHERE id=$1::text::uuid", &[&id])
                .await?
                .is_some();
            if exists {
                let same = same_event(tx, grant, event, p, false, edit_time).await?;
                outcomes.push(json!({"id":id,"kind":if same {"saved"} else {"id_conflict"},"current":current}));
                continue;
            }
            let stale = if record["expectedRevision"] == "unknown" {
                !current.is_null()
            } else {
                current.get("eventId").unwrap_or(&Value::Null) != &record["expectedEventId"]
            };
            if let Some(time) = edit_time
                && stale_edit(tx, event, p, time).await?
            {
                outcomes.push(json!({"id":id,"kind":"conflict","current":current}));
                continue;
            }
            if edit_time.is_none()
                && stale
                && current.get("status").unwrap_or(&Value::Null) != &p["status"]
            {
                outcomes.push(json!({"id":id,"kind":"conflict","current":current}));
                continue;
            }
            insert(tx, grant, event, p, false, edit_time).await?;
            outcomes.push(
                json!({"id":id,"kind":"saved","current":{"eventId":id,"status":p["status"]}}),
            );
        }
        return Ok(json!({"outcomes":outcomes}));
    }
    let snap = crate::staff_reads::snapshot(tx, grant, false)
        .await
        .map_err(Error::Other)?;
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
        let edit_time = event.get("editTime").and_then(Value::as_str);
        let exists = snap["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == event["id"]);
        if !exists
            && student["kind"] == "attendance"
            && let Some(time) = edit_time
            && stale_edit(tx, event, student, time).await?
        {
            return Err(Error::Conflict);
        }
        if insert(tx, grant, event, student, true, edit_time).await? == 0
            && !same_event(tx, grant, event, student, true, edit_time).await?
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
