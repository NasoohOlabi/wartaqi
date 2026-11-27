use crate::access::Grant;
use crate::ledger::{
    InvalidData, VISIBLE_LEDGER_EVENT, decode_event, iso_datetime, js_trim, valid_uuid,
};
use serde_json::{Value, json};
use tokio_postgres::{Row, Transaction};

type Error = Box<dyn std::error::Error + Send + Sync>;

fn object(row: &Row) -> Value {
    row.get(0)
}

pub(crate) async fn notebook(tx: &Transaction<'_>, grant: &Grant) -> Result<Value, Error> {
    snapshot(tx, grant, true).await
}

pub(crate) async fn snapshot(
    tx: &Transaction<'_>,
    grant: &Grant,
    include_inactive: bool,
) -> Result<Value, Error> {
    let admin = grant.is_admin();
    let scope = if admin {
        if include_inactive { "true" } else { "s.active" }
    } else {
        "s.teacher_id = $1::text::uuid AND s.active"
    };
    let teacher_id = grant.id();
    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] =
        if admin { &[] } else { &[&teacher_id] };
    let mut students = tx.query(&format!("SELECT jsonb_build_object('kind','student','studentId',s.id::text,'name',s.name,'group',s.group_name,'active',s.active,'firstName',COALESCE(s.first_name,s.name),'lastName',COALESCE(s.last_name,''),'whatsapp',s.whatsapp,'location',s.location,'birthYear',s.birth_year) FROM students s WHERE {scope} ORDER BY s.name,s.id FOR SHARE"), params).await?.iter().map(object).collect::<Vec<_>>();
    let events = tx.query(&format!("SELECT e.id::text AS id, e.actor::text AS actor, to_char(e.client_time AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS client_time, to_char(e.received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS received_at, e.payload FROM events e JOIN students s ON s.id=e.student_id WHERE {scope} AND {VISIBLE_LEDGER_EVENT} ORDER BY e.sequence"), params).await?.iter().map(decode_event).collect::<Result<Vec<_>, _>>()?;
    let mut enrollments = tx.query(&format!("SELECT jsonb_build_object('id',ce.id::text,'courseId',ce.course_id::text,'studentId',ce.student_id::text,'level',ce.level,'practical',{},'theory',{},'notes',{},'active',ce.active,'handbookReceived',ce.handbook_received,'teacherId',ce.teacher_id::text,'teacherName',(SELECT name FROM teachers WHERE id=ce.teacher_id),'gradeDecisions',{}) FROM course_enrollments ce JOIN students s ON s.id=ce.student_id WHERE {scope} ORDER BY ce.id FOR SHARE OF ce", if admin { "ce.practical::float8" } else { "NULL::float8" }, if admin { "ce.theory::float8" } else { "NULL::float8" }, if admin { "ce.notes" } else { "''::text" }, if admin { "ce.grade_decisions" } else { "'[]'::jsonb" }), params).await?.iter().map(object).collect::<Vec<_>>();
    let course_sql = if admin {
        "SELECT jsonb_build_object('id',id::text,'name',name,'open',open,'attendanceMinimum',attendance_minimum::float8,'practicalMinimum',practical_minimum::float8,'theoryMinimum',theory_minimum::float8) FROM courses ORDER BY btrim(name) COLLATE \"en-US-x-icu\", id FOR SHARE"
    } else {
        "SELECT jsonb_build_object('id',c.id::text,'name',c.name,'open',c.open,'attendanceMinimum',70,'practicalMinimum',80,'theoryMinimum',80) FROM courses c WHERE EXISTS (SELECT 1 FROM course_enrollments ce JOIN students s ON s.id=ce.student_id WHERE ce.course_id=c.id AND s.teacher_id=$1::text::uuid AND s.active) ORDER BY btrim(c.name) COLLATE \"en-US-x-icu\", c.id FOR SHARE OF c"
    };
    let mut courses = tx
        .query(course_sql, params)
        .await?
        .iter()
        .map(object)
        .collect::<Vec<_>>();
    snapshot_shapes(&mut students, &mut courses, &mut enrollments)?;
    Ok(
        json!({"teacher": grant.view(), "students": students, "events": events, "courses": courses, "enrollments": enrollments}),
    )
}

