use crate::{
    oauth::{self, App, Error},
    oauth_dpop,
};
use axum::http::{HeaderMap, StatusCode};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, aead::Aead};
use ring::{
    digest, hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant, SystemTime},
};

use tokio_postgres::GenericClient;
use url::{Host, Url};

pub(crate) const SCOPES: &[&str] = &["mcp:read", "mcp:write", "offline_access"];
pub(crate) const GRANTS: &[&str] = &["authorization_code", "refresh_token"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Client {
    pub(crate) client_id: String,
    pub(crate) client_secret: Option<String>,
    pub(crate) client_discovery_id: Option<String>,
    pub(crate) disabled: Option<bool>,
    pub(crate) skip_consent: Option<bool>,
    pub(crate) application_type: Option<String>,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) token_endpoint_auth_method: Option<String>,
    pub(crate) grant_types: Option<Vec<String>>,
    pub(crate) scopes: Option<Vec<String>>,
    pub(crate) require_pkce: Option<bool>,
    pub(crate) dpop_bound_access_tokens: Option<bool>,
    pub(crate) metadata: Option<Value>,
    pub(crate) jwks: Option<String>,
    pub(crate) jwks_uri: Option<String>,
}

impl Client {
    pub(crate) fn allows_grant(&self, grant: &str) -> bool {
        let allows_code = self
            .grant_types
            .as_ref()
            .filter(|grants| !grants.is_empty())
            .is_none_or(|grants| grants.iter().any(|entry| entry == "authorization_code"));
        (allows_code && matches!(grant, "authorization_code" | "refresh_token"))
            || self
                .grant_types
                .as_ref()
                .is_some_and(|grants| grants.iter().any(|entry| entry == grant))
    }

    pub(crate) fn allows_scopes(&self, scopes: &[String]) -> bool {
        scopes.iter().all(|scope| {
            SCOPES.contains(&scope.as_str())
                && self
                    .scopes
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(scope))
        })
    }

    pub(crate) fn requires_dpop(&self) -> bool {
        self.dpop_bound_access_tokens == Some(true)
            || self
                .metadata
                .as_ref()
                .and_then(|value| value.get("dpop_bound_access_tokens"))
                == Some(&Value::Bool(true))
    }

    pub(crate) fn requires_pkce(&self, scopes: &[String]) -> bool {
        self.require_pkce != Some(false)
            || self.method() == "none"
            || scopes.iter().any(|scope| scope == "offline_access")
    }

    pub(crate) fn method(&self) -> &str {
        self.token_endpoint_auth_method
            .as_deref()
            .unwrap_or("client_secret_basic")
    }

    pub(crate) fn valid_redirect(&self, requested: &str) -> bool {
        let web = self.application_type.as_deref() == Some("web");
        let Some(url) = redirect_uri(requested, web) else {
            return false;
        };
        let without_port = (!web && is_exact_loopback_http(requested)).then(|| remove_port(url));
        self.redirect_uris.iter().any(|registered| {
            registered == requested
                || without_port.as_ref().is_some_and(|expected| {
                    is_exact_loopback_http(registered)
                        && Url::parse(registered).is_ok_and(|url| remove_port(url) == *expected)
                })
        })
    }
}

fn remove_port(mut url: Url) -> Url {
    let _ = url.set_port(None);
    url
}

fn is_exact_loopback_http(uri: &str) -> bool {
    let Some((scheme, tail)) = uri.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") {
        return false;
    }
    let authority = tail.split(['/', '?', '#']).next().unwrap_or_default();
    let host = if authority.starts_with('[') {
        authority
            .split_once(']')
            .map_or("", |(part, _)| &authority[..part.len() + 1])
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "[::1]"
    )
}

fn redirect_uri(input: &str, web: bool) -> Option<Url> {
    let url = Url::parse(input).ok()?;
    if input.contains('#') || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let can_redirect = match url.scheme() {
        "https" => match url.host()? {
            Host::Ipv4(ip) => !ip.is_loopback(),
            Host::Ipv6(ip) => {
                !ip.is_loopback() && !ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
            }
            Host::Domain(name) => !name.trim_end_matches('.').eq_ignore_ascii_case("localhost"),
        },
        "http" => !web && is_exact_loopback_http(input),
        "ws" | "wss" | "file" | "ftp" | "mailto" | "javascript" | "data" | "vbscript" | "blob"
        | "about" | "view-source" => false,
        _ => {
            !web && url.path().starts_with('/')
                && (url.has_host()
                    || !url.path().starts_with("//")
                        && url.scheme().contains('.')
                        && url.scheme().split('.').all(|part| {
                            part.as_bytes()
                                .first()
                                .is_some_and(u8::is_ascii_alphanumeric)
                                && part
                                    .as_bytes()
                                    .last()
                                    .is_some_and(u8::is_ascii_alphanumeric)
                                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                        }))
        }
    };
    can_redirect.then_some(url)
}

