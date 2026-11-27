use std::collections::HashMap;

use serde_json::{Value, json};
use tokio_postgres::Transaction;

use crate::{ledger, writes::Error};

fn uuid<'a>(body: &'a Value, key: &str) -> Result<&'a str, Error> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|s| ledger::valid_uuid(s))
        .ok_or(Error::Invalid)
}

fn grade(body: &Value, key: &str) -> Result<Option<String>, Error> {
    match body.get(key) {
        Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|n| (0.0..=100.0).contains(n))
            .map(|n| Some(ryu_js::Buffer::new().format_finite(n).to_owned()))
            .ok_or(Error::Invalid),
        None => Err(Error::Invalid),
    }
}

fn history(value: &Value) -> Result<&Vec<Value>, Error> {
    let items = value.as_array().ok_or(Error::Invalid)?;
    for item in items {
        let object = item.as_object().ok_or(Error::Invalid)?;
        if object.len() != 5
            || uuid(item, "requestId").is_err()
            || !matches!(
                item["result"].as_str(),
                Some("pass" | "fail" | "calculated")
            )
            || !item["note"].as_str().is_some_and(|s| {
                let n = ledger::js_trim(s).encode_utf16().count();
                (1..=2000).contains(&n)
            })
            || item["actor"].as_str().is_none_or(|s| s.is_empty())
            || !item["at"].as_str().is_some_and(ledger::iso_datetime)
        {
            return Err(Error::Invalid);
        }
    }
    Ok(items)
}

