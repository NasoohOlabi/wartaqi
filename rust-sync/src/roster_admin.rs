use crate::{
    ledger::{js_trim, valid_uuid},
    writes::Error,
};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

fn strict(value: &Value, keys: &[&str]) -> Result<(), Error> {
    let object = value.as_object().ok_or(Error::Invalid)?;
    if object.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn uuid<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| valid_uuid(s))
        .ok_or(Error::Invalid)
}
fn string<'a>(value: &'a Value, key: &str, min: usize, max: usize) -> Result<&'a str, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(js_trim)
        .filter(|s| (min..=max).contains(&s.encode_utf16().count()))
        .ok_or(Error::Invalid)
}
fn optional_string<'a>(value: &'a Value, key: &str, max: usize) -> Result<Option<&'a str>, Error> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => Ok(Some(string(value, key, 0, max)?).filter(|s| !s.is_empty())),
    }
}

pub(crate) async fn apply(tx: &Transaction<'_>, path: &str, body: Value) -> Result<Value, Error> {
    let combined = path == "/api/admin/students/enroll";
    if combined {
        strict(&body, &["student", "enrollment"])?;
    }
    let student = if combined {
        body.get("student").ok_or(Error::Invalid)?
    } else {
        &body
    };
    let split = student.get("name").is_none();
    let mut keys = vec![
        "studentId",
        "teacherId",
        "group",
        "active",
        "whatsapp",
        "location",
        "birthYear",
    ];
    if split {
        keys.extend(["firstName", "lastName"]);
    } else {
        keys.push("name");
    }
    strict(student, &keys)?;
    let id = uuid(student, "studentId")?;
    let teacher = match student.get("teacherId") {
        Some(Value::Null) => None,
        Some(Value::String(s)) if valid_uuid(s) => Some(s.as_str()),
        _ => return Err(Error::Invalid),
    };
    let group = string(student, "group", 0, 100)?;
    let active = student
        .get("active")
        .and_then(Value::as_bool)
        .ok_or(Error::Invalid)?;
    let (name, first, last) = if split {
        let first = string(student, "firstName", 1, 200)?;
        let last = string(student, "lastName", 0, 200)?;
        let name = if last.is_empty() {
            first.to_owned()
        } else {
            format!("{first} {last}")
        };
        if name.encode_utf16().count() > 200 {
            return Err(Error::Invalid);
        }
        (name, Some(first), Some(last))
    } else {
        (string(student, "name", 1, 200)?.to_owned(), None, None)
    };
    let whatsapp = optional_string(student, "whatsapp", 40)?;
    let location = optional_string(student, "location", 200)?;
    let year = match student.get("birthYear") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let n = v.as_i64().ok_or(Error::Invalid)?;
            let current: i32 = tx
                .query_one("SELECT EXTRACT(YEAR FROM now())::int", &[])
                .await?
                .get(0);
            if !(1900..=i64::from(current)).contains(&n) {
                return Err(Error::Invalid);
            }
            Some(n as i32)
        }
    };
    let mut enrollment = if combined {
        let value = body.get("enrollment").ok_or(Error::Invalid)?;
        strict(value, &["id", "courseId", "teacherId", "level"])?;
        let course = uuid(value, "courseId")?;
        uuid(value, "id")?;
        tx.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            &[&course],
        )
        .await?;
        Some(value.clone())
    } else {
        None
    };
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        &[&id],
    )
    .await?;
    let current = tx
        .query_opt(
            "SELECT teacher_id::text FROM students WHERE id = $1::text::uuid FOR UPDATE",
            &[&id],
        )
        .await?;
    if let Some(teacher) = teacher {
        let old = current
            .as_ref()
            .and_then(|row| row.get::<_, Option<String>>(0));
        if old.as_deref() != Some(teacher) && tx.query_opt("SELECT t.id FROM teachers t JOIN app_accounts a ON a.teacher_id = t.id WHERE t.id = $1::text::uuid AND a.role = 'teacher' AND a.active AND (a.expires_at IS NULL OR a.expires_at > now()) AND t.active AND (t.expires_at IS NULL OR t.expires_at > now()) FOR SHARE OF t, a", &[&teacher]).await?.is_none() { return Err(Error::Invalid) }
    }
    let has_whatsapp = student.get("whatsapp").is_some();
    let has_location = student.get("location").is_some();
    let has_year = student.get("birthYear").is_some();
    tx.execute("INSERT INTO students (id, teacher_id, name, group_name, active, first_name, last_name, whatsapp, location, birth_year) VALUES ($1::text::uuid, $2::text::uuid, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT (id) DO UPDATE SET teacher_id = EXCLUDED.teacher_id, name = EXCLUDED.name, active = EXCLUDED.active, first_name = CASE WHEN EXCLUDED.first_name IS NULL AND students.name = EXCLUDED.name THEN students.first_name ELSE EXCLUDED.first_name END, last_name = CASE WHEN EXCLUDED.first_name IS NULL AND students.name = EXCLUDED.name THEN students.last_name ELSE EXCLUDED.last_name END, whatsapp = CASE WHEN $11 THEN EXCLUDED.whatsapp ELSE students.whatsapp END, location = CASE WHEN $12 THEN EXCLUDED.location ELSE students.location END, birth_year = CASE WHEN $13 THEN EXCLUDED.birth_year ELSE students.birth_year END", &[&id, &teacher, &name, &group, &active, &first, &last, &whatsapp, &location, &year, &has_whatsapp, &has_location, &has_year]).await?;
    if let Some(ref mut membership) = enrollment {
        membership["studentId"] = json!(id);
        membership["active"] = json!(true);
        crate::catalog::apply(
            tx,
            "/api/admin/enrollments/membership",
            "",
            membership.clone(),
        )
        .await?;
    }
    crate::staff_reads::admin_board(tx)
        .await
        .map_err(Error::Other)
}