#[derive(Clone, Copy)]
enum Source<'a> {
    Dynamic(&'a str),
    Metadata { client_id: &'a str, origin: &'a str },
}

#[derive(Default, Deserialize)]
struct Metadata {
    redirect_uris: Option<Vec<String>>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
    token_endpoint_auth_method: Option<String>,
    application_type: Option<String>,
    scope: Option<String>,
    require_pkce: Option<bool>,
    dpop_bound_access_tokens: Option<bool>,
    jwks: Option<Value>,
    jwks_uri: Option<String>,
    resources: Option<Vec<String>>,
    subject_type: Option<String>,
    client_name: Option<String>,
    client_uri: Option<String>,
    logo_uri: Option<String>,
    contacts: Option<Vec<String>>,
    tos_uri: Option<String>,
    policy_uri: Option<String>,
    software_id: Option<String>,
    software_version: Option<String>,
    software_statement: Option<String>,
    post_logout_redirect_uris: Option<Vec<String>>,
}

fn valid_public_key(key: &Value) -> bool {
    let member = |name| {
        key.get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let allowed = match (member("kty"), member("crv")) {
        (Some("RSA"), _) if member("n").is_some() && member("e").is_some() => {
            &["RS256", "RS384", "RS512", "PS256", "PS384", "PS512"][..]
        }
        (Some("EC"), Some(curve)) if member("x").is_some() && member("y").is_some() => {
            match curve {
                "P-256" => &["ES256"][..],
                "P-384" => &["ES384"][..],
                "P-521" => &["ES512"][..],
                _ => return false,
            }
        }
        (Some("OKP"), Some("Ed25519")) if member("x").is_some() => &["EdDSA"][..],
        _ => return false,
    };
    oauth_dpop::thumbprint(key).is_some()
        && key
            .get("alg")
            .is_none_or(|alg| alg.as_str().is_some_and(|alg| allowed.contains(&alg)))
}

fn metadata_error() -> Error {
    Error::bad("invalid_client_metadata", "Invalid client metadata")
}

impl Metadata {
    fn parse(raw: Value, source: Source<'_>, resource: &str) -> Result<Self, Error> {
        let object = raw.as_object().ok_or_else(metadata_error)?;
        let forbidden = [
            "backchannel_logout_uri",
            "backchannel_logout_session_required",
            "client_secret",
            "client_secret_expires_at",
            "disabled",
            "skip_consent",
            "reference_id",
            "enable_end_session",
            "user_id",
        ];
        if forbidden.iter().any(|key| object.contains_key(*key))
            || (matches!(source, Source::Dynamic(_)) && object.contains_key("client_id"))
        {
            return Err(metadata_error());
        }
        if let Source::Metadata { client_id, .. } = source {
            let forbidden = [
                "client_id_issued_at",
                "require_pkce",
                "resources",
                "clientSecret",
                "clientDiscoveryId",
                "skipConsent",
                "enableEndSession",
                "requirePKCE",
                "referenceId",
                "userId",
                "clientId",
                "applicationType",
                "tokenEndpointAuthMethod",
                "redirectUris",
                "postLogoutRedirectUris",
                "grantTypes",
                "responseTypes",
                "scopes",
                "expiresAt",
                "createdAt",
                "updatedAt",
                "softwareId",
                "softwareVersion",
                "softwareStatement",
                "backchannelLogoutUri",
                "backchannelLogoutSessionRequired",
                "jwksUri",
                "dpopBoundAccessTokens",
                "subjectType",
            ];
            if object.get("client_id").and_then(Value::as_str) != Some(client_id)
                || forbidden.iter().any(|key| object.contains_key(*key))
            {
                return Err(metadata_error());
            }
        }
        let mut data: Self = serde_json::from_value(raw).map_err(|_| metadata_error())?;
        let default_method = if matches!(source, Source::Metadata { .. }) {
            "none"
        } else {
            "client_secret_basic"
        };
        let method = data
            .token_endpoint_auth_method
            .get_or_insert_with(|| default_method.to_owned());
        match method.as_str() {
            "none" | "private_key_jwt" => (),
            "client_secret_basic" | "client_secret_post"
                if matches!(source, Source::Dynamic(_)) => {}
            _ => return Err(metadata_error()),
        }
        if data.require_pkce == Some(false)
            || data.subject_type.as_deref().is_some_and(|s| s != "public")
        {
            return Err(metadata_error());
        }
        if data
            .resources
            .as_ref()
            .is_some_and(|resources| resources.iter().any(|r| r != resource))
        {
            return Err(Error::bad(
                "invalid_target",
                "Requested resource is not allowed",
            ));
        }
        if data.scope.as_deref().is_some_and(|scopes| {
            oauth::scopes(scopes)
                .iter()
                .any(|s| !SCOPES.contains(&s.as_str()))
        }) {
            return Err(Error::bad("invalid_scope", "Unsupported scope"));
        }
        let grant_types = data
            .grant_types
            .get_or_insert_with(|| vec!["authorization_code".to_owned()]);
        let requested_code = grant_types
            .iter()
            .any(|grant| grant == "authorization_code");
        grant_types.retain(|grant| GRANTS.contains(&grant.as_str()));
        if grant_types.is_empty() {
            return Err(metadata_error());
        }
        let redirects = data.redirect_uris.get_or_insert_with(Vec::new);
        let code = grant_types
            .iter()
            .any(|grant| grant == "authorization_code");
        if requested_code && !code {
            data.response_types = None;
        }
        if data.response_types.is_none() && code {
            data.response_types = Some(vec!["code".to_owned()]);
        }
        if data.response_types.as_ref().is_some_and(|response| {
            response.is_empty() || response.iter().any(|item| item != "code") || !code
        }) {
            return Err(metadata_error());
        }
        if matches!(source, Source::Dynamic(_)) {
            data.application_type.get_or_insert_with(|| {
                if redirects.iter().any(|uri| {
                    is_exact_loopback_http(uri)
                        || Url::parse(uri)
                            .is_ok_and(|url| !matches!(url.scheme(), "http" | "https"))
                }) {
                    "native".to_owned()
                } else {
                    "web".to_owned()
                }
            });
        }
        if data
            .application_type
            .as_deref()
            .is_some_and(|kind| !matches!(kind, "web" | "native"))
        {
            return Err(metadata_error());
        }
        let web = data.application_type.as_deref() == Some("web");
        if (code && redirects.is_empty())
            || redirects.iter().any(|uri| redirect_uri(uri, web).is_none())
            || data
                .post_logout_redirect_uris
                .as_ref()
                .is_some_and(|uris| uris.iter().any(|uri| redirect_uri(uri, false).is_none()))
        {
            return Err(Error::bad("invalid_redirect_uri", "Invalid redirect URI"));
        }
        if let Source::Metadata { client_id, .. } = source {
            let issuer = Url::parse(client_id)
                .map_err(|_| metadata_error())?
                .origin();
            let contact_urls = [
                &data.client_uri,
                &data.logo_uri,
                &data.tos_uri,
                &data.policy_uri,
            ];
            for value in contact_urls.into_iter().flatten() {
                let mut url = Url::parse(value).map_err(|_| metadata_error())?;
                if !matches!(url.scheme(), "https" | "http")
                    || (Some(value) == data.client_uri.as_ref() && url.origin() != issuer)
                {
                    return Err(metadata_error());
                }
                let _ = url.set_scheme("https");
                if public_host(&url).is_none() {
                    return Err(metadata_error());
                }
            }
            for value in data
                .post_logout_redirect_uris
                .as_deref()
                .unwrap_or_default()
            {
                let url = Url::parse(value).map_err(|_| metadata_error())?;
                let reverse_domain =
                    !url.has_host() && url.scheme().contains('.') && url.path().starts_with('/');
                if url.origin() != issuer && !is_exact_loopback_http(value) && !reverse_domain {
                    return Err(metadata_error());
                }
            }
        }
        if let Some(jwks) = &data.jwks {
            let keys = jwks
                .get("keys")
                .and_then(Value::as_array)
                .ok_or_else(metadata_error)?;
            if keys.is_empty()
                || keys.iter().any(|key| !valid_public_key(key))
                || data.jwks_uri.is_some()
            {
                return Err(metadata_error());
            }
        }
        if let Some(uri) = &data.jwks_uri {
            let url = Url::parse(uri).map_err(|_| metadata_error())?;
            let origin = match source {
                Source::Dynamic(origin) | Source::Metadata { origin, .. } => origin,
            };
            let origin_ok = url.origin().ascii_serialization() == origin
                || matches!(source, Source::Metadata { client_id, .. } if Url::parse(client_id).is_ok_and(|client_url| client_url.origin() == url.origin()));
            if !origin_ok || public_host(&url).is_none() {
                return Err(metadata_error());
            }
        }
        if method == "private_key_jwt" && data.jwks.is_none() && data.jwks_uri.is_none() {
            return Err(metadata_error());
        }
        Ok(data)
    }

    async fn store(
        &self,
        db: &impl GenericClient,
        id: &str,
        encrypted: Option<&str>,
        discovery: Option<&str>,
        resource: &str,
    ) -> Result<(), Error> {
        let available: bool = db.query_one(
            r#"SELECT EXISTS (SELECT 1 FROM neon_auth."oauthResource" WHERE identifier=$1 AND NOT COALESCE(disabled, false))"#,
            &[&resource],
        ).await?.get(0);
        if !available {
            return Err(Error::bad(
                "invalid_target",
                "Requested resource is unavailable",
            ));
        }
        db.execute(
            r#"INSERT INTO neon_auth."oauthClient" ("clientId", "clientSecret", "clientDiscoveryId",
             "redirectUris", "tokenEndpointAuthMethod", "applicationType", "grantTypes", "responseTypes",
             scopes, "requirePKCE", "dpopBoundAccessTokens", jwks, "jwksUri", name, uri, icon, contacts,
             tos, policy, "softwareId", "softwareVersion", "softwareStatement", "postLogoutRedirectUris",
             "subjectType", "clientCredentialsScopes", "createdAt", "updatedAt")
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                     $18, $19, $20, $21, $22, $23, $24, '[]', now(), now())
             ON CONFLICT ("clientId") DO UPDATE SET
               "redirectUris"=EXCLUDED."redirectUris", "tokenEndpointAuthMethod"=EXCLUDED."tokenEndpointAuthMethod",
               "applicationType"=EXCLUDED."applicationType", "grantTypes"=EXCLUDED."grantTypes",
               "responseTypes"=EXCLUDED."responseTypes", scopes=EXCLUDED.scopes, "requirePKCE"=EXCLUDED."requirePKCE",
               "dpopBoundAccessTokens"=EXCLUDED."dpopBoundAccessTokens", jwks=EXCLUDED.jwks, "jwksUri"=EXCLUDED."jwksUri",
               name=EXCLUDED.name, uri=EXCLUDED.uri, icon=EXCLUDED.icon, contacts=EXCLUDED.contacts,
               tos=EXCLUDED.tos, policy=EXCLUDED.policy, "softwareId"=EXCLUDED."softwareId",
               "softwareVersion"=EXCLUDED."softwareVersion", "softwareStatement"=EXCLUDED."softwareStatement",
               "postLogoutRedirectUris"=EXCLUDED."postLogoutRedirectUris", "subjectType"=EXCLUDED."subjectType",
               "updatedAt"=now()
             WHERE neon_auth."oauthClient"."clientDiscoveryId"='cimd' AND EXCLUDED."clientDiscoveryId"='cimd'"#,
            &[&id, &encrypted, &discovery, &json!(self.redirect_uris), &self.token_endpoint_auth_method,
                &self.application_type, &self.grant_types.as_ref().map(|value| json!(value)),
                &self.response_types.as_ref().map(|value| json!(value)), &json!(SCOPES), &self.require_pkce,
                &self.dpop_bound_access_tokens, &self.jwks.as_ref().map(Value::to_string), &self.jwks_uri,
                &self.client_name, &self.client_uri, &self.logo_uri, &self.contacts.as_ref().map(|v| json!(v)),
                &self.tos_uri, &self.policy_uri, &self.software_id, &self.software_version, &self.software_statement,
                &self.post_logout_redirect_uris.as_ref().map(|v| json!(v)), &self.subject_type],
        ).await?;
        db.execute(
            r#"INSERT INTO neon_auth."oauthClientResource" ("clientId", "resourceId", "createdAt")
               SELECT $1, $2, now() WHERE EXISTS (SELECT 1 FROM neon_auth."oauthResource"
                 WHERE identifier=$2 AND NOT COALESCE(disabled,false))
               ON CONFLICT ("clientId", "resourceId") DO NOTHING"#,
            &[&id, &resource],
        )
        .await?;
        Ok(())
    }
}

