use crate::{
    access::Grant,
    ledger::{js_trim, valid_uuid},
    writes::Error,
};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

const FIELDS: [&str; 6] = [
    "firstName",
    "lastName",
    "group",
    "whatsapp",
    "location",
    "birthYear",
];

fn profile(value: &Value, update: bool, year: i32) -> Result<Value, Error> {
    let object = value.as_object().ok_or(Error::Invalid)?;
    if FIELDS.iter().any(|key| !object.contains_key(*key)) {
        return Err(Error::Invalid);
    }
    let mut result = serde_json::Map::new();
    for field in FIELDS {
        let v = &value[field];
        if field == "birthYear" {
            if !v.is_null()
                && !v
                    .as_i64()
                    .is_some_and(|n| !update || (1900..=year as i64).contains(&n))
            {
                return Err(Error::Invalid);
            }
            result.insert(field.into(), v.clone());
            continue;
        }
        if v.is_null() && matches!(field, "whatsapp" | "location") {
            result.insert(field.into(), Value::Null);
            continue;
        }
        let s = v.as_str().ok_or(Error::Invalid)?;
        let s = if field == "location" && !update {
            s
        } else {
            js_trim(s)
        };
        let length = s.encode_utf16().count();
        if update
            && (length
                > match field {
                    "group" => 100,
                    "whatsapp" => 40,
                    _ => 200,
                }
                || (field == "firstName" && length == 0))
        {
            return Err(Error::Invalid);
        }
        result.insert(field.into(), json!(s));
    }
    Ok(Value::Object(result))
}

pub(crate) async fn apply(
    tx: &Transaction<'_>,
    path: &str,
    _subject: &str,
    grant: &Grant,
    input: Value,
) -> Result<Value, Error> {
    if path != "/api/student/update" {
        return Err(Error::NotFound);
    }
    let obj = input.as_object().ok_or(Error::Invalid)?;
    if obj.len() != 8
        || obj
            .keys()
            .any(|k| !FIELDS.contains(&k.as_str()) && k != "studentId" && k != "expected")
    {
        return Err(Error::Invalid);
    }
    let id = input["studentId"]
        .as_str()
        .filter(|s| valid_uuid(s))
        .ok_or(Error::Invalid)?;
    let expected_obj = input["expected"].as_object().ok_or(Error::Invalid)?;
    if expected_obj.len() != FIELDS.len()
        || expected_obj.keys().any(|k| !FIELDS.contains(&k.as_str()))
    {
        return Err(Error::Invalid);
    }
    let year: i32 = tx
        .query_one("SELECT extract(year from now())::int", &[])
        .await?
        .get(0);
    let next = profile(&input, true, year)?;
    let expected = profile(&input["expected"], false, year)?;
    let first = next["firstName"].as_str().unwrap();
    let last = next["lastName"].as_str().unwrap();
    let name = if last.is_empty() {
        first.to_owned()
    } else {
        format!("{first} {last}")
    };
    if name.encode_utf16().count() > 200 {
        return Err(Error::Invalid);
    }
    let row = tx.query_opt("SELECT COALESCE(first_name, name), COALESCE(last_name, ''), group_name, whatsapp, location, birth_year FROM students WHERE id = $1::text::uuid AND active AND ($2::boolean OR teacher_id = $3::text::uuid) FOR UPDATE", &[&id, &grant.is_admin(), &grant.id()]).await?.ok_or(Error::Forbidden)?;
    let current = json!({"firstName":row.get::<_, String>(0), "lastName":row.get::<_, String>(1), "group":row.get::<_, String>(2), "whatsapp":row.get::<_, Option<String>>(3), "location":row.get::<_, Option<String>>(4), "birthYear":row.get::<_, Option<i32>>(5)});
    let timed: bool = tx
        .query_one(
            "SELECT NULLIF(current_setting('wartaq.edit_time', true), '') IS NOT NULL",
            &[],
        )
        .await?
        .get(0);
    if !timed && profile(&current, false, year)? != expected {
        return Err(Error::Conflict);
    }
    let whatsapp = next["whatsapp"].as_str();
    let location = next["location"].as_str();
    let birth_year = next["birthYear"].as_i64().map(|n| n as i32);
    tx.execute("UPDATE students SET first_name = $2, last_name = $3, name = $4, whatsapp = $5, location = $6, birth_year = $7 WHERE id = $1::text::uuid AND active AND ($8::boolean OR teacher_id = $9::text::uuid)", &[&id, &first, &last, &name, &whatsapp, &location, &birth_year, &grant.is_admin(), &grant.id()]).await?;
    Ok(json!({"saved":true}))
}

