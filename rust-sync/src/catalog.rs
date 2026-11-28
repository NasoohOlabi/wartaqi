use crate::{
    ledger::{js_trim, valid_uuid},
    writes::Error,
};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

fn id(body: &Value) -> Result<&str, Error> {
    body.get("id")
        .and_then(Value::as_str)
        .filter(|v| valid_uuid(v))
        .ok_or(Error::Invalid)
}
fn strict(body: &Value, keys: &[&str]) -> Result<(), Error> {
    let object = body.as_object().ok_or(Error::Invalid)?;
    if object.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn percentage(body: &Value, key: &str) -> Result<f64, Error> {
    body.get(key)
        .and_then(Value::as_f64)
        .filter(|n| (0.0..=100.0).contains(n))
        .ok_or(Error::Invalid)
}
fn grade(body: &Value, key: &str) -> Result<Option<f64>, Error> {
    match body.get(key) {
        Some(Value::Null) => Ok(None),
        Some(_) => percentage(body, key).map(Some),
        None => Err(Error::Invalid),
    }
}

pub(crate) async fn apply(
    tx: &Transaction<'_>,
    path: &str,
    _subject: &str,
    body: Value,
) -> Result<Value, Error> {
    let id = if path == "/api/admin/enrollments/assignment"
        && body.get("kind").and_then(Value::as_str) == Some("group")
    {
        ""
    } else {
        id(&body)?
    };
    match path {
        "/api/admin/courses" => {
            let name = body
                .get("name")
                .and_then(Value::as_str)
                .map(js_trim)
                .filter(|s| !s.is_empty() && s.encode_utf16().count() <= 200)
                .ok_or(Error::Invalid)?;
            tx.execute("INSERT INTO courses (id, name) VALUES ($1::text::uuid, $2) ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name", &[&id, &name]).await?;
        }
        "/api/admin/courses/settings" => {
            strict(
                &body,
                &[
                    "id",
                    "attendanceMinimum",
                    "practicalMinimum",
                    "theoryMinimum",
                ],
            )?;
            let attendance = percentage(&body, "attendanceMinimum")?;
            let practical = percentage(&body, "practicalMinimum")?;
            let theory = percentage(&body, "theoryMinimum")?;
            if tx.execute("UPDATE courses SET attendance_minimum = $2::float8, practical_minimum = $3::float8, theory_minimum = $4::float8 WHERE id = $1::text::uuid", &[&id, &attendance, &practical, &theory]).await? == 0 { return Err(Error::Invalid) }
        }
        "/api/admin/enrollments/handbook" => {
            strict(&body, &["id", "handbookReceived"])?;
            let received = body
                .get("handbookReceived")
                .and_then(Value::as_bool)
                .ok_or(Error::Invalid)?;
            if tx.execute("UPDATE course_enrollments SET handbook_received = $2 WHERE id = $1::text::uuid", &[&id, &received]).await? == 0 { return Err(Error::Invalid) }
        }
        "/api/admin/enrollments/grades" => {
            strict(&body, &["id", "practical", "theory", "notes"])?;
            let practical = grade(&body, "practical")?
                .map(|value| ryu_js::Buffer::new().format_finite(value).to_owned());
            let theory = grade(&body, "theory")?
                .map(|value| ryu_js::Buffer::new().format_finite(value).to_owned());
            let notes = match body.get("notes") {
                None => None,
                Some(Value::String(s)) => Some(s.as_str()),
                _ => return Err(Error::Invalid),
            };
            if tx.execute("UPDATE course_enrollments SET practical = $2::text::numeric, theory = $3::text::numeric, notes = COALESCE($4, notes) WHERE id = $1::text::uuid", &[&id, &practical, &theory, &notes]).await? == 0 { return Err(Error::Invalid) }
        }
        "/api/admin/enrollments/membership" | "/api/admin/enrollments/save" => {
            strict(
                &body,
                &[
                    "id",
                    "courseId",
                    "studentId",
                    "teacherId",
                    "level",
                    "active",
                ],
            )?;
            let uuid = |key: &str| {
                body.get(key)
                    .and_then(Value::as_str)
                    .filter(|v| valid_uuid(v))
                    .ok_or(Error::Invalid)
            };
            let course = uuid("courseId")?;
            let student = uuid("studentId")?;
            let active = body
                .get("active")
                .and_then(Value::as_bool)
                .ok_or(Error::Invalid)?;
            let teacher = match body.get("teacherId") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) if valid_uuid(value) => Some(value.as_str()),
                _ => return Err(Error::Invalid),
            };
            let explicit_level = body
                .get("level")
                .map(|level| {
                    level
                        .as_str()
                        .map(js_trim)
                        .filter(|level| level.encode_utf16().count() <= 200)
                        .ok_or(Error::Invalid)
                })
                .transpose()?;
            let explicit_level = explicit_level.filter(|_| path == "/api/admin/enrollments/save");
            tx.query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&course],
            )
            .await?;
            let student_row = tx
                .query_opt(
                    "SELECT group_name FROM students WHERE id = $1::text::uuid FOR UPDATE",
                    &[&student],
                )
                .await?
                .ok_or(Error::Invalid)?;
            let level: String = student_row.get(0);
            let level = explicit_level.unwrap_or_else(|| js_trim(&level));
            if level.encode_utf16().count() > 200 {
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
            let existing = tx.query("SELECT id::text, student_id::text, course_id::text FROM course_enrollments WHERE id = $1::text::uuid OR (student_id = $2::text::uuid AND course_id = $3::text::uuid) FOR UPDATE", &[&id, &student, &course]).await?;
            if existing.iter().any(|row| {
                !row.get::<_, String>(0).eq(id)
                    || !row.get::<_, String>(1).eq(student)
                    || !row.get::<_, String>(2).eq(course)
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
            tx.execute("INSERT INTO course_enrollments (id, course_id, student_id, level, active, teacher_id) VALUES ($1::text::uuid, $2::text::uuid, $3::text::uuid, $4, $5, $6::text::uuid) ON CONFLICT (course_id, student_id) DO UPDATE SET active = EXCLUDED.active, teacher_id = CASE WHEN $7 THEN EXCLUDED.teacher_id ELSE course_enrollments.teacher_id END, level = COALESCE($8, course_enrollments.level)", &[&id, &course, &student, &level, &active, &teacher, &teacher_present, &explicit_level]).await?;
        }
        "/api/admin/enrollments/assignment" => {
            let kind = body
                .get("kind")
                .and_then(Value::as_str)
                .ok_or(Error::Invalid)?;
            let bulk = match kind {
                "edit" => {
                    strict(
                        &body,
                        &["kind", "courseId", "id", "teacherId", "level", "active"],
                    )?;
                    false
                }
                "group" => {
                    strict(&body, &["kind", "courseId", "ids", "teacherId", "level"])?;
                    true
                }
                _ => return Err(Error::Invalid),
            };
            let course = body
                .get("courseId")
                .and_then(Value::as_str)
                .filter(|v| valid_uuid(v))
                .ok_or(Error::Invalid)?;
            let ids: Vec<&str> = if bulk {
                let values = body
                    .get("ids")
                    .and_then(Value::as_array)
                    .filter(|a| !a.is_empty())
                    .ok_or(Error::Invalid)?;
                values
                    .iter()
                    .map(|v| v.as_str().filter(|s| valid_uuid(s)).ok_or(Error::Invalid))
                    .collect::<Result<_, _>>()?
            } else {
                vec![id]
            };
            let teacher = match body.get("teacherId") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) if valid_uuid(value) => Some(value.as_str()),
                _ => return Err(Error::Invalid),
            };
            let active = match body.get("active") {
                None => None,
                Some(Value::Bool(value)) => Some(*value),
                _ => return Err(Error::Invalid),
            };
            if let Some(level) = body.get("level") {
                let level = level.as_str().ok_or(Error::Invalid)?;
                if js_trim(level).encode_utf16().count() > 200 {
                    return Err(Error::Invalid);
                }
            }
            let teacher_present = body.get("teacherId").is_some();
            if !teacher_present && body.get("level").is_none() && active.is_none() {
                return Err(Error::Invalid);
            }
            tx.query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&course],
            )
            .await?;
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
            let count = tx.execute("UPDATE course_enrollments SET teacher_id = CASE WHEN $3 THEN $4::text::uuid ELSE teacher_id END, active = COALESCE($5, active) WHERE course_id = $1::text::uuid AND id = ANY(SELECT unnest($2::text[])::uuid) AND (NOT $6::boolean OR active)", &[&course, &ids, &teacher_present, &teacher, &active, &bulk]).await?;
            if count != ids.len() as u64 {
                return Err(Error::Conflict);
            }
        }
        _ => return Err(Error::NotFound),
    }
    Ok(json!({"saved": true}))
}