async fn stored(db: &impl GenericClient, client_id: &str) -> Result<Option<Client>, Error> {
    db.query_opt(
        r#"SELECT to_jsonb(c) FROM neon_auth."oauthClient" c WHERE c."clientId"=$1"#,
        &[&client_id],
    )
    .await?
    .map(|row| serde_json::from_value(row.get(0)).map_err(|_| Error::Internal))
    .transpose()
}

#[derive(Clone)]
struct CachedMetadata {
    expires: Instant,
    headers: HeaderMap,
}

static METADATA: LazyLock<Mutex<HashMap<(String, String), CachedMetadata>>> =
    LazyLock::new(Mutex::default);

fn cache_metadata(key: (String, String), headers: HeaderMap) {
    // Keep only freshness fields and validators from the remote response.
    let mut headers = headers;
    let keep = [
        "age",
        "cache-control",
        "date",
        "etag",
        "expires",
        "last-modified",
        "vary",
    ];
    let names: Vec<_> = headers
        .keys()
        .filter(|name| !keep.contains(&name.as_str()))
        .cloned()
        .collect();
    for name in names {
        headers.remove(name);
    }
    let mut cache = METADATA.lock().unwrap_or_else(|err| err.into_inner());
    let Some(lifetime) = metadata_lifetime(&headers, SystemTime::now()) else {
        cache.remove(&key);
        return;
    };
    if cache.len() >= 1000
        && !cache.contains_key(&key)
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, value)| value.expires)
            .map(|(k, _)| k.clone())
    {
        cache.remove(&oldest);
    }
    cache.insert(
        key,
        CachedMetadata {
            expires: Instant::now() + lifetime,
            headers,
        },
    );
}

