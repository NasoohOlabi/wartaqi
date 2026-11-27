use axum::{
    Json,
    body::to_bytes,
    extract::Request,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_postgres::Transaction;

use crate::ledger;

const PREFIX: &str = "wartaq:telegram:";

#[derive(Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
enum Input {
    #[serde(rename = "login")]
    Login { phone: String },
    #[serde(rename = "login-phone")]
    LoginPhone { phone: String },
    #[serde(rename = "attendance")]
    Attendance { token: String },
    #[serde(rename = "unlink")]
    Unlink { token: String },
}

fn phone(value: &str, roster: bool) -> Option<String> {
    let value = ledger::js_trim(value);
    if value.encode_utf16().count() > 40 {
        return None;
    }
    let roster_compact: String = value
        .chars()
        .filter(|c| !matches!(c, ' ' | '(' | ')' | '-'))
        .collect();
    let add_plus = roster
        && (11..=15).contains(&roster_compact.len())
        && !roster_compact.starts_with('0')
        && roster_compact.bytes().all(|c| c.is_ascii_digit());
    let compact: String = value
        .chars()
        .filter_map(|c| match c {
            '\u{0660}'..='\u{0669}' => Some(char::from(b'0' + (c as u32 - 0x660) as u8)),
            '\u{06f0}'..='\u{06f9}' => Some(char::from(b'0' + (c as u32 - 0x6f0) as u8)),
            ' ' | '(' | ')' | '-' => None,
            _ => Some(c),
        })
        .collect();
    let compact = if let Some(rest) = compact.strip_prefix("00") {
        format!("+{rest}")
    } else if compact.len() == 10
        && compact.starts_with("09")
        && compact.bytes().all(|c| c.is_ascii_digit())
    {
        format!("+963{}", &compact[1..])
    } else if add_plus {
        format!("+{compact}")
    } else {
        compact
    };
    let bytes = compact.as_bytes();
    (bytes.len() >= 8
        && bytes.len() <= 16
        && bytes[0] == b'+'
        && (b'1'..=b'9').contains(&bytes[1])
        && bytes[2..].iter().all(u8::is_ascii_digit))
    .then_some(compact)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn hash(value: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, value).as_ref())
}

pub(crate) fn binding(fields: &[&str]) -> String {
    hash(
        serde_json::to_string(fields)
            .expect("strings serialize")
            .as_bytes(),
    )
}

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

fn unavailable() -> Response {
    reply(StatusCode::FORBIDDEN, json!({"error":"Unavailable"}))
}

async fn roster_student(
    tx: &Transaction<'_>,
    expected: &str,
) -> Result<Option<String>, tokio_postgres::Error> {
    let rows = tx.query("SELECT s.id::text, s.whatsapp, (a.auth_user_id IS NULL OR (a.active AND (a.expires_at IS NULL OR a.expires_at > now()))) AS allowed FROM students s LEFT JOIN app_accounts a ON a.student_id = s.id WHERE s.active AND s.whatsapp IS NOT NULL", &[]).await?;
    let mut matching = rows
        .iter()
        .filter(|row| phone(row.get::<_, &str>(1), true).as_deref() == Some(expected));
    let first = matching.next();
    Ok(match (first, matching.next()) {
        (Some(row), None) if row.get::<_, bool>(2) => Some(row.get(0)),
        _ => None,
    })
}

