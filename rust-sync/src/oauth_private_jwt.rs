use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use p521::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use ring::{digest, signature};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio_postgres::Client;
use url::Url;

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    exp: f64,
    iat: Option<f64>,
    nbf: Option<f64>,
    jti: String,
}

impl Claims {
    fn valid(&self, client_id: &str, issuer: &str, endpoint: &str, now: f64) -> bool {
        let accepted = |aud: &str| aud == issuer || aud == endpoint;
        let audience_matches = match &self.aud {
            Audience::One(aud) => accepted(aud),
            Audience::Many(auds) => auds.iter().any(|aud| accepted(aud)),
        };
        self.iss == client_id
            && self.sub == client_id
            && audience_matches
            && self.exp > now
            && self.exp - now <= 300.0
            && self.iat.is_none_or(|iat| now - iat <= 300.0)
            && self.nbf.is_none_or(|nbf| nbf <= now)
            && !self.jti.is_empty()
    }
}

fn public_key_matches(alg: &str, jwk: &Map<String, Value>) -> bool {
    if ["d", "p", "q", "dp", "dq", "qi", "oth", "k"]
        .iter()
        .any(|key| jwk.contains_key(*key))
        || jwk
            .get("alg")
            .is_some_and(|value| value.as_str() != Some(alg))
    {
        return false;
    }
    let kty = jwk.get("kty").and_then(Value::as_str);
    let curve = jwk.get("crv").and_then(Value::as_str);
    let has = |field| {
        jwk.get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    };
    match alg {
        "RS256" | "RS384" | "RS512" | "PS256" | "PS384" | "PS512" => {
            kty == Some("RSA") && has("n") && has("e")
        }
        "ES256" => kty == Some("EC") && curve == Some("P-256") && has("x") && has("y"),
        "ES384" => kty == Some("EC") && curve == Some("P-384") && has("x") && has("y"),
        "ES512" => kty == Some("EC") && curve == Some("P-521") && has("x") && has("y"),
        "EdDSA" => kty == Some("OKP") && curve == Some("Ed25519") && has("x"),
        _ => false,
    }
}

fn key_matches(alg: &str, jwk: &Value) -> bool {
    let Some(jwk) = jwk.as_object() else {
        return false;
    };
    public_key_matches(alg, jwk)
        && jwk.get("use").is_none_or(|value| value == "sig")
        && jwk.get("ext").is_none_or(Value::is_boolean)
        && jwk.get("key_ops").is_none_or(|ops| {
            ops.as_array().is_some_and(|ops| {
                ops.iter().any(|op| op == "verify")
                    && ops
                        .iter()
                        .enumerate()
                        .all(|(idx, op)| op.is_string() && !ops[..idx].contains(op))
            })
        })
}

pub(crate) fn verify_jws(alg: &str, jwk: &Value, input: &[u8], signature: &[u8]) -> bool {
    key_matches(alg, jwk) && verify_matching_key(alg, jwk, input, signature).is_some()
}