fn metadata_lifetime(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    let controls: Vec<_> = headers
        .get_all("cache-control")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|v| v.trim().to_ascii_lowercase())
        .collect();
    let has = |directive: &str| {
        controls
            .iter()
            .any(|v| v.split('=').next() == Some(directive))
    };
    let mut shared = controls
        .iter()
        .filter_map(|value| value.strip_prefix("s-maxage="));
    let mut max_age = controls
        .iter()
        .filter_map(|value| value.strip_prefix("max-age="));
    if has("no-store")
        || has("private")
        || headers
            .get_all("vary")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.split(',').any(|v| v.trim() == "*"))
    {
        return None;
    }
    if has("no-cache") {
        return Some(Duration::ZERO);
    }
    let maximum = if has("s-maxage") {
        shared.next().filter(|_| shared.next().is_none())
    } else {
        max_age.next().filter(|_| max_age.next().is_none())
    };
    let date = |name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| httpdate::parse_http_date(value).ok())
    };
    let response_date = date("date").unwrap_or(now);
    let age = headers
        .get("age")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_default();
    let current_age = age.max(now.duration_since(response_date).unwrap_or_default());
    let lifetime = if has("s-maxage") || has("max-age") {
        Some(
            maximum
                .and_then(|v| v.trim().trim_matches('"').parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or_default(),
        )
    } else if headers.contains_key("expires") {
        Some(
            date("expires")
                .and_then(|expires| expires.duration_since(response_date).ok())
                .unwrap_or_default(),
        )
    } else {
        None
    };
    let limit = Duration::from_secs(3600);
    Some(lifetime.map_or(limit, |lifetime| {
        lifetime.saturating_sub(current_age).min(limit)
    }))
}