pub(crate) async fn admin_board(tx: &Transaction<'_>) -> Result<Value, Error> {
    let mut accounts = tx.query("SELECT jsonb_build_object('authUserId',u.id::text,'profileName',u.name,'name',COALESCE(t.name,s.name,u.name,''),'verifiedPhone',a.verified_phone,'role',a.role,'active',COALESCE(a.active,false),'expiresAt',CASE WHEN a.expires_at IS NULL THEN NULL ELSE to_char(a.expires_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') END,'teacherId',a.teacher_id::text,'studentId',a.student_id::text) FROM neon_auth.\"user\" u LEFT JOIN app_accounts a ON a.auth_user_id=u.id::text LEFT JOIN teachers t ON t.id=a.teacher_id LEFT JOIN students s ON s.id=a.student_id ORDER BY u.id LIMIT 1000", &[]).await?.iter().map(object).collect::<Vec<_>>();
    let mut teachers = tx.query("SELECT jsonb_build_object('teacherId',t.id::text,'name',t.name,'active',COALESCE((t.active AND a.role='teacher' AND a.active AND (t.expires_at IS NULL OR t.expires_at>now()) AND (a.expires_at IS NULL OR a.expires_at>now())),false)) FROM teachers t LEFT JOIN app_accounts a ON a.teacher_id=t.id WHERE a.role='teacher' OR EXISTS (SELECT 1 FROM students s WHERE s.teacher_id=t.id) ORDER BY t.name,t.id", &[]).await?.iter().map(object).collect::<Vec<_>>();
    let mut students = tx.query("SELECT jsonb_build_object('studentId',id::text,'name',name,'firstName',COALESCE(first_name,name),'lastName',COALESCE(last_name,''),'group',group_name,'active',active,'teacherId',teacher_id::text,'whatsapp',whatsapp,'location',location,'birthYear',birth_year) FROM students ORDER BY name,id", &[]).await?.iter().map(object).collect::<Vec<_>>();
    admin_shapes(&mut accounts, &mut teachers, &mut students)?;
    Ok(json!({"accounts":accounts,"teachers":teachers,"students":students}))
}

