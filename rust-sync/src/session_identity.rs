use axum::http::{HeaderMap, header::COOKIE};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ring::hmac;

// Better Call decodes cookie values with decodeURIComponent, preserving the original on error.
fn decode(value: &str) -> String {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let Some((a, b)) = input.next().zip(input.next()) else {
                return value.to_owned();
            };
            let Some((a, b)) = (a as char).to_digit(16).zip((b as char).to_digit(16)) else {
                return value.to_owned();
            };
            bytes.push((a * 16 + b) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| value.to_owned())
}

pub(crate) fn token(headers: &HeaderMap, secret: &str, secure: bool) -> Option<String> {
    let name = if secure {
        "__Secure-better-auth.session_token"
    } else {
        "better-auth.session_token"
    };
    let raw = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|part| part.split_once('='))
        .find_map(|(key, value)| (key.trim() == name).then_some(value.trim()))?;
    let raw = if raw.starts_with('"') {
        raw.get(1..raw.len().saturating_sub(1)).unwrap_or("")
    } else {
        raw
    };
    let value = decode(raw);
    let (token, signature) = value.rsplit_once('.')?;
    if token.is_empty() || signature.len() != 44 || !signature.ends_with('=') {
        return None;
    }
    let signature = STANDARD.decode(signature).ok()?;
    hmac::verify(
        &hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes()),
        token.as_bytes(),
        &signature,
    )
    .ok()?;
    Some(token.to_owned())
}

pub(crate) async fn user_id(
    db: &impl tokio_postgres::GenericClient,
    token: &str,
) -> Result<Option<String>, tokio_postgres::Error> {
    Ok(db.query_opt(
        "SELECT s.\"userId\"::text FROM neon_auth.session s JOIN neon_auth.\"user\" u ON u.id = s.\"userId\" WHERE s.token = $1 AND s.\"expiresAt\" > statement_timestamp()",
        &[&token],
    ).await?.map(|row| row.get(0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn verifies_bun_hmac_cookie_and_rejects_invalid_inputs() {
        let mut headers = HeaderMap::new();
        // Raw signature from Bun Web Crypto for the synthetic key and token below.
        let signature = STANDARD.encode([
            0x63, 0x5c, 0x1a, 0x9f, 0xff, 0x2c, 0x0c, 0xfe, 0x3f, 0x52, 0x40, 0xc9, 0x39, 0x2e,
            0xf3, 0x9e, 0xee, 0x5e, 0x27, 0x3b, 0xa0, 0x09, 0x65, 0x47, 0x3d, 0xa4, 0x65, 0x04,
            0xe9, 0xa5, 0xe1, 0xb8,
        ]);
        let escaped: String = url::form_urlencoded::byte_serialize(signature.as_bytes()).collect();
        let cookie = format!("better-auth.session_token=fixture-token.{escaped}");
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("other=x; {cookie}")).unwrap(),
        );
        let secret = "synthetic-secret-for-session-fixture";
        assert_eq!(
            token(&headers, secret, false).as_deref(),
            Some("fixture-token")
        );
        assert_eq!(token(&headers, "wrong", false), None);
        assert_eq!(token(&headers, secret, true), None);
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("better-auth.session_token=bad; {cookie}")).unwrap(),
        );
        assert_eq!(token(&headers, secret, false), None);
    }
}