pub(crate) async fn lookup(
    db: &impl GenericClient,
    origin: &str,
    resource: &str,
    id: &str,
) -> Result<Option<Client>, Error> {
    if id.len() > 4096 {
        return Ok(None);
    }
    let prior = stored(db, id).await?;
    if let Some(client) = &prior {
        if client.client_discovery_id.is_none() || client.disabled == Some(true) {
            return Ok(prior);
        }
        if client.client_discovery_id.as_deref() != Some("cimd") {
            return Ok(None);
        }
    }
    let url = match Url::parse(id) {
        Ok(url) if valid_metadata_url(id, &url) => url,
        _ => return Ok(None),
    };
    let key = (origin.to_owned(), id.to_owned());
    let cached = prior.as_ref().and_then(|_| {
        METADATA
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(&key)
            .cloned()
    });
    if cached
        .as_ref()
        .is_some_and(|entry| entry.expires > Instant::now())
    {
        return Ok(prior);
    }
    let Some(result) = fetch_json(&url, 5 * 1024, cached.as_ref()).await else {
        return Ok(None);
    };
    if let Some(raw) = result.bytes {
        let Ok(raw) = serde_json::from_slice(&raw) else {
            return Ok(None);
        };
        let Ok(metadata) = Metadata::parse(
            raw,
            Source::Metadata {
                client_id: id,
                origin,
            },
            resource,
        ) else {
            return Ok(None);
        };
        metadata.store(db, id, None, Some("cimd"), resource).await?;
    }
    cache_metadata(key, result.headers);
    Ok(if prior.is_some() && result.not_modified {
        prior
    } else {
        stored(db, id).await?
    })
}
fn valid_metadata_url(raw: &str, url: &Url) -> bool {
    raw.get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        && raw
            .get(8..)
            .and_then(|suffix| suffix.split_once('/'))
            .is_some_and(|(authority, tail)| {
                !authority.is_empty()
                    && !authority.contains(['?', '#', '@'])
                    && !tail
                        .split('?')
                        .next()
                        .unwrap_or_default()
                        .split('/')
                        .any(|segment| {
                            matches!(
                                segment.to_ascii_lowercase().replace("%2e", ".").as_str(),
                                "." | ".."
                            )
                        })
            })
        && !raw.contains(['\\', '#'])
        && public_host(url).is_some()
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !matches!(
                (a, b, c),
                (0 | 10 | 127 | 224..=255, _, _)
                    | (100, 64..=127, _)
                    | (169, 254, _)
                    | (172, 16..=31, _)
                    | (192, 168, _)
                    | (192, 0, 0 | 2)
                    | (192, 88, 99)
                    | (198, 18 | 19, _)
                    | (198, 51, 100)
                    | (203, 0, 113)
            )
        }
        IpAddr::V6(ip) => {
            let [a, b, ..] = ip.segments();
            (0x2000..0x4000).contains(&a)
                && !matches!((a, b), (0x2001, 0..=0x01ff | 0x0db8) | (0x2002 | 0x3fff, _))
        }
    }
}