fn verify_matching_key(alg: &str, jwk: &Value, input: &[u8], proof: &[u8]) -> Option<()> {
    let bytes = |field: &str| URL_SAFE_NO_PAD.decode(jwk.get(field)?.as_str()?).ok();
    let rsa = match alg {
        "RS256" => Some(&signature::RSA_PKCS1_2048_8192_SHA256),
        "RS384" => Some(&signature::RSA_PKCS1_2048_8192_SHA384),
        "RS512" => Some(&signature::RSA_PKCS1_2048_8192_SHA512),
        "PS256" => Some(&signature::RSA_PSS_2048_8192_SHA256),
        "PS384" => Some(&signature::RSA_PSS_2048_8192_SHA384),
        "PS512" => Some(&signature::RSA_PSS_2048_8192_SHA512),
        _ => None,
    };
    if let Some(parameters) = rsa {
        return signature::RsaPublicKeyComponents {
            n: bytes("n")?,
            e: bytes("e")?,
        }
        .verify(parameters, input, proof)
        .ok();
    }
    if alg == "EdDSA" {
        return signature::UnparsedPublicKey::new(&signature::ED25519, bytes("x")?)
            .verify(input, proof)
            .ok();
    }
    let (algorithm, width) = match alg {
        "ES256" => (Some(&signature::ECDSA_P256_SHA256_FIXED), 32),
        "ES384" => (Some(&signature::ECDSA_P384_SHA384_FIXED), 48),
        "ES512" => (None, 66),
        _ => return None,
    };
    let (x, y) = (bytes("x")?, bytes("y")?);
    if x.len() != width || y.len() != width {
        return None;
    }
    let point = [vec![0x04], x, y].concat();
    if let Some(algorithm) = algorithm {
        signature::UnparsedPublicKey::new(algorithm, point)
            .verify(input, proof)
            .ok()
    } else {
        VerifyingKey::from_sec1_bytes(&point)
            .ok()?
            .verify(input, &Signature::try_from(proof).ok()?)
            .ok()
    }
}

fn assertion(token: &str) -> Option<(Header, Claims, &[u8], Vec<u8>)> {
    let (input, signature) = token.rsplit_once('.')?;
    let (header, claims) = input.split_once('.')?;
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    if header.get("crit").is_some()
        || header.get("b64").is_some_and(|b64| b64 != true)
        || header.get("kid").is_some_and(|kid| !kid.is_string())
    {
        return None;
    }
    Some((
        serde_json::from_value(header).ok()?,
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).ok()?).ok()?,
        input.as_bytes(),
        URL_SAFE_NO_PAD.decode(signature).ok()?,
    ))
}

pub(crate) fn replay_id(namespace: &str, jti: &str) -> String {
    let digest = digest::digest(&digest::SHA256, format!("{namespace}:{jti}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_ref()[..16]);
    bytes[6] = bytes[6] & 0x0f | 0x50;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

const JWKS_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct CachedKeys {
    fetched: Instant,
    keys: Arc<[Value]>,
}

static JWKS: LazyLock<Mutex<HashMap<Url, CachedKeys>>> = LazyLock::new(Mutex::default);

fn keys_from(value: Value) -> Option<Arc<[Value]>> {
    let set = value.as_object()?.get("keys")?.as_array()?;
    if set.is_empty()
        || !set.iter().all(|key| {
            key.as_object().is_some_and(|key| {
                [
                    "RS256", "RS384", "RS512", "PS256", "PS384", "PS512", "ES256", "ES384",
                    "ES512", "EdDSA",
                ]
                .iter()
                .any(|alg| public_key_matches(alg, key))
            })
        })
    {
        return None;
    }
    Some(Arc::from(set.clone()))
}

async fn remote_keys(uri: &Url, force: bool) -> Option<Arc<[Value]>> {
    let cached = JWKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(uri)
        .cloned();
    if !force
        && let Some(cached) = &cached
        && cached.fetched.elapsed() < JWKS_TTL
    {
        return Some(cached.keys.clone());
    }
    let Some(body) = crate::oauth_client::fetch_public(uri, 64 * 1024).await else {
        return cached
            .filter(|entry| !force && entry.fetched.elapsed() < JWKS_TTL * 2)
            .map(|entry| entry.keys);
    };
    let keys = keys_from(serde_json::from_slice(&body).ok()?)?;
    let mut cache = JWKS.lock().unwrap_or_else(|error| error.into_inner());
    if cache.len() >= 500
        && !cache.contains_key(uri)
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.fetched)
            .map(|(key, _)| key.clone())
    {
        cache.remove(&oldest);
    }
    cache.insert(
        uri.clone(),
        CachedKeys {
            fetched: Instant::now(),
            keys: keys.clone(),
        },
    );
    Some(keys)
}

fn find_key<'a>(header: &Header, keys: &'a [Value]) -> Result<Option<&'a Value>, ()> {
    let mut matches = keys.iter().filter(|key| {
        key_matches(&header.alg, key)
            && header
                .kid
                .as_deref()
                .is_none_or(|kid| key.get("kid").and_then(Value::as_str) == Some(kid))
    });
    let first = matches.next();
    if matches.next().is_some() {
        return Err(());
    }
    Ok(first)
}