pub(crate) async fn sync_note(
    tx: &Transaction<'_>,
    subject: &str,
    grant: &Grant,
    input: &Value,
) -> Result<Option<Value>, Error> {
    let operation = &input["operation"];
    if operation["kind"] != "note" {
        return Ok(None);
    }
    let body = input.as_object().ok_or(Error::Invalid)?;
    let op = operation.as_object().ok_or(Error::Invalid)?;
    if body.len() != 2
        || !body.contains_key("accountId")
        || !body.contains_key("operation")
        || op.len() != 2
        || !op.contains_key("event")
        || !op.contains_key("kind")
    {
        return Err(Error::Invalid);
    }
    let account = input["accountId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(Error::Invalid)?;
    let event = crate::ledger::decode_input(&operation["event"]).map_err(|_| Error::Invalid)?;
    let payload = &event["payload"];
    if payload["kind"] != "note" || !payload["studentId"].as_str().is_some_and(valid_uuid) {
        return Err(Error::Invalid);
    }
    if account != subject {
        return Err(Error::Forbidden);
    }
    let id = event["id"].as_str().unwrap();
    let student_id = payload["studentId"].as_str().unwrap();
    let client_time = event["clientTime"].as_str().unwrap();
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended('attendance-ledger', 0))",
        &[],
    )
    .await?;
    let Some(row) = tx.query_opt("SELECT whatsapp FROM students WHERE id = $1::text::uuid AND active AND ($2::boolean OR teacher_id = $3::text::uuid) FOR UPDATE", &[&student_id, &grant.is_admin(), &grant.id()]).await? else {
        return Ok(Some(json!({"id":id,"kind":"forbidden","currentWhatsapp":null})));
    };
    let whatsapp: Option<String> = row.get(0);
    let current_whatsapp = whatsapp.as_deref().map(js_trim).filter(|s| !s.is_empty());
    let existing = tx.query_opt("SELECT teacher_id = $2::text::uuid AND student_id = $3::text::uuid AND client_time = $4::text::timestamptz AND payload = $5::jsonb FROM events WHERE id = $1::text::uuid", &[&id, &grant.id(), &student_id, &client_time, &payload]).await?;
    if let Some(row) = existing {
        let same: Option<bool> = row.get(0);
        return Ok(Some(
            json!({"id":id,"kind":if same == Some(true) {"saved"} else {"id_conflict"},"currentWhatsapp":current_whatsapp}),
        ));
    }
    tx.execute("INSERT INTO events (id, teacher_id, student_id, actor, client_time, payload, enrollment_id) VALUES ($1::text::uuid, $2::text::uuid, $3::text::uuid, $4, $5::text::timestamptz, $6::jsonb, NULL)", &[&id, &grant.id(), &student_id, &grant.name(), &client_time, &payload]).await?;
    Ok(Some(
        json!({"id":id,"kind":"saved","currentWhatsapp":current_whatsapp}),
    ))
}

fn contact_string(value: &Value) -> Result<Option<&str>, Error> {
    if value.is_null() {
        return Ok(None);
    }
    let value = js_trim(value.as_str().ok_or(Error::Invalid)?);
    if value.encode_utf16().count() > 40 {
        return Err(Error::Invalid);
    }
    Ok((!value.is_empty()).then_some(value))
}