fn public_host(url: &Url) -> Option<Host<&str>> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host()?;
    match host {
        Host::Ipv4(ip) if public_ip(ip.into()) => (),
        Host::Ipv6(ip) if public_ip(ip.into()) => (),
        Host::Domain(name) => {
            let name = name.trim_end_matches('.');
            if !name.contains('.')
                || [
                    "local",
                    "localhost",
                    "internal",
                    "home",
                    "onion",
                    "test",
                    "invalid",
                    "example",
                ]
                .iter()
                .any(|suffix| name.rsplit('.').next() == Some(suffix))
            {
                return None;
            }
        }
        _ => return None,
    }
    Some(host)
}

#[derive(Default)]
struct FetchBudget {
    requests: VecDeque<(Instant, String)>,
    active: HashMap<String, usize>,
}

static FETCHES: LazyLock<Mutex<FetchBudget>> = LazyLock::new(Mutex::default);

struct Permit(String);

impl Permit {
    fn take(host: &str) -> Option<Self> {
        let mut budget = FETCHES.lock().unwrap_or_else(|err| err.into_inner());
        let now = Instant::now();
        budget
            .requests
            .retain(|(time, _)| now.duration_since(*time) < Duration::from_secs(60));
        if budget.requests.len() >= 120
            || budget
                .requests
                .iter()
                .filter(|(_, name)| name == host)
                .count()
                >= 30
            || budget.active.values().sum::<usize>() >= 16
            || budget.active.get(host).copied().unwrap_or_default() >= 4
        {
            return None;
        }
        budget.requests.push_back((now, host.to_owned()));
        *budget.active.entry(host.to_owned()).or_default() += 1;
        Some(Self(host.to_owned()))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut budget = FETCHES.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(count) = budget.active.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                budget.active.remove(&self.0);
            }
        }
    }
}

struct Document {
    bytes: Option<Vec<u8>>,
    headers: HeaderMap,
    not_modified: bool,
}

pub(crate) async fn fetch_public(url: &Url, max_bytes: usize) -> Option<Vec<u8>> {
    fetch_json(url, max_bytes, None).await?.bytes
}

async fn fetch_json(
    url: &Url,
    max_bytes: usize,
    cached: Option<&CachedMetadata>,
) -> Option<Document> {
    let host = public_host(url)?;
    let hostname = url.host_str()?;
    let _permit = Permit::take(hostname.trim_end_matches('.'))?;
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none());
        if let Host::Domain(name) = host {
            let port = url.port_or_known_default()?;
            let addresses: Vec<_> = tokio::net::lookup_host((name, port)).await.ok()?.collect();
            if addresses.iter().any(|address| !public_ip(address.ip())) {
                return None;
            }
            builder = builder.resolve(name, *addresses.first()?);
        }
        let mut request = builder
            .build()
            .ok()?
            .get(url.clone())
            .header("accept", "application/json");
        let mut conditional = false;
        if let Some(cached) = cached {
            for (source, target) in [
                ("etag", "if-none-match"),
                ("last-modified", "if-modified-since"),
            ] {
                if let Some(value) = cached.headers.get(source) {
                    request = request.header(target, value.clone());
                    conditional = true;
                }
            }
        }
        let mut result = request.send().await.ok()?;
        if result.status() == reqwest::StatusCode::NOT_MODIFIED && conditional {
            let mut headers = cached?.headers.clone();
            for key in result.headers().keys() {
                headers.remove(key);
                for value in result.headers().get_all(key) {
                    headers.append(key, value.clone());
                }
            }
            return Some(Document {
                bytes: None,
                headers,
                not_modified: true,
            });
        }
        let mime = result
            .headers()
            .get("content-type")?
            .to_str()
            .ok()?
            .split(';')
            .next()?
            .trim()
            .to_ascii_lowercase();
        if result.status() != reqwest::StatusCode::OK
            || !mime
                .strip_prefix("application/")
                .is_some_and(|s| s == "json" || s.ends_with("+json"))
            || result
                .content_length()
                .is_some_and(|size| size > max_bytes as u64)
        {
            return None;
        }
        let headers = result.headers().clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = result.chunk().await.ok()? {
            if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        Some(Document {
            bytes: Some(bytes),
            headers,
            not_modified: false,
        })
    })
    .await
    .ok()?
}
fn secret_key(secret: &str) -> Result<XChaCha20Poly1305, Error> {
    XChaCha20Poly1305::new_from_slice(digest::digest(&digest::SHA256, secret.as_bytes()).as_ref())
        .map_err(|_| Error::Internal)
}