// Return the canonical integer and whether the decimal part is nonzero.
fn numeric_level(value: &str) -> Option<(String, bool)> {
    let mut normalized = String::new();
    for c in ledger::js_trim(value).chars() {
        match c {
            '0'..='9' | '.' => normalized.push(c),
            '\u{066b}' => normalized.push('.'),
            '\u{0660}'..='\u{0669}' => normalized.push(char::from_u32(c as u32 - 0x660 + 0x30)?),
            '\u{06f0}'..='\u{06f9}' => normalized.push(char::from_u32(c as u32 - 0x6f0 + 0x30)?),
            _ => return None,
        }
    }
    let (whole, fraction) = normalized.split_once('.').unwrap_or((&normalized, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || (normalized.contains('.') && fraction.is_empty())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole = whole.trim_start_matches('0');
    Some((
        if whole.is_empty() { "0" } else { whole }.to_owned(),
        fraction.bytes().any(|b| b != b'0'),
    ))
}

fn increment(value: &str) -> String {
    let mut digits = value.as_bytes().to_vec();
    let mut carry = true;
    for digit in digits.iter_mut().rev() {
        if *digit == b'9' {
            *digit = b'0';
        } else {
            *digit += 1;
            carry = false;
            break;
        }
    }
    if carry {
        digits.insert(0, b'1');
    }
    String::from_utf8(digits).expect("ASCII digits")
}

fn at_least(a: &str, b: &str) -> bool {
    a.len() > b.len() || (a.len() == b.len() && a >= b)
}

pub(crate) async fn apply(
    tx: &Transaction<'_>,
    path: &str,
    subject: &str,
    body: Value,
) -> Result<Value, Error> {
    let id = uuid(&body, "id")?;
    match path {
        "/api/admin/enrollments/decision" => {
            if body.as_object().is_none_or(|o| o.len() != 4) {
                return Err(Error::Invalid);
            }
            let request = uuid(&body, "requestId")?;
            let result = body["result"]
                .as_str()
                .filter(|v| ["pass", "fail", "calculated"].contains(v))
                .ok_or(Error::Invalid)?;
            let note = body["note"]
                .as_str()
                .map(ledger::js_trim)
                .filter(|s| (1..=2000).contains(&s.encode_utf16().count()))
                .ok_or(Error::Invalid)?;
            let row = tx.query_opt("SELECT grade_decisions FROM course_enrollments WHERE id = $1::text::uuid FOR UPDATE", &[&id]).await?.ok_or(Error::Invalid)?;
            let decisions: Value = row.get(0);
            let decisions = history(&decisions)?;
            if let Some(existing) = decisions.iter().find(|d| d["requestId"] == request) {
                if existing["result"] != result
                    || existing["note"].as_str().map(ledger::js_trim) != Some(note)
                    || existing["actor"] != subject
                {
                    return Err(Error::Conflict);
                }
            } else {
                tx.execute("UPDATE course_enrollments SET grade_decisions = grade_decisions || jsonb_build_array(jsonb_build_object('requestId', $2::text, 'result', $3::text, 'note', $4::text, 'actor', $5::text, 'at', to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'))) WHERE id = $1::text::uuid", &[&id, &request, &result, &note, &subject]).await?;
            }
        }
        "/api/admin/enrollments" => {
            let course = uuid(&body, "courseId")?;
            let student = uuid(&body, "studentId")?;
            let level = body["level"]
                .as_str()
                .map(ledger::js_trim)
                .filter(|s| s.encode_utf16().count() <= 200)
                .ok_or(Error::Invalid)?;
            let _ = level;
            let practical = grade(&body, "practical")?;
            let theory = grade(&body, "theory")?;
            let active = match body.get("active") {
                None => None,
                Some(v) => Some(v.as_bool().ok_or(Error::Invalid)?),
            };
            let teacher = match body.get("teacherId") {
                None | Some(Value::Null) => None,
                Some(_) => Some(uuid(&body, "teacherId")?),
            };
            if let Some(v) = body.get("teacherName")
                && !v.is_null()
                && !v.is_string()
            {
                return Err(Error::Invalid);
            }
            if let Some(v) = body.get("handbookReceived")
                && !v.is_boolean()
            {
                return Err(Error::Invalid);
            }
            if let Some(v) = body.get("notes")
                && !v.is_string()
            {
                return Err(Error::Invalid);
            }
            if let Some(v) = body.get("gradeDecisions") {
                history(v)?;
            }
            tx.query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&course],
            )
            .await?;
            let row = tx
                .query_opt(
                    "SELECT group_name FROM students WHERE id = $1::text::uuid FOR UPDATE",
                    &[&student],
                )
                .await?
                .ok_or(Error::Invalid)?;
            let stored_level: String = row.get(0);
            let stored_level = ledger::js_trim(&stored_level);
            if stored_level.encode_utf16().count() > 200 {
                return Err(Error::Invalid);
            }
            if tx
                .query_opt(
                    "SELECT id FROM courses WHERE id = $1::text::uuid FOR SHARE",
                    &[&course],
                )
                .await?
                .is_none()
            {
                return Err(Error::Invalid);
            }
            let rows = tx.query("SELECT id::text, student_id::text, course_id::text FROM course_enrollments WHERE id = $1::text::uuid OR (student_id = $2::text::uuid AND course_id = $3::text::uuid) FOR UPDATE", &[&id, &student, &course]).await?;
            if rows.iter().any(|row| {
                row.get::<_, String>(0) != id
                    || row.get::<_, String>(1) != student
                    || row.get::<_, String>(2) != course
            }) {
                return Err(Error::Conflict);
            }
            if let Some(teacher) = teacher
                && tx
                    .query_opt(
                        "SELECT id FROM teachers WHERE id = $1::text::uuid FOR SHARE",
                        &[&teacher],
                    )
                    .await?
                    .is_none()
            {
                return Err(Error::Invalid);
            }
            let teacher_present = body.get("teacherId").is_some();
            tx.execute("INSERT INTO course_enrollments (id, course_id, student_id, level, practical, theory, active, teacher_id) VALUES ($1::text::uuid, $2::text::uuid, $3::text::uuid, $4, $5::text::numeric, $6::text::numeric, COALESCE($7, true), $8::text::uuid) ON CONFLICT (course_id, student_id) DO UPDATE SET practical = EXCLUDED.practical, theory = EXCLUDED.theory, active = COALESCE($7, course_enrollments.active), teacher_id = CASE WHEN $9 THEN EXCLUDED.teacher_id ELSE course_enrollments.teacher_id END", &[&id, &course, &student, &stored_level, &practical, &theory, &active, &teacher, &teacher_present]).await?;
        }
        "/api/admin/courses/status" => {
            if body.as_object().is_none_or(|o| o.len() != 2) {
                return Err(Error::Invalid);
            }
            let open = body["open"].as_bool().ok_or(Error::Invalid)?;
            if !open {
                close(tx, id).await?;
            }
            if tx
                .execute(
                    "UPDATE courses SET open = $2 WHERE id = $1::text::uuid",
                    &[&id, &open],
                )
                .await?
                == 0
            {
                return Err(Error::Invalid);
            }
        }
        _ => return Err(Error::NotFound),
    }
    Ok(json!({"saved": true}))
}