fn contact_input(subject: &str, op: &Value) -> Result<String, Error> {
    let o = op.as_object().ok_or(Error::Invalid)?;
    let keys = [
        "kind",
        "id",
        "studentId",
        "clientTime",
        "expectedWhatsapp",
        "whatsapp",
    ];
    if o.len() != keys.len() || keys.iter().any(|k| !o.contains_key(*k)) || op["kind"] != "contact"
    {
        return Err(Error::Invalid);
    }
    for key in ["id", "studentId"] {
        if !op[key].as_str().is_some_and(valid_uuid) {
            return Err(Error::Invalid);
        }
    }
    if !op["clientTime"]
        .as_str()
        .is_some_and(crate::ledger::iso_datetime)
    {
        return Err(Error::Invalid);
    }
    let expected = contact_string(&op["expectedWhatsapp"])?;
    let whatsapp = contact_string(&op["whatsapp"])?;
    let encode = |v: &Value| serde_json::to_string(v).unwrap();
    Ok(format!(
        "{{\"subject\":{},\"operation\":{{\"kind\":\"contact\",\"id\":{},\"studentId\":{},\"clientTime\":{},\"expectedWhatsapp\":{},\"whatsapp\":{}}}}}",
        encode(&json!(subject)),
        encode(&op["id"]),
        encode(&op["studentId"]),
        encode(&op["clientTime"]),
        encode(&json!(expected)),
        encode(&json!(whatsapp))
    ))
}

pub(crate) async fn sync_contact(
    tx: &Transaction<'_>,
    subject: &str,
    grant: &Grant,
    input: &Value,
) -> Result<Option<Value>, Error> {
    let op = &input["operation"];
    if op["kind"] != "contact" {
        return Ok(None);
    }
    let body = input.as_object().ok_or(Error::Invalid)?;
    if body.len() != 2 || !body.contains_key("accountId") || !body.contains_key("operation") {
        return Err(Error::Invalid);
    }
    let account = input["accountId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(Error::Invalid)?;
    let serialized = contact_input(subject, op)?;
    if account != subject {
        return Err(Error::Forbidden);
    }
    let id = op["id"].as_str().unwrap();
    let student_id = op["studentId"].as_str().unwrap();
    let time = op["clientTime"].as_str().unwrap();
    let expected = contact_string(&op["expectedWhatsapp"])?;
    let next = contact_string(&op["whatsapp"])?;
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended('attendance-ledger', 0))",
        &[],
    )
    .await?;
    let Some(row) = tx.query_opt("SELECT whatsapp FROM students WHERE id = $1::text::uuid AND active AND ($2::boolean OR teacher_id = $3::text::uuid) FOR UPDATE", &[&student_id, &grant.is_admin(), &grant.id()]).await? else {
        return Ok(Some(json!({"id":id,"kind":"forbidden","currentWhatsapp":null})));
    };
    let current: Option<String> = row.get(0);
    let current = current.as_deref().map(js_trim).filter(|s| !s.is_empty());
    let fingerprint: String = tx
        .query_one(
            "SELECT encode(sha256(convert_to($1, 'UTF8')), 'hex')",
            &[&serialized],
        )
        .await?
        .get(0);
    let payload = json!({"kind":"contact_receipt","fingerprint":fingerprint});
    if let Some(row) = tx.query_opt("SELECT teacher_id = $2::text::uuid AND student_id = $3::text::uuid AND client_time = $4::text::timestamptz AND payload = $5::jsonb FROM events WHERE id = $1::text::uuid", &[&id, &grant.id(), &student_id, &time, &payload]).await? {
        let same: Option<bool> = row.get(0);
        return Ok(Some(json!({"id":id,"kind":if same == Some(true) {"saved"} else {"id_conflict"},"currentWhatsapp":current})));
    }
    if current != expected {
        return Ok(Some(
            json!({"id":id,"kind":"conflict","currentWhatsapp":current}),
        ));
    }
    tx.execute(
        "UPDATE students SET whatsapp = $2 WHERE id = $1::text::uuid",
        &[&student_id, &next],
    )
    .await?;
    tx.execute("INSERT INTO events (id, teacher_id, student_id, actor, client_time, payload, enrollment_id) VALUES ($1::text::uuid, $2::text::uuid, $3::text::uuid, $4, $5::text::timestamptz, $6::jsonb, NULL)", &[&id, &grant.id(), &student_id, &grant.name(), &time, &payload]).await?;
    Ok(Some(json!({"id":id,"kind":"saved","currentWhatsapp":next})))
}