fn encrypt_secret(secret: &str, plaintext: &str) -> Result<String, Error> {
    let mut nonce = [0; 24];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| Error::Internal)?;
    let ciphertext = secret_key(secret)?
        .encrypt((&nonce).into(), plaintext.as_bytes())
        .map_err(|_| Error::Internal)?;
    Ok(oauth::hex_bytes(nonce.into_iter().chain(ciphertext)))
}

pub(crate) fn verify_secret(secret: &str, encrypted: &str, plaintext: &str) -> bool {
    let Some(bytes) = encrypted
        .len()
        .is_multiple_of(2)
        .then(|| {
            encrypted
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    Some(
                        ((pair[0] as char).to_digit(16)? as u8) << 4
                            | (pair[1] as char).to_digit(16)? as u8,
                    )
                })
                .collect::<Option<Vec<_>>>()
        })
        .flatten()
    else {
        return false;
    };
    let Some((nonce, ciphertext)) = bytes.split_at_checked(24) else {
        return false;
    };
    secret_key(secret)
        .and_then(|cipher| {
            cipher
                .decrypt(nonce.into(), ciphertext)
                .map_err(|_| Error::Internal)
        })
        .is_ok_and(|decrypted| {
            let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
            let tag = hmac::sign(&key, &decrypted);
            hmac::verify(&key, plaintext.as_bytes(), tag.as_ref()).is_ok()
        })
}