async fn close(tx: &Transaction<'_>, id: &str) -> Result<(), Error> {
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        &[&id],
    )
    .await?;
    let students = tx.query("SELECT s.id::text, s.name, s.group_name, s.active FROM students s JOIN course_enrollments ce ON ce.student_id = s.id WHERE ce.course_id = $1::text::uuid ORDER BY s.name, s.id FOR UPDATE OF s", &[&id]).await?;
    let entries = tx.query("SELECT id::text, student_id::text, level, practical::float8, theory::float8, active, grade_decisions FROM course_enrollments WHERE course_id = $1::text::uuid ORDER BY id FOR SHARE", &[&id]).await?;
    for entry in &entries {
        let level: &str = entry.get(2);
        let practical: Option<f64> = entry.get(3);
        let theory: Option<f64> = entry.get(4);
        let decisions: Value = entry.get(6);
        if ledger::js_trim(level).encode_utf16().count() > 200
            || practical.is_some_and(|n| !(0.0..=100.0).contains(&n))
            || theory.is_some_and(|n| !(0.0..=100.0).contains(&n))
        {
            return Err(Error::Invalid);
        }
        history(&decisions)?;
    }
    let course = tx.query_opt("SELECT name, open, attendance_minimum::float8, practical_minimum::float8, theory_minimum::float8 FROM courses WHERE id = $1::text::uuid FOR NO KEY UPDATE", &[&id]).await?.ok_or(Error::Invalid)?;
    let course_name: &str = course.get(0);
    let attendance_min: f64 = course.get(2);
    let practical_min: f64 = course.get(3);
    let theory_min: f64 = course.get(4);
    if ledger::js_trim(course_name).is_empty()
        || ledger::js_trim(course_name).encode_utf16().count() > 200
        || [attendance_min, practical_min, theory_min]
            .iter()
            .any(|n| !(0.0..=100.0).contains(n))
    {
        return Err(Error::Invalid);
    }
    if !course.get::<_, bool>(1) {
        return Ok(());
    }
    let ids: Vec<&str> = students.iter().map(|row| row.get(0)).collect();
    let sql = format!(
        "SELECT e.id::text AS id, e.actor::text AS actor, to_char(e.client_time AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS client_time, to_char(e.received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS received_at, e.payload FROM events e WHERE e.student_id = ANY(SELECT unnest($1::text[])::uuid) AND {} ORDER BY e.sequence",
        ledger::VISIBLE_LEDGER_EVENT
    );
    let events = tx
        .query(&sql, &[&ids])
        .await?
        .iter()
        .map(ledger::decode_event)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::Other)?;
    let attendance = ledger::project(&events, &HashMap::new());
    for entry in &entries {
        if !entry.get::<_, bool>(5) {
            continue;
        }
        let enrollment: &str = entry.get(0);
        let student_id: &str = entry.get(1);
        let decisions: Value = entry.get(6);
        let latest = history(&decisions)?
            .last()
            .and_then(|d| d["result"].as_str());
        let marks: Vec<_> = attendance
            .iter()
            .filter(|v| v["enrollmentId"] == enrollment)
            .collect();
        let passed = match latest {
            Some("pass") => true,
            Some("fail") => false,
            _ => {
                !marks.is_empty()
                    && entry
                        .get::<_, Option<f64>>(3)
                        .is_some_and(|n| n >= practical_min)
                    && entry
                        .get::<_, Option<f64>>(4)
                        .is_some_and(|n| n >= theory_min)
                    && marks.iter().filter(|v| v["status"] == "present").count() as f64
                        / marks.len() as f64
                        * 100.0
                        >= attendance_min
            }
        };
        if !passed {
            continue;
        }
        let Some(student) = students
            .iter()
            .find(|s| s.get::<_, &str>(0) == student_id && s.get::<_, bool>(3))
        else {
            continue;
        };
        let level: &str = entry.get(2);
        let roster: &str = student.get(2);
        let Some((whole, false)) = numeric_level(level) else {
            continue;
        };
        let current = numeric_level(roster);
        if current.is_none() && !ledger::js_trim(roster).is_empty() {
            continue;
        }
        let next = increment(&whole);
        if next.len() > 100 || current.is_some_and(|(n, _)| at_least(&n, &next)) {
            continue;
        }
        tx.execute(
            "UPDATE students SET group_name = $2 WHERE id = $1::text::uuid",
            &[&student_id, &next],
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn levels_keep_arbitrary_precision_and_arabic_digits() {
        assert_eq!(numeric_level(" ٢٫٠ "), Some(("2".to_owned(), false)));
        assert_eq!(
            numeric_level("1.00000000000000001"),
            Some(("1".to_owned(), true))
        );
        assert_eq!(increment("999999999999999999999"), "1000000000000000000000");
        assert!(numeric_level("1.").is_none());
    }

    #[tokio::test]
    async fn decisions_and_closure_preserve_promotions_and_retries() {
        let Ok(url) = std::env::var("TEST_RAILWAY_DATABASE_URL") else {
            return;
        };
        let (mut client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move { connection.await.unwrap() });
        let tx = client.transaction().await.unwrap();
        let row = tx.query_one("SELECT gen_random_uuid()::text, gen_random_uuid()::text, gen_random_uuid()::text, gen_random_uuid()::text, gen_random_uuid()::text, gen_random_uuid()::text, gen_random_uuid()::text", &[]).await.unwrap();
        let course: &str = row.get(0);
        let student: &str = row.get(1);
        let other: &str = row.get(2);
        let enrollment: &str = row.get(3);
        let other_enrollment: &str = row.get(4);
        let teacher: &str = row.get(5);
        let request: &str = row.get(6);
        tx.execute("INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ($1::text::uuid, $2, 'Test teacher', 'read_write')", &[&teacher, &teacher]).await.unwrap();
        tx.execute(
            "INSERT INTO courses (id, name) VALUES ($1::text::uuid, 'Test course')",
            &[&course],
        )
        .await
        .unwrap();
        tx.execute("INSERT INTO students (id, name, group_name) VALUES ($1::text::uuid, 'First student', '1'), ($2::text::uuid, 'Second student', '1')", &[&student, &other]).await.unwrap();
        for (id, student_id) in [(enrollment, student), (other_enrollment, other)] {
            let input = json!({"id":id,"studentId":student_id,"courseId":course,"level":"ignored","practical":90,"theory":90});
            assert_eq!(
                apply(&tx, "/api/admin/enrollments", "admin", input)
                    .await
                    .unwrap(),
                json!({"saved":true})
            );
        }
        let mark = json!({"kind":"attendance","studentId":student,"enrollmentId":enrollment,"date":"2026-11-20","status":"late"});
        tx.execute("INSERT INTO events (id, teacher_id, student_id, actor, client_time, payload, enrollment_id) VALUES (gen_random_uuid(), $1::text::uuid, $2::text::uuid, 'Test teacher', now(), $3, $4::text::uuid)", &[&teacher, &student, &mark, &enrollment]).await.unwrap();
        let decision =
            json!({"id":other_enrollment,"requestId":request,"result":"pass","note":" reason "});
        assert_eq!(
            apply(
                &tx,
                "/api/admin/enrollments/decision",
                "admin",
                decision.clone()
            )
            .await
            .unwrap(),
            json!({"saved":true})
        );
        assert_eq!(
            apply(
                &tx,
                "/api/admin/enrollments/decision",
                "admin",
                decision.clone()
            )
            .await
            .unwrap(),
            json!({"saved":true})
        );
        let mut conflict = decision;
        conflict["result"] = json!("fail");
        assert!(matches!(
            apply(&tx, "/api/admin/enrollments/decision", "admin", conflict).await,
            Err(Error::Conflict)
        ));
        for open in [false, false, true, false] {
            apply(
                &tx,
                "/api/admin/courses/status",
                "admin",
                json!({"id":course,"open":open}),
            )
            .await
            .unwrap();
        }
        let rows = tx.query("SELECT group_name FROM students WHERE id = ANY(SELECT unnest($1::text[])::uuid) ORDER BY name", &[&vec![student, other]]).await.unwrap();
        assert_eq!(
            rows.iter().map(|r| r.get::<_, &str>(0)).collect::<Vec<_>>(),
            vec!["2", "2"]
        );
        assert_eq!(tx.query_one("SELECT jsonb_array_length(grade_decisions) FROM course_enrollments WHERE id = $1::text::uuid", &[&other_enrollment]).await.unwrap().get::<_, i32>(0), 1);
        tx.rollback().await.unwrap();
    }
}