fn invalid() -> Error {
    InvalidData.into()
}
fn string(value: &mut Value, key: &str, min: usize, max: usize, trim: bool) -> Result<(), Error> {
    let v = value.get_mut(key).ok_or_else(invalid)?;
    let s = v.as_str().ok_or_else(invalid)?;
    let s = if trim { js_trim(s) } else { s };
    if !(min..=max).contains(&s.encode_utf16().count()) {
        return Err(invalid());
    }
    *v = Value::String(s.to_owned());
    Ok(())
}
fn uuid(value: &Value, key: &str, nullable: bool) -> Result<(), Error> {
    let v = value.get(key).ok_or_else(invalid)?;
    if nullable && v.is_null() {
        return Ok(());
    }
    let s = v.as_str().ok_or_else(invalid)?;
    if !valid_uuid(s) {
        return Err(invalid());
    }
    Ok(())
}
fn boolean(value: &Value, key: &str) -> Result<(), Error> {
    if value.get(key).and_then(Value::as_bool).is_some() {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn nullable_string(value: &mut Value, key: &str, max: usize, trim: bool) -> Result<(), Error> {
    if value.get(key).is_some_and(Value::is_null) {
        Ok(())
    } else {
        string(value, key, 0, max, trim)
    }
}
fn nullable_int(value: &Value, key: &str) -> Result<(), Error> {
    let v = value.get(key).ok_or_else(invalid)?;
    if v.is_null() || v.as_i64().is_some() {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn percent(value: &Value, key: &str, nullable: bool) -> Result<(), Error> {
    let v = value.get(key).ok_or_else(invalid)?;
    if (nullable && v.is_null()) || v.as_f64().is_some_and(|n| (0.0..=100.0).contains(&n)) {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn snapshot_shapes(
    students: &mut [Value],
    courses: &mut [Value],
    enrollments: &mut [Value],
) -> Result<(), Error> {
    for s in students {
        uuid(s, "studentId", false)?;
        string(s, "name", 1, 200, true)?;
        string(s, "group", 0, 100, true)?;
        boolean(s, "active")?;
        string(s, "firstName", 1, 200, true)?;
        string(s, "lastName", 0, 200, true)?;
        nullable_string(s, "whatsapp", 40, true)?;
        nullable_string(s, "location", usize::MAX, false)?;
        nullable_int(s, "birthYear")?;
    }
    for c in courses {
        uuid(c, "id", false)?;
        string(c, "name", 1, 200, true)?;
        boolean(c, "open")?;
        for k in ["attendanceMinimum", "practicalMinimum", "theoryMinimum"] {
            percent(c, k, false)?;
        }
    }
    for e in enrollments {
        for k in ["id", "courseId", "studentId"] {
            uuid(e, k, false)?;
        }
        uuid(e, "teacherId", true)?;
        nullable_string(e, "teacherName", usize::MAX, false)?;
        string(e, "level", 0, 200, true)?;
        percent(e, "practical", true)?;
        percent(e, "theory", true)?;
        boolean(e, "active")?;
        boolean(e, "handbookReceived")?;
        string(e, "notes", 0, usize::MAX, false)?;
        let decisions = e
            .get_mut("gradeDecisions")
            .and_then(Value::as_array_mut)
            .ok_or_else(invalid)?;
        for d in decisions {
            uuid(d, "requestId", false)?;
            string(d, "note", 1, 2000, true)?;
            string(d, "actor", 1, usize::MAX, false)?;
            if !matches!(
                d.get("result").and_then(Value::as_str),
                Some("pass" | "fail" | "calculated")
            ) || !d
                .get("at")
                .and_then(Value::as_str)
                .is_some_and(iso_datetime)
            {
                return Err(invalid());
            }
            if d.as_object().is_none_or(|o| {
                o.len() != 5
                    || o.keys().any(|k| {
                        !["requestId", "result", "note", "actor", "at"].contains(&k.as_str())
                    })
            }) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

fn admin_shapes(
    accounts: &mut [Value],
    teachers: &mut [Value],
    students: &mut [Value],
) -> Result<(), Error> {
    for a in accounts {
        string(a, "authUserId", 1, 256, false)?;
        string(a, "name", 0, usize::MAX, false)?;
        string(a, "profileName", 0, usize::MAX, false)?;
        nullable_string(a, "verifiedPhone", usize::MAX, false)?;
        boolean(a, "active")?;
        uuid(a, "teacherId", true)?;
        uuid(a, "studentId", true)?;
        if !a["role"].is_null()
            && !matches!(
                a["role"].as_str(),
                Some("administrator" | "teacher" | "student")
            )
        {
            return Err(invalid());
        }
        nullable_string(a, "expiresAt", usize::MAX, false)?;
    }
    for t in teachers {
        uuid(t, "teacherId", false)?;
        string(t, "name", 0, usize::MAX, false)?;
        boolean(t, "active")?;
    }
    for s in students {
        uuid(s, "studentId", false)?;
        uuid(s, "teacherId", true)?;
        for k in ["name", "firstName", "lastName", "group"] {
            string(s, k, 0, usize::MAX, false)?;
        }
        boolean(s, "active")?;
        nullable_string(s, "whatsapp", usize::MAX, false)?;
        nullable_string(s, "location", usize::MAX, false)?;
        nullable_int(s, "birthYear")?;
    }
    Ok(())
}