pub(crate) async fn register(
    db: &mut deadpool_postgres::Client,
    app: &App,
    headers: &HeaderMap,
    raw: Value,
) -> Result<Value, Error> {
    if let Some(header) = oauth_dpop::one_header(headers, "authorization", 4096)? {
        let status = if oauth_dpop::authorization(header)
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::BAD_REQUEST
        };
        return Err(Error::Protocol(
            status,
            "invalid_token",
            "Invalid initial access token",
        ));
    }
    let data = Metadata::parse(raw, Source::Dynamic(&app.origin), &app.resource)?;
    let id = oauth::random_token()?;
    let client_secret = if data
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|s| s.starts_with("client_secret_"))
    {
        Some(oauth::random_token()?)
    } else {
        None
    };
    let encrypted = client_secret
        .as_deref()
        .map(|secret| encrypt_secret(&app.secret, secret))
        .transpose()?;
    let tx = db.transaction().await?;
    data.store(&*tx, &id, encrypted.as_deref(), None, &app.resource)
        .await?;
    tx.commit().await?;
    let mut value = json!({
        "client_id": id, "client_id_issued_at": oauth::now_seconds(),
        "client_name":data.client_name, "client_uri":data.client_uri, "logo_uri":data.logo_uri,
        "contacts":data.contacts, "tos_uri":data.tos_uri, "policy_uri":data.policy_uri,
        "software_id":data.software_id,"software_version":data.software_version,
        "software_statement":data.software_statement,
        "redirect_uris":data.redirect_uris,"post_logout_redirect_uris":data.post_logout_redirect_uris,
        "grant_types":data.grant_types,"response_types":data.response_types,"application_type":data.application_type,
        "token_endpoint_auth_method":data.token_endpoint_auth_method,"scope":SCOPES.join(" "),
        "subject_type":data.subject_type, "jwks":data.jwks, "jwks_uri":data.jwks_uri,
        "require_pkce":data.require_pkce,"dpop_bound_access_tokens":data.dpop_bound_access_tokens,
    });
    if let Some(secret) = client_secret {
        value["client_secret"] = json!(secret);
        value["client_secret_expires_at"] = json!(0);
    }
    value
        .as_object_mut()
        .expect("registration response is an object")
        .retain(|_, item| !item.is_null());
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_transport_rejects_non_public_addresses_and_malformed_urls() {
        for host in [
            "localhost",
            "127.0.0.1",
            "127.1",
            "0x7f000001",
            "100.100.100.200",
            "169.254.169.254",
            "[::1]",
            "[::ffff:8.8.8.8]",
            "[2002:0101:0101::]",
            "metadata.google.internal",
        ] {
            let input = format!("https://{host}/metadata");
            assert!(
                !valid_metadata_url(&input, &Url::parse(&input).unwrap()),
                "{host}"
            );
        }
        for input in [
            "https://example.com",
            "https://example.com?path=/metadata",
            "https://example.com/a/%2e%2e/metadata",
            "https://example.com/metadata#",
            "https://user@example.com/metadata",
        ] {
            assert!(
                !valid_metadata_url(input, &Url::parse(input).unwrap()),
                "{input}"
            );
        }
        assert!(valid_metadata_url(
            "https://example.com/metadata",
            &Url::parse("https://example.com/metadata").unwrap()
        ));
    }

    #[test]
    fn metadata_document_owns_its_id_and_jwks_origin_but_allows_registered_native_callbacks() {
        let origin = "https://wartaq.example.com";
        let id = "https://client.example.com/mcp.json";
        let resource = "https://wartaq.example.com/mcp";
        let source = Source::Metadata {
            client_id: id,
            origin,
        };
        let doc = json!({
            "client_id":id,
            "client_name":"Synthetic client",
            "redirect_uris":["cursor://client/authorize", "http://127.0.0.1:9898/callback"],
            "token_endpoint_auth_method":"private_key_jwt",
            "jwks_uri":"https://client.example.com/jwks.json",
        });
        assert!(Metadata::parse(doc.clone(), source, resource).is_ok());
        for (key, value) in [
            ("client_id", json!("https://peer.example.com/mcp.json")),
            ("client_uri", json!("https://peer.example.com/")),
            ("jwks_uri", json!("https://peer.example.com/jwks.json")),
            ("require_pkce", json!(true)),
            ("token_endpoint_auth_method", json!("client_secret_post")),
        ] {
            let mut changed = doc.clone();
            changed[key] = value;
            assert!(Metadata::parse(changed, source, resource).is_err(), "{key}");
        }
    }

    #[test]
    fn metadata_honors_age_and_disables_cache_for_no_store() {
        let key = (
            "https://wartaq.example.com".to_owned(),
            "https://cache.example.com/metadata".to_owned(),
        );
        let mut headers = HeaderMap::new();
        headers.insert("cache-control", "public, max-age=60".parse().unwrap());
        headers.insert("age", "50".parse().unwrap());
        cache_metadata(key.clone(), headers);
        let expiry = METADATA.lock().unwrap().get(&key).unwrap().expires;
        assert!(expiry > Instant::now() && expiry <= Instant::now() + Duration::from_secs(10));
        let mut headers = HeaderMap::new();
        headers.insert("cache-control", "no-store".parse().unwrap());
        cache_metadata(key.clone(), headers);
        assert!(!METADATA.lock().unwrap().contains_key(&key));
    }

    #[test]
    fn metadata_uses_the_greater_of_apparent_age_and_age_header() {
        let date = "Sun, 06 Nov 1994 08:49:37 GMT";
        let now = httpdate::parse_http_date(date).unwrap() + Duration::from_secs(45);
        let mut headers = HeaderMap::new();
        headers.insert("date", date.parse().unwrap());
        headers.insert("cache-control", "public, max-age=60".parse().unwrap());
        for (age, remaining) in [("5", 15), ("50", 10), ("60", 0)] {
            headers.insert("age", age.parse().unwrap());
            assert_eq!(
                metadata_lifetime(&headers, now),
                Some(Duration::from_secs(remaining))
            );
        }
    }

    #[test]
    fn metadata_honors_expires_and_gives_cache_control_precedence() {
        let date = "Sun, 06 Nov 1994 08:49:37 GMT";
        let now = httpdate::parse_http_date(date).unwrap() + Duration::from_secs(30);
        let mut headers = HeaderMap::new();
        headers.insert("date", date.parse().unwrap());
        headers.insert("expires", "Sun, 06 Nov 1994 08:50:37 GMT".parse().unwrap());
        assert_eq!(
            metadata_lifetime(&headers, now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            metadata_lifetime(&headers, now + Duration::from_secs(31)),
            Some(Duration::ZERO)
        );
        headers.insert("expires", "0".parse().unwrap());
        assert_eq!(metadata_lifetime(&headers, now), Some(Duration::ZERO));
        headers.insert("cache-control", "max-age=40, s-maxage=50".parse().unwrap());
        assert_eq!(
            metadata_lifetime(&headers, now),
            Some(Duration::from_secs(20))
        );
    }

    #[test]
    fn registered_secrets_roundtrip_and_fail_closed() {
        let key = "synthetic test signing secret";
        let encrypted = encrypt_secret(key, "synthetic client secret").unwrap();
        assert!(verify_secret(key, &encrypted, "synthetic client secret"));
        assert!(!verify_secret(
            "another test signing secret",
            &encrypted,
            "synthetic client secret"
        ));
        assert!(!verify_secret(key, &encrypted, "another client secret"));
    }
}
