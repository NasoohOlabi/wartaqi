use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;

use crate::credentials;

pub(crate) enum SignInOutcome {
    InvalidCredentials,
    SignedIn {
        token: String,
        user: Value,
        remember: bool,
    },
}

#[derive(Debug)]
pub(crate) enum SignInError {
    Database,
    Random,
    Worker,
}

impl From<tokio_postgres::Error> for SignInError {
    fn from(_: tokio_postgres::Error) -> Self {
        Self::Database
    }
}

pub(crate) fn session_token() -> Result<String, SignInError> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let random = SystemRandom::new();
    let mut token = String::with_capacity(32);
    while token.len() < 32 {
        let mut bytes = [0; 64];
        random.fill(&mut bytes).map_err(|_| SignInError::Random)?;
        for byte in bytes {
            if byte < 248 {
                token.push(ALPHABET[(byte % 62) as usize] as char);
                if token.len() == 32 {
                    break;
                }
            }
        }
    }
    Ok(token)
}

// The caller validates and normalizes the phone and request body. The transaction
// shares the existing auth-mutations advisory lock with the Bun server.
pub(crate) async fn sign_in(
    db: &mut tokio_postgres::Client,
    phone: &str,
    password: &str,
    remember: bool,
    ip_address: &str,
    user_agent: &str,
) -> Result<SignInOutcome, SignInError> {
    let tx = db.transaction().await?;
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtextextended('auth-mutations', 0))",
        &[],
    )
    .await?;
    let row = tx.query_opt(r#"
        SELECT u.id::text, c.password, jsonb_build_object(
          'id', u.id::text, 'name', u.name, 'email', u.email, 'emailVerified', u."emailVerified", 'image', u.image,
          'createdAt', to_char(u."createdAt" AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'),
          'updatedAt', to_char(u."updatedAt" AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'))
        FROM app_accounts a JOIN neon_auth."user" u ON u.id::text = a.auth_user_id
        LEFT JOIN LATERAL (SELECT password FROM neon_auth.account
          WHERE "userId" = u.id AND "providerId" = 'credential' AND "accountId" = u.id::text LIMIT 1) c ON true
        WHERE a.verified_phone = $1 AND a.active AND (a.expires_at IS NULL OR a.expires_at > now())
          AND ((a.role = 'student' AND EXISTS (SELECT 1 FROM students s WHERE s.id = a.student_id AND s.active))
            OR (a.role IN ('teacher', 'administrator') AND EXISTS (SELECT 1 FROM teachers t
              WHERE t.id = a.teacher_id AND t.active AND (t.expires_at IS NULL OR t.expires_at > now()))))
        LIMIT 1
    "#, &[&phone]).await?;
    // Perform the same expensive derivation when no eligible credential exists.
    const DUMMY: &str = "195481588d82f3a3f1c05d7546f5fddf:2b0f4f9a0f46e3a7ec5cf693db034684e395b98a0eac74c88d0c6aaa6e9f4b168677cd6c09aff9e2cc6863c3ac68d3f08c9a3b80e9da2ee4219deabdd21cc0a6";
    let stored = row.as_ref().and_then(|r| r.get::<_, Option<String>>(1));
    let hash = stored.as_deref().unwrap_or(DUMMY).to_owned();
    let password = password.to_owned();
    let valid = tokio::task::spawn_blocking(move || credentials::verify_password(&password, &hash))
        .await
        .map_err(|_| SignInError::Worker)?;
    if !valid || stored.is_none() {
        tx.commit().await?;
        return Ok(SignInOutcome::InvalidCredentials);
    }
    let row = row.expect("a stored credential requires a user row");
    let user_id: String = row.get(0);
    let user: Value = row.get(2);
    let token = session_token()?;
    let lifetime: i32 = if remember { 604800 } else { 86400 };
    tx.execute(r#"
        INSERT INTO neon_auth.session (id, "userId", token, "expiresAt", "createdAt", "updatedAt", "ipAddress", "userAgent")
        VALUES (gen_random_uuid(), $1::text::uuid, $2, date_trunc('milliseconds', statement_timestamp()) + $3::int * interval '1 second',
          date_trunc('milliseconds', statement_timestamp()), date_trunc('milliseconds', statement_timestamp()), $4, $5)
    "#, &[&user_id, &token, &lifetime, &ip_address, &user_agent]).await?;
    tx.commit().await?;
    Ok(SignInOutcome::SignedIn {
        token,
        user,
        remember,
    })
}

#[cfg(test)]
mod tests {
    use super::session_token;
    #[test]
    fn token_has_better_auth_format_and_is_fresh() {
        let first = session_token().unwrap();
        let second = session_token().unwrap();
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        assert_ne!(first, second);
    }
}
