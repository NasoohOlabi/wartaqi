use crate::{access::Grant, writes::Error};
use serde_json::{Value, json};

fn keys(v: &Value, names: &[&str]) -> bool {
    v.as_object()
        .is_some_and(|o| o.keys().all(|k| names.contains(&k.as_str())))
}
fn grant(v: &Value) -> bool {
    keys(v, &["id", "expiresAt"])
        && v["id"].as_str().is_some_and(crate::ledger::valid_uuid)
        && (v.get("expiresAt").is_some_and(Value::is_null)
            || v["expiresAt"]
                .as_str()
                .is_some_and(crate::ledger::iso_datetime))
}
fn operation(v: &Value) -> bool {
    if !keys(v, &["kind", "requestId", "id", "grant", "fields"])
        || v["kind"] != "grade-edit"
        || !v["requestId"]
            .as_str()
            .is_some_and(crate::ledger::valid_uuid)
        || !v["id"].as_str().is_some_and(crate::ledger::valid_uuid)
        || !grant(&v["grant"])
    {
        return false;
    }
    let f = &v["fields"];
    if !keys(f, &["practical", "theory", "notes"]) || f.as_object().is_none_or(|o| o.is_empty()) {
        return false;
    }
    ["practical", "theory", "notes"].iter().all(|name| {
        f.get(name).is_none_or(|c| {
            keys(c, &["previous", "next"])
                && ["previous", "next"].iter().all(|k| {
                    c.get(k).is_some_and(|x| {
                        if *name == "notes" {
                            x.is_string()
                        } else {
                            x.is_null() || x.as_f64().is_some_and(|n| (0.0..=100.0).contains(&n))
                        }
                    })
                })
        })
    })
}
fn encoded(v: &Value) -> String {
    if let Some(n) = v.as_f64() {
        ryu_js::Buffer::new().format_finite(n).to_owned()
    } else {
        serde_json::to_string(v).unwrap()
    }
}
fn fingerprint_input(subject: &str, op: &Value) -> String {
    let g = &op["grant"];
    let mut fields = Vec::new();
    for name in ["practical", "theory", "notes"] {
        if let Some(c) = op["fields"].get(name) {
            fields.push(format!(
                "\"{}\":{{\"previous\":{},\"next\":{}}}",
                name,
                encoded(&c["previous"]),
                encoded(&c["next"])
            ));
        }
    }
    format!(
        "[{},{{\"kind\":\"grade-edit\",\"requestId\":{},\"id\":{},\"grant\":{{\"id\":{},\"expiresAt\":{}}},\"fields\":{{{}}}}}]",
        encoded(&json!(subject)),
        encoded(&op["requestId"]),
        encoded(&op["id"]),
        encoded(&g["id"]),
        encoded(&g["expiresAt"]),
        fields.join(",")
    )
}

pub(crate) async fn apply(
    tx: &tokio_postgres::Transaction<'_>,
    subject: &str,
    access: &Grant,
    input: &Value,
) -> Result<Option<Value>, Error> {
    let (op, submitted) = if operation(input) {
        (input, &input["grant"])
    } else if keys(input, &["kind", "operation", "grant"])
        && input["kind"] == "recover-grade-edit"
        && operation(&input["operation"])
        && grant(&input["grant"])
    {
        (&input["operation"], &input["grant"])
    } else {
        return Ok(None);
    };
    if op["grant"]["id"].as_str() != Some(access.id())
        || submitted["id"].as_str() != Some(access.id())
        || submitted["expiresAt"].as_str() != access.expires_at()
    {
        return Ok(Some(json!({"status":"blocked"})));
    }
    let request = op["requestId"].as_str().unwrap();
    let id = op["id"].as_str().unwrap();
    let serialized = fingerprint_input(subject, op);
    let fingerprint: String = tx
        .query_one(
            "SELECT encode(sha256(convert_to($1, 'UTF8')), 'hex')",
            &[&serialized],
        )
        .await?
        .get(0);
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended('grade-sync:' || $1, 0))",
        &[&request],
    )
    .await?;
    if let Some(row) = tx
        .query_opt(
            "SELECT fingerprint FROM grade_sync_receipts WHERE request_id = $1::text::uuid",
            &[&request],
        )
        .await?
    {
        let existing: String = row.get(0);
        return Ok(Some(
            json!({"status": if existing == fingerprint { "accepted" } else { "blocked" }}),
        ));
    }
    let Some(row) = tx.query_opt("SELECT practical::float8, theory::float8, notes FROM course_enrollments WHERE id = $1::text::uuid FOR UPDATE", &[&id]).await? else { return Ok(Some(json!({"status":"blocked"}))) };
    let practical: Option<f64> = row.get(0);
    let theory: Option<f64> = row.get(1);
    let notes: String = row.get(2);
    let current = json!({"practical": practical, "theory": theory, "notes": notes});
    let mut next = current.clone();
    let mut changed = false;
    for name in ["practical", "theory", "notes"] {
        if let Some(change) = op["fields"].get(name) {
            let equal = |a: &Value, b: &Value| {
                if name == "notes" {
                    a == b
                } else {
                    a.as_f64() == b.as_f64()
                }
            };
            if !equal(&current[name], &change["previous"])
                && !equal(&current[name], &change["next"])
            {
                return Ok(Some(json!({"status":"conflict", "current":current})));
            }
            if !equal(&current[name], &change["next"]) {
                next[name] = change["next"].clone();
                changed = true;
            }
        }
    }
    if changed {
        let p = next["practical"]
            .as_f64()
            .map(|_| encoded(&next["practical"]));
        let t = next["theory"].as_f64().map(|_| encoded(&next["theory"]));
        let n = next["notes"].as_str().unwrap();
        tx.execute("UPDATE course_enrollments SET practical = $2::text::numeric, theory = $3::text::numeric, notes = $4 WHERE id = $1::text::uuid", &[&id, &p, &t, &n]).await?;
    }
    tx.execute(
        "INSERT INTO grade_sync_receipts (request_id, fingerprint) VALUES ($1::text::uuid, $2)",
        &[&request, &fingerprint],
    )
    .await?;
    Ok(Some(json!({"status":"accepted"})))
}