async fn signed_by_registered_key(
    header: &Header,
    input: &[u8],
    signature: &[u8],
    inline: Option<&str>,
    remote: Option<&Url>,
) -> Option<()> {
    let mut keys = if let Some(inline) = inline {
        keys_from(serde_json::from_str(inline).ok()?)?
    } else {
        remote_keys(remote?, false).await?
    };
    if find_key(header, &keys).ok()?.is_none() {
        keys = remote_keys(remote?, true).await?;
    }
    let key = find_key(header, &keys).ok()??;
    verify_matching_key(&header.alg, key, input, signature)
}

pub(crate) async fn verify(
    db: &Client,
    token: &str,
    client_id: &str,
    issuer: &str,
    endpoint: &str,
    jwks: Option<&str>,
    jwks_uri: Option<&str>,
) -> Result<bool, tokio_postgres::Error> {
    let Some((header, claims, input, proof)) = assertion(token) else {
        return Ok(false);
    };
    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as f64
    };
    if !claims.valid(client_id, issuer, endpoint, now()) {
        return Ok(false);
    }
    let remote = jwks_uri.and_then(|uri| Url::parse(uri).ok()).filter(|uri| {
        [issuer, client_id]
            .into_iter()
            .any(|owner| Url::parse(owner).is_ok_and(|owner| owner.origin() == uri.origin()))
    });
    if signed_by_registered_key(&header, input, &proof, jwks, remote.as_ref())
        .await
        .is_none()
        || !claims.valid(client_id, issuer, endpoint, now())
    {
        return Ok(false);
    }
    let id = replay_id(&format!("private_key_jwt:{client_id}"), &claims.jti);
    Ok(db
        .execute(
            r#"INSERT INTO neon_auth."oauthClientAssertion" (id, "expiresAt")
           VALUES ($1::text::uuid, to_timestamp($2)) ON CONFLICT (id) DO NOTHING"#,
            &[&id, &claims.exp],
        )
        .await?
        == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use serde_json::json;

    #[test]
    fn verifies_asymmetric_key_policy_and_claims() {
        let key = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let public = URL_SAFE_NO_PAD.encode(key.public_key().as_ref());
        let jwk = json!({"kty":"OKP","crv":"Ed25519","x":public,"key_ops":["verify"]});
        let input = b"protected.payload";
        let signature = key.sign(input);
        assert!(verify_jws("EdDSA", &jwk, input, signature.as_ref()));
        assert!(!verify_jws("EdDSA", &jwk, b"tampered", signature.as_ref()));
        assert!(!verify_jws("HS256", &jwk, input, signature.as_ref()));
        let mut private = jwk;
        private["d"] = json!("private");
        assert!(!verify_jws("EdDSA", &private, input, signature.as_ref()));

        let mut claims: Claims = serde_json::from_value(json!({
            "iss":"legacy-client", "sub":"legacy-client", "aud":["https://test.invalid/api/auth"],
            "exp":390, "iat":100,"jti":"nonce"
        }))
        .unwrap();
        let valid = |claims: &Claims, now| {
            claims.valid(
                "legacy-client",
                "https://test.invalid/api/auth",
                "https://test.invalid/api/auth/oauth2/token",
                now,
            )
        };
        assert!(valid(&claims, 100.0));
        assert!(!valid(&claims, 80.0));
        assert!(!valid(&claims, 390.0));
        claims.nbf = Some(200.0);
        assert!(!valid(&claims, 100.0));
        claims.sub = "other-client".into();
        assert!(!valid(&claims, 200.0));
    }
}
