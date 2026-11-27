use axum::http::{HeaderMap, header};
use tokio_postgres::{Client, Transaction};

use crate::{
    access::{self, Grant},
    ledger, telegram_student,
};

pub(crate) struct Credential {
    subject: String,
    identifier: String,
}

impl Credential {
    pub(crate) fn subject(&self) -> &str {
        &self.subject
    }
}

// This is the private worker's first gate. MCP host/origin and method validation
// remain the transport's responsibility after identification.
pub(crate) async fn identify(
    db: &Client,
    headers: &HeaderMap,
    secret: Option<&str>,
) -> Result<Option<Credential>, tokio_postgres::Error> {
    let supplied = headers
        .get(header::AUTHORIZATION)
        .map(|value| value.as_bytes())
        .unwrap_or_default();
    let expected = format!("Bearer {}", secret.unwrap_or_default());
    #[allow(deprecated)]
    if secret.is_none_or(str::is_empty)
        || ring::constant_time::verify_slices_are_equal(supplied, expected.as_bytes()).is_err()
    {
        return Ok(None);
    }
    let Some(token) = headers
        .get("x-telegram-grant")
        .and_then(|value| value.to_str().ok())
        .filter(|token| telegram_student::valid_token(token))
    else {
        return Ok(None);
    };
    let prefix = format!(
        "wartaq:telegram:{}:",
        telegram_student::hash(token.as_bytes())
    );
    let Some(row) = db.query_opt("SELECT identifier FROM neon_auth.verification WHERE starts_with(identifier, $1) AND \"expiresAt\" > now()", &[&prefix]).await? else {
        return Ok(None);
    };
    let identifier: String = row.get(0);
    let subject = &identifier[prefix.len()..];
    if !ledger::valid_uuid(subject) {
        return Ok(None);
    }
    Ok(Some(Credential {
        subject: subject.to_owned(),
        identifier,
    }))
}

// Call for every notebook operation within the same transaction as the operation.
// This rechecks account access and the token binding after taking account locks.
pub(crate) async fn authorize(
    tx: &Transaction<'_>,
    credential: &Credential,
) -> Result<Option<Grant>, tokio_postgres::Error> {
    let Some(grant) = access::load(tx, credential.subject()).await? else {
        return Ok(None);
    };
    if grant.role() != "teacher" {
        return Ok(None);
    }
    let row = tx
        .query_opt(
            "SELECT verified_phone FROM app_accounts WHERE auth_user_id = $1",
            &[&credential.subject],
        )
        .await?;
    let Some(phone) = row
        .and_then(|row| row.get::<_, Option<String>>(0))
        .filter(|phone| !phone.is_empty())
    else {
        return Ok(None);
    };
    let binding = telegram_student::binding(&["teacher", credential.subject(), &phone, grant.id()]);
    let allowed = tx.query_opt("SELECT 1 FROM neon_auth.verification WHERE identifier = $1 AND value = $2 AND \"expiresAt\" > now() AND EXISTS (SELECT 1 FROM neon_auth.\"user\" WHERE id = $3::text::uuid) FOR SHARE", &[&credential.identifier, &binding, &credential.subject]).await?.is_some();
    Ok(allowed.then_some(grant))
}

#[cfg(test)]
mod tests {
    use super::{authorize, identify};
    use crate::telegram_student::{binding, hash};
    use axum::http::{HeaderMap, HeaderValue, header};

    #[tokio::test]
    async fn teacher_grant_is_revalidated_for_each_operation() {
        let Ok(database_url) = std::env::var("TEST_RAILWAY_DATABASE_URL") else {
            return;
        };
        let (mut db, connection) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls)
            .await
            .unwrap();
        let connection_task = tokio::spawn(async move { connection.await.unwrap() });
        let user: String = db.query_one("INSERT INTO neon_auth.\"user\" (id, name, email) SELECT id, 'Synthetic Telegram teacher', id::text || '@example.invalid' FROM (SELECT gen_random_uuid() id) v RETURNING id::text", &[]).await.unwrap().get(0);
        let teacher: String = db.query_one("INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES (gen_random_uuid(), $1, 'Synthetic Telegram teacher', 'read_write') RETURNING id::text", &[&user]).await.unwrap().get(0);
        let phone = "+19990000002";
        db.execute("INSERT INTO app_accounts (auth_user_id, role, teacher_id, verified_phone) VALUES ($1, 'teacher', $2::text::uuid, $3)", &[&user, &teacher, &phone]).await.unwrap();
        let token = "T".repeat(43);
        let identifier = format!("wartaq:telegram:{}:{user}", hash(token.as_bytes()));
        let value = binding(&["teacher", &user, phone, &teacher]);
        db.execute("INSERT INTO neon_auth.verification (id, identifier, value, \"expiresAt\") VALUES (gen_random_uuid(), $1, $2, now() + interval '1 day')", &[&identifier, &value]).await.unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer synthetic-secret"),
        );
        headers.insert("x-telegram-grant", HeaderValue::from_str(&token).unwrap());
        assert!(
            identify(&db, &headers, Some("wrong"))
                .await
                .unwrap()
                .is_none()
        );
        let credential = identify(&db, &headers, Some("synthetic-secret"))
            .await
            .unwrap()
            .unwrap();
        let tx = db.transaction().await.unwrap();
        assert!(authorize(&tx, &credential).await.unwrap().is_some());
        tx.commit().await.unwrap();
        db.execute(
            "UPDATE app_accounts SET active = false WHERE auth_user_id = $1",
            &[&user],
        )
        .await
        .unwrap();
        let tx = db.transaction().await.unwrap();
        assert!(authorize(&tx, &credential).await.unwrap().is_none());
        tx.commit().await.unwrap();
        db.execute(
            "DELETE FROM neon_auth.verification WHERE identifier = $1",
            &[&identifier],
        )
        .await
        .unwrap();
        db.execute("DELETE FROM app_accounts WHERE auth_user_id = $1", &[&user])
            .await
            .unwrap();
        db.execute(
            "DELETE FROM teachers WHERE id = $1::text::uuid",
            &[&teacher],
        )
        .await
        .unwrap();
        db.execute(
            "DELETE FROM neon_auth.\"user\" WHERE id = $1::text::uuid",
            &[&user],
        )
        .await
        .unwrap();
        drop(db);
        connection_task.await.unwrap();
    }
}
