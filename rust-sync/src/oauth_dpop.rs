use crate::{oauth, oauth_private_jwt};
use axum::http::HeaderMap;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use tokio_postgres::GenericClient;
use url::Url;

pub(crate) fn one_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
    max: usize,
) -> Result<Option<&'a str>, oauth::Error> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    let valid = value
        .to_str()
        .ok()
        .filter(|value| !value.is_empty() && value.len() <= max);
    if values.next().is_some() || valid.is_none() {
        return Err(oauth::Error::bad(
            "invalid_request",
            "Invalid request header",
        ));
    }
    Ok(valid)
}

pub(crate) fn authorization(value: &str) -> Option<(&str, &str)> {
    let (scheme, raw) = value.trim().split_once(char::is_whitespace)?;
    let token = raw.trim();
    (!token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b)))
    .then_some((scheme, token))
}

pub(crate) fn jkt(confirmation: &Value) -> Option<&str> {
    confirmation
        .get("jkt")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

pub(crate) fn thumbprint(jwk: &Value) -> Option<String> {
    if ["d", "p", "q", "dp", "dq", "qi", "oth", "k"]
        .iter()
        .any(|key| jwk.get(key).is_some())
    {
        return None;
    }
    let kind = jwk.get("kty")?.as_str()?;
    let fields: &[&str] = match kind {
        "EC" => &["crv", "kty", "x", "y"],
        "OKP" => &["crv", "kty", "x"],
        "RSA" => &["e", "kty", "n"],
        _ => return None,
    };
    let mut key = serde_json::Map::new();
    for field in fields {
        key.insert((*field).to_owned(), json!(jwk.get(field)?.as_str()?));
    }
    Some(oauth::hash(Value::Object(key).to_string().as_bytes()))
}

pub(crate) struct Proof {
    pub(crate) jkt: String,
    key: String,
    expires: f64,
}

fn target(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    if url.fragment().is_some_and(|s| !s.is_empty()) || !matches!(url.scheme(), "https" | "http") {
        return None;
    }
    Some(format!(
        "{}{}",
        url.origin().ascii_serialization(),
        url.path()
    ))
}

pub(crate) fn verify(
    raw: &str,
    method: &str,
    url: &str,
    expected_jkt: Option<&str>,
    access_token: Option<&str>,
) -> Option<Proof> {
    let (signed, signature) = raw.rsplit_once('.')?;
    let (header, body) = signed.split_once('.')?;
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    let body: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()?;
    let algorithm = header.get("alg")?.as_str()?;
    if header.get("typ")?.as_str()? != "dpop+jwt"
        || !matches!(algorithm, "EdDSA" | "ES256" | "ES512" | "PS256" | "RS256")
        || header.get("crit").is_some()
    {
        return None;
    }
    let jwk = header.get("jwk")?;
    let jkt = thumbprint(jwk)?;
    let now = oauth::now_seconds() as f64;
    let iat = body.get("iat")?.as_f64()?;
    let jti = body.get("jti")?.as_str()?;
    let method = method.to_ascii_uppercase();
    let htu = target(url)?;
    if jti.is_empty()
        || jti.encode_utf16().count() > 512
        || !body.get("htm")?.as_str()?.eq_ignore_ascii_case(&method)
        || target(body.get("htu")?.as_str()?)? != htu
        || iat > now + 5.0
        || now - iat > 300.0
        || body
            .get("exp")
            .is_some_and(|v| v.as_f64().is_none_or(|exp| exp <= now))
        || body
            .get("nbf")
            .is_some_and(|v| v.as_f64().is_none_or(|nbf| nbf > now))
        || expected_jkt.is_some_and(|key| key != jkt)
        || access_token.is_some_and(|token| {
            let expected = oauth::hash(token.as_bytes());
            body.get("ath").and_then(Value::as_str) != Some(&expected)
        })
        || !oauth_private_jwt::verify_jws(
            algorithm,
            jwk,
            signed.as_bytes(),
            &URL_SAFE_NO_PAD.decode(signature).ok()?,
        )
    {
        return None;
    }
    Some(Proof {
        key: oauth::hash(format!("{jkt}\n{method}\n{htu}\n{jti}").as_bytes()),
        jkt,
        expires: iat + 300.0,
    })
}

impl Proof {
    pub(crate) async fn reserve(
        &self,
        db: &impl GenericClient,
    ) -> Result<bool, tokio_postgres::Error> {
        let identifier = format!("dpop-proof:{}", self.key);
        let id = oauth_private_jwt::replay_id("reserve", &identifier);
        Ok(db
            .execute(
                "INSERT INTO neon_auth.verification (id, identifier, value, \"expiresAt\")
             VALUES ($1::text::uuid, $2, $3, to_timestamp($4)) ON CONFLICT (id) DO NOTHING",
                &[&id, &identifier, &self.key, &self.expires],
            )
            .await?
            == 1)
    }
}