#[cfg(test)]
mod contact_tests {
    use super::*;
    #[test]
    fn bun_receipt_input_matches_normalized_operation() {
        let op = json!({"kind":"contact","id":"123e4567-e89b-42d3-a456-426614174000","studentId":"123e4567-e89b-42d3-a456-426614174001","clientTime":"2026-01-01T00:00:00Z","expectedWhatsapp":"  ","whatsapp":" 123 "});
        assert_eq!(
            contact_input("subject", &op).unwrap(),
            r#"{"subject":"subject","operation":{"kind":"contact","id":"123e4567-e89b-42d3-a456-426614174000","studentId":"123e4567-e89b-42d3-a456-426614174001","clientTime":"2026-01-01T00:00:00Z","expectedWhatsapp":null,"whatsapp":"123"}}"#
        );
    }
}

const EDITABLE: [&str; 5] = ["firstName", "lastName", "whatsapp", "location", "birthYear"];

fn profile_patch(value: &Value, changes: bool, year: i32) -> Result<Value, Error> {
    let object = value.as_object().ok_or(Error::Invalid)?;
    if object.keys().any(|k| !EDITABLE.contains(&k.as_str())) {
        return Err(Error::Invalid);
    }
    let mut out = serde_json::Map::new();
    for key in EDITABLE {
        let Some(v) = object.get(key) else { continue };
        let normalized = match key {
            "birthYear" => {
                if !v.is_null()
                    && !v
                        .as_i64()
                        .is_some_and(|n| !changes || (1900..=year as i64).contains(&n))
                {
                    return Err(Error::Invalid);
                }
                v.clone()
            }
            "whatsapp" => json!(contact_string(v)?),
            "location" if v.is_null() => Value::Null,
            "location" => {
                let s = v.as_str().ok_or(Error::Invalid)?;
                if changes {
                    let s = js_trim(s);
                    if s.encode_utf16().count() > 200 {
                        return Err(Error::Invalid);
                    }
                    json!((!s.is_empty()).then_some(s))
                } else {
                    json!(s)
                }
            }
            _ => {
                let s = js_trim(v.as_str().ok_or(Error::Invalid)?);
                let n = s.encode_utf16().count();
                if changes && (n > 200 || (key == "firstName" && n == 0)) {
                    return Err(Error::Invalid);
                }
                json!(s)
            }
        };
        out.insert(key.into(), normalized);
    }
    Ok(Value::Object(out))
}