async fn login(
    tx: &Transaction<'_>,
    value: &str,
    allow_teacher: bool,
) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
    let Some(phone) = phone(value, false) else {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    };
    let row = tx.query_opt(r#"SELECT a.auth_user_id, a.verified_phone, a.student_id::text, a.teacher_id::text, a.role
        FROM app_accounts a JOIN neon_auth."user" u ON u.id::text = a.auth_user_id
        LEFT JOIN students s ON s.id = a.student_id LEFT JOIN teachers t ON t.id = a.teacher_id
        WHERE a.verified_phone = $1 AND a.active AND (a.expires_at IS NULL OR a.expires_at > now())
        AND ((a.role = 'student' AND s.active) OR ($2::boolean AND a.role = 'teacher' AND t.active AND (t.expires_at IS NULL OR t.expires_at > now()))) FOR UPDATE OF a"#, &[&phone, &allow_teacher]).await?;
    let grant = if let Some(row) = row {
        let subject: String = row.get(0);
        let verified: String = row.get(1);
        let role: String = row.get(4);
        let id: Option<String> = if role == "student" {
            row.get(2)
        } else {
            row.get(3)
        };
        let Some(id) = id else {
            return Ok(unavailable());
        };
        if !ledger::valid_uuid(&subject) || !ledger::valid_uuid(&id) {
            return Ok(unavailable());
        }
        let binding = if role == "student" {
            binding(&[&subject, &verified, &id])
        } else {
            binding(&["teacher", &subject, &verified, &id])
        };
        Some((subject, role, binding))
    } else if allow_teacher {
        roster_student(tx, &phone).await?.map(|id| {
            (
                format!("roster:{id}"),
                "student".into(),
                binding(&["roster", &id, &phone]),
            )
        })
    } else {
        None
    };
    let Some((subject, role, grant_binding)) = grant else {
        return Ok(unavailable());
    };
    let mut random = [0u8; 32];
    SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| "random generation failed")?;
    let token = URL_SAFE_NO_PAD.encode(random);
    let identifier = format!("{PREFIX}{}:{subject}", hash(token.as_bytes()));
    tx.execute("DELETE FROM neon_auth.verification WHERE starts_with(identifier, $1) AND \"expiresAt\" <= now()", &[&PREFIX]).await?;
    tx.execute("INSERT INTO neon_auth.verification (id, identifier, value, \"expiresAt\") VALUES (gen_random_uuid(), $1, $2, now() + interval '30 days')", &[&identifier, &grant_binding]).await?;
    Ok(reply(
        StatusCode::OK,
        if allow_teacher {
            json!({"token":token,"role":role})
        } else {
            json!({"token":token})
        },
    ))
}

async fn attendance(
    tx: &Transaction<'_>,
    token: &str,
) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
    let prefix = format!("{PREFIX}{}:", hash(token.as_bytes()));
    let Some(credential) = tx.query_opt("SELECT identifier, value FROM neon_auth.verification WHERE starts_with(identifier, $1) AND \"expiresAt\" > now()", &[&prefix]).await? else { return Ok(unavailable()); };
    let identifier: String = credential.get(0);
    let expected: String = credential.get(1);
    let suffix = &identifier[prefix.len()..];
    let roster = suffix.strip_prefix("roster:");
    let subject = roster.unwrap_or(suffix);
    if !ledger::valid_uuid(subject) {
        return Ok(unavailable());
    }
    let student = if roster.is_some() {
        let Some(row) = tx.query_opt("SELECT id::text, name, group_name, whatsapp FROM students WHERE id = $1::text::uuid AND active", &[&subject]).await? else { return Ok(unavailable()); };
        let Some(number) = row
            .get::<_, Option<String>>(3)
            .and_then(|value| phone(&value, true))
        else {
            return Ok(unavailable());
        };
        if binding(&["roster", subject, &number]) != expected
            || roster_student(tx, &number).await?.as_deref() != Some(subject)
        {
            return Ok(unavailable());
        }
        row
    } else {
        let Some(account) = tx.query_opt("SELECT a.student_id::text, a.verified_phone FROM app_accounts a JOIN neon_auth.\"user\" u ON u.id::text = a.auth_user_id WHERE a.auth_user_id = $1 AND a.role = 'student' AND a.active AND (a.expires_at IS NULL OR a.expires_at > now()) FOR UPDATE OF a", &[&subject]).await? else { return Ok(unavailable()); };
        let Some(id) = account.get::<_, Option<String>>(0) else {
            return Ok(unavailable());
        };
        let Some(number) = account
            .get::<_, Option<String>>(1)
            .filter(|v| !v.is_empty())
        else {
            return Ok(unavailable());
        };
        if binding(&[subject, &number, &id]) != expected {
            return Ok(unavailable());
        }
        if tx.query_opt("SELECT 1 FROM neon_auth.verification WHERE identifier = $1 AND value = $2 AND \"expiresAt\" > now() FOR SHARE", &[&identifier, &expected]).await?.is_none() { return Ok(unavailable()); }
        let Some(row) = tx.query_opt("SELECT id::text, name, group_name, whatsapp FROM students WHERE id = $1::text::uuid AND active FOR SHARE", &[&id]).await? else { return Ok(unavailable()); };
        row
    };
    let result =
        ledger::student_attendance(tx, student.get(0), student.get(1), student.get(2)).await?;
    Ok(reply(StatusCode::OK, result))
}