fn ordered_patch(value: &Value) -> String {
    EDITABLE
        .iter()
        .filter_map(|key| {
            value.get(*key).map(|v| {
                format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap(),
                    serde_json::to_string(v).unwrap()
                )
            })
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn profile_input(subject: &str, op: &Value, year: i32) -> Result<(Value, Value, String), Error> {
    let o = op.as_object().ok_or(Error::Invalid)?;
    let keys = [
        "kind",
        "id",
        "studentId",
        "clientTime",
        "expected",
        "changes",
    ];
    if (o.len() != keys.len() && !(o.len() == keys.len() + 1 && o.contains_key("editTime")))
        || keys.iter().any(|k| !o.contains_key(*k))
        || op["kind"] != "profile-v1"
    {
        return Err(Error::Invalid);
    }
    for key in ["id", "studentId"] {
        if !op[key].as_str().is_some_and(valid_uuid) {
            return Err(Error::Invalid);
        }
    }
    if !op["clientTime"]
        .as_str()
        .is_some_and(crate::ledger::iso_datetime)
    {
        return Err(Error::Invalid);
    }
    if let Some(edit_time) = o.get("editTime")
        && !edit_time.as_str().is_some_and(crate::ledger::iso_datetime)
    {
        return Err(Error::Invalid);
    }
    let expected = profile_patch(&op["expected"], false, year)?;
    let changes = profile_patch(&op["changes"], true, year)?;
    if changes.as_object().unwrap().is_empty()
        || EDITABLE
            .iter()
            .any(|k| expected.get(*k).is_some() != changes.get(*k).is_some())
    {
        return Err(Error::Invalid);
    }
    let encode = |v: &Value| serde_json::to_string(v).unwrap();
    let serialized = format!(
        "{{\"subject\":{},\"operation\":{{\"kind\":\"profile-v1\",{}\"id\":{},\"studentId\":{},\"clientTime\":{},\"expected\":{{{}}},\"changes\":{{{}}}}}}}",
        encode(&json!(subject)),
        o.get("editTime")
            .map(|v| format!("\"editTime\":{},", encode(v)))
            .unwrap_or_default(),
        encode(&op["id"]),
        encode(&op["studentId"]),
        encode(&op["clientTime"]),
        ordered_patch(&expected),
        ordered_patch(&changes)
    );
    Ok((expected, changes, serialized))
}

pub(crate) async fn sync_profile(
    tx: &Transaction<'_>,
    subject: &str,
    grant: &Grant,
    input: &Value,
) -> Result<Option<Value>, Error> {
    let op = &input["operation"];
    if op["kind"] != "profile-v1" {
        return Ok(None);
    }
    let body = input.as_object().ok_or(Error::Invalid)?;
    if body.len() != 2 || !body.contains_key("accountId") || !body.contains_key("operation") {
        return Err(Error::Invalid);
    }
    let account = input["accountId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(Error::Invalid)?;
    let year: i32 = tx
        .query_one("SELECT extract(year from now())::int", &[])
        .await?
        .get(0);
    let (expected, changes, serialized) = profile_input(subject, op, year)?;
    if account != subject {
        return Err(Error::Forbidden);
    }
    let id = op["id"].as_str().unwrap();
    let student_id = op["studentId"].as_str().unwrap();
    let time = op["clientTime"].as_str().unwrap();
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended('attendance-ledger', 0))",
        &[],
    )
    .await?;
    let edit_time = op.get("editTime").and_then(Value::as_str);
    if let Some(edit_time) = edit_time {
        let future: bool = tx
            .query_one(
                "SELECT $1::text::timestamptz > clock_timestamp() + interval '5 minutes'",
                &[&edit_time],
            )
            .await?
            .get(0);
        if future {
            return Err(Error::Conflict);
        }
        tx.query_one("SELECT set_config('wartaq.edit_time', $1, true), set_config('wartaq.edit_id', $2, true)", &[&edit_time, &id]).await?;
    }
    let Some(row) = tx.query_opt("SELECT COALESCE(first_name, name), COALESCE(last_name, ''), whatsapp, location, birth_year, COALESCE((edit_time, edit_id) >= ($4::text::timestamptz, $5::text::uuid), false) FROM students WHERE id = $1::text::uuid AND active AND ($2::boolean OR teacher_id = $3::text::uuid) FOR UPDATE", &[&student_id, &grant.is_admin(), &grant.id(), &edit_time, &id]).await? else {
        return Ok(Some(json!({"id":id,"kind":"forbidden","currentWhatsapp":null})));
    };
    let raw = json!({"firstName":row.get::<_,String>(0),"lastName":row.get::<_,String>(1),"whatsapp":row.get::<_,Option<String>>(2),"location":row.get::<_,Option<String>>(3),"birthYear":row.get::<_,Option<i32>>(4)});
    let current = profile_patch(&raw, false, year)?;
    let whatsapp = current["whatsapp"].clone();
    let fingerprint: String = tx
        .query_one(
            "SELECT encode(sha256(convert_to($1, 'UTF8')), 'hex')",
            &[&serialized],
        )
        .await?
        .get(0);
    let payload = json!({"kind":"contact_receipt","fingerprint":fingerprint});
    if let Some(row) = tx.query_opt("SELECT teacher_id = $2::text::uuid AND student_id = $3::text::uuid AND client_time = $4::text::timestamptz AND payload = $5::jsonb FROM events WHERE id = $1::text::uuid", &[&id, &grant.id(), &student_id, &time, &payload]).await? {
        let same: Option<bool> = row.get(0);
        return Ok(Some(json!({"id":id,"kind":if same == Some(true) {"saved"} else {"id_conflict"},"currentWhatsapp":whatsapp,"profile":current})));
    }
    let stale: bool = row.get(5);
    let mut next = current.clone();
    let mut conflicts = Vec::new();
    for key in EDITABLE {
        if let Some(value) = changes.get(key) {
            if if edit_time.is_some() {
                stale
            } else {
                current[key] != expected[key] && current[key] != *value
            } {
                conflicts.push(key);
            }
            next[key] = value.clone();
        }
    }
    let first = next["firstName"].as_str().unwrap();
    let last = next["lastName"].as_str().unwrap();
    let name = if last.is_empty() {
        first.to_owned()
    } else {
        format!("{first} {last}")
    };
    if name.encode_utf16().count() > 200 {
        for key in ["firstName", "lastName"] {
            if changes.get(key).is_some() && !conflicts.contains(&key) {
                conflicts.push(key);
            }
        }
    }
    if !conflicts.is_empty() {
        return Ok(Some(
            json!({"id":id,"kind":"conflict","currentWhatsapp":whatsapp,"profile":current,"profileConflicts":conflicts}),
        ));
    }
    let first_changed = next["firstName"] != current["firstName"];
    let last_changed = next["lastName"] != current["lastName"];
    let phone_changed = next["whatsapp"] != current["whatsapp"];
    let location_changed = next["location"] != current["location"];
    let year_changed = next["birthYear"] != current["birthYear"];
    let phone = next["whatsapp"].as_str();
    let location = next["location"].as_str();
    let birth = next["birthYear"].as_i64().map(|n| n as i32);
    if first_changed
        || last_changed
        || phone_changed
        || location_changed
        || year_changed
        || edit_time.is_some()
    {
        tx.execute("UPDATE students SET first_name = CASE WHEN $2 THEN $3 ELSE first_name END, last_name = CASE WHEN $4 THEN $5 ELSE last_name END, name = CASE WHEN $2 OR $4 THEN $6 ELSE name END, whatsapp = CASE WHEN $7 THEN $8 ELSE whatsapp END, location = CASE WHEN $9 THEN $10 ELSE location END, birth_year = CASE WHEN $11 THEN $12 ELSE birth_year END WHERE id = $1::text::uuid", &[&student_id,&first_changed,&first,&last_changed,&last,&name,&phone_changed,&phone,&location_changed,&location,&year_changed,&birth]).await?;
    }
    tx.execute("INSERT INTO events (id, teacher_id, student_id, actor, client_time, payload, enrollment_id) VALUES ($1::text::uuid, $2::text::uuid, $3::text::uuid, $4, $5::text::timestamptz, $6::jsonb, NULL)", &[&id, &grant.id(), &student_id, &grant.name(), &time, &payload]).await?;
    Ok(Some(
        json!({"id":id,"kind":"saved","currentWhatsapp":next["whatsapp"],"profile":next}),
    ))
}

#[cfg(test)]
mod profile_sync_tests {
    use super::*;
    #[test]
    fn timed_receipt_preserves_edit_time() {
        let op = json!({"kind":"profile-v1","editTime":"2026-01-01T00:00:00Z","id":"123e4567-e89b-42d3-a456-426614174000","studentId":"123e4567-e89b-42d3-a456-426614174001","clientTime":"2026-01-01T00:00:00Z","expected":{"firstName":" A "},"changes":{"firstName":" B "}});
        let (_, _, serialized) = profile_input("subject", &op, 2026).unwrap();
        assert_eq!(
            serialized,
            r#"{"subject":"subject","operation":{"kind":"profile-v1","editTime":"2026-01-01T00:00:00Z","id":"123e4567-e89b-42d3-a456-426614174000","studentId":"123e4567-e89b-42d3-a456-426614174001","clientTime":"2026-01-01T00:00:00Z","expected":{"firstName":"A"},"changes":{"firstName":"B"}}}"#
        );
    }
    #[test]
    fn normalized_receipt_matches_bun() {
        let op = json!({"kind":"profile-v1","id":"123e4567-e89b-42d3-a456-426614174000","studentId":"123e4567-e89b-42d3-a456-426614174001","clientTime":"2026-01-01T00:00:00Z","expected":{"location":" old ","firstName":" A ","whatsapp":" "},"changes":{"location":" new ","firstName":" B ","whatsapp":" 123 "}});
        let (_, _, serialized) = profile_input("subject", &op, 2026).unwrap();
        assert_eq!(
            serialized,
            r#"{"subject":"subject","operation":{"kind":"profile-v1","id":"123e4567-e89b-42d3-a456-426614174000","studentId":"123e4567-e89b-42d3-a456-426614174001","clientTime":"2026-01-01T00:00:00Z","expected":{"firstName":"A","whatsapp":null,"location":" old "},"changes":{"firstName":"B","whatsapp":"123","location":"new"}}}"#
        );
    }
}