pub(crate) async fn handle(
    pool: &deadpool_postgres::Pool,
    request: Request,
    secret: Option<&str>,
) -> Response {
    let expected = format!("Bearer {}", secret.unwrap_or_default());
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .map(|h| h.as_bytes())
        .unwrap_or_default();
    #[allow(deprecated)]
    if secret.is_none_or(str::is_empty)
        || ring::constant_time::verify_slices_are_equal(supplied, expected.as_bytes()).is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if request.method() != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Ok(body) = to_bytes(request.into_body(), 1024).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(input) = serde_json::from_slice::<Input>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut client = match pool.get().await {
        Ok(client) => client,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let tx = match client.transaction().await {
        Ok(tx) => tx,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let result = match input {
        Input::Login { phone } => login(&tx, &phone, false).await,
        Input::LoginPhone { phone } => login(&tx, &phone, true).await,
        Input::Attendance { token } | Input::Unlink { token } if !valid_token(&token) => {
            return StatusCode::BAD_REQUEST.into_response();
        }
        Input::Attendance { token } => attendance(&tx, &token).await,
        Input::Unlink { token } => {
            let prefix = format!("{PREFIX}{}:", hash(token.as_bytes()));
            tx.execute(
                "DELETE FROM neon_auth.verification WHERE starts_with(identifier, $1)",
                &[&prefix],
            )
            .await
            .map(|_| reply(StatusCode::OK, json!({"ok":true})))
            .map_err(Into::into)
        }
    };
    match result {
        Ok(response) => match tx.commit().await {
            Ok(()) => response,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Err(error) if error.is::<ledger::InvalidData>() => {
            reply(StatusCode::BAD_REQUEST, json!({"error":"Invalid request"}))
        }
        Err(_) => reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"Server error"}),
        ),
    }
}

pub(crate) fn valid_token(token: &str) -> bool {
    token.len() == 43
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::{binding, handle, phone, valid_token};
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use ring::rand::{SecureRandom, SystemRandom};
    use serde_json::{Value, json};

    #[test]
    fn normalization_and_grant_binding() {
        assert_eq!(
            phone("0995 120 034", false).as_deref(),
            Some("+963995120034")
        );
        assert_eq!(
            phone("1 202 555 0791", true).as_deref(),
            Some("+12025550791")
        );
        assert_eq!(phone("1 202 555 0791", false), None);
        assert_eq!(binding(&["roster", "student", "+1234567"]).len(), 64);
        assert!(valid_token(&"A".repeat(43)));
        assert!(!valid_token(&"A".repeat(42)));
    }

    #[tokio::test]
    async fn roster_grant_round_trip_in_disposable_database() {
        let Ok(database_url) = std::env::var("TEST_RAILWAY_DATABASE_URL") else {
            return;
        };
        let config: tokio_postgres::Config = database_url.parse().unwrap();
        let pool = deadpool_postgres::Pool::builder(deadpool_postgres::Manager::new(
            config,
            tokio_postgres::NoTls,
        ))
        .runtime(deadpool_postgres::Runtime::Tokio1)
        .build()
        .unwrap();
        let db = pool.get().await.unwrap();
        let mut random = [0u8; 4];
        SystemRandom::new().fill(&mut random).unwrap();
        let phone = format!("+199{:09}", u32::from_be_bytes(random) % 1_000_000_000);
        let row = db.query_one("INSERT INTO students (id, name, whatsapp) VALUES (gen_random_uuid(), 'Synthetic telegram test', $1) RETURNING id::text", &[&phone]).await.unwrap();
        let student: String = row.get(0);
        let request = |value: Value| {
            Request::builder()
                .method("POST")
                .uri("/api/telegram/student")
                .header("authorization", "Bearer synthetic-secret")
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap()
        };
        let response = handle(
            &pool,
            request(json!({"action":"login-phone", "phone":phone})),
            Some("synthetic-secret"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let login: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(login["role"], "student");
        let token = login["token"].as_str().unwrap();
        let response = handle(
            &pool,
            request(json!({"action":"attendance", "token":token})),
            Some("synthetic-secret"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let attendance: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(attendance["student"]["studentId"], student);
        assert_eq!(attendance["attendance"], json!([]));
        let response = handle(
            &pool,
            request(json!({"action":"unlink", "token":token})),
            Some("synthetic-secret"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = handle(
            &pool,
            request(json!({"action":"attendance", "token":token})),
            Some("synthetic-secret"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        db.execute(
            "DELETE FROM students WHERE id = $1::text::uuid",
            &[&student],
        )
        .await
        .unwrap();
    }
}
