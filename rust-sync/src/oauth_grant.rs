use crate::{
    oauth::{self, App, Code, Error, Resources},
    oauth_client::{self, Client},
    oauth_dpop, oauth_private_jwt,
};
use axum::{
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_postgres::{GenericClient, Row};
use url::form_urlencoded;

pub(crate) struct Grant {
    pub(crate) user_id: String,
    pub(crate) scopes: Vec<String>,
    hash: String,
    resource: String,
}

impl Grant {
    pub(crate) async fn still_active(
        &self,
        db: &impl GenericClient,
    ) -> Result<bool, tokio_postgres::Error> {
        Ok(access_row(db, &self.hash, Some(&self.resource))
            .await?
            .is_some_and(|row| {
                row.get::<_, String>("user") == self.user_id
                    && serde_json::from_value::<Vec<String>>(row.get("scopes"))
                        .is_ok_and(|scopes| scopes == self.scopes)
            }))
    }
}

async fn access_row(
    db: &impl GenericClient,
    hash: &str,
    required_resource: Option<&str>,
) -> Result<Option<Row>, tokio_postgres::Error> {
    db.query_opt(
        r#"SELECT s."userId"::text AS "user", t."sessionId"::text AS session, t."clientId" AS client,
                  t.scopes, t.resources, t.confirmation, c."clientDiscoveryId" AS discovery, floor(extract(epoch FROM t."expiresAt"))::bigint AS expires,
                  floor(extract(epoch FROM t."createdAt"))::bigint AS issued
           FROM neon_auth."oauthAccessToken" t
           JOIN neon_auth."oauthClient" c ON c."clientId"=t."clientId"
           JOIN neon_auth.session s ON s.id=t."sessionId" AND s."userId"=t."userId"
           WHERE t.token=$1 AND t.revoked IS NULL AND t."expiresAt">statement_timestamp()
             AND s."expiresAt">statement_timestamp() AND NOT COALESCE(c.disabled,false)
             AND ($2::text IS NULL OR t.resources ? $2)
             AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements_text(COALESCE(t.resources,'[]'::jsonb)) r(id)
               WHERE NOT EXISTS (SELECT 1 FROM neon_auth."oauthResource" p WHERE p.identifier=r.id))"#,
        &[&hash, &required_resource],
    ).await
}

pub(crate) async fn validate_resource(
    db: &tokio_postgres::Client,
    origin: &str,
    headers: &HeaderMap,
    method: &Method,
) -> Result<Option<Grant>, tokio_postgres::Error> {
    let Ok(Some(value)) = oauth_dpop::one_header(headers, "authorization", 4096) else {
        return Ok(None);
    };
    let Some((scheme, token)) = oauth_dpop::authorization(value) else {
        return Ok(None);
    };
    let dpop = scheme.eq_ignore_ascii_case("DPoP");
    if !dpop && !scheme.eq_ignore_ascii_case("Bearer") {
        return Ok(None);
    }
    let resource = format!("{origin}/mcp");
    let hash = oauth::hash(token.as_bytes());
    let Some(row) = access_row(db, &hash, Some(&resource)).await? else {
        return Ok(None);
    };
    if !access_client_active(db, origin, &resource, &row).await {
        return Ok(None);
    }
    let value: Option<Value> = row.get("confirmation");
    let jkt = value.as_ref().and_then(oauth_dpop::jkt);
    if jkt.is_some() != dpop {
        return Ok(None);
    }
    if let Some(jkt) = jkt {
        let Ok(Some(raw)) = oauth_dpop::one_header(headers, "dpop", 8192) else {
            return Ok(None);
        };
        let Some(proof) =
            oauth_dpop::verify(raw, method.as_str(), &resource, Some(jkt), Some(token))
        else {
            return Ok(None);
        };
        if !proof.reserve(db).await? {
            return Ok(None);
        }
    }
    let Ok(scopes) = serde_json::from_value(row.get("scopes")) else {
        return Ok(None);
    };
    Ok(Some(Grant {
        user_id: row.get("user"),
        scopes,
        hash,
        resource,
    }))
}

async fn access_client_active(
    db: &tokio_postgres::Client,
    origin: &str,
    resource: &str,
    row: &Row,
) -> bool {
    let discovery: Option<String> = row.get("discovery");
    if discovery.is_none() {
        return true;
    }
    let client_id: String = row.get("client");
    oauth_client::lookup(db, origin, resource, &client_id)
        .await
        .is_ok_and(|client| client.is_some_and(|c| c.disabled != Some(true)))
}

pub(crate) struct Policy {
    scopes: Vec<String>,
    access_ttl: i32,
    refresh_ttl: i32,
    dpop: bool,
}

pub(crate) async fn policy(
    db: &impl GenericClient,
    app: &App,
    client_id: &str,
    resources: &[String],
    scopes: &[String],
) -> Result<Policy, Error> {
    let invalid = || {
        Error::bad(
            "invalid_target",
            "Requested resource is unavailable or unauthorized",
        )
    };
    let mut result = Policy {
        scopes: scopes.to_vec(),
        access_ttl: 3600,
        refresh_ttl: 2_592_000,
        dpop: false,
    };
    if resources.len() > 16 {
        return Err(invalid());
    }
    for resource in resources {
        if resource == &format!("{}/oauth2/userinfo", app.issuer) {
            // The MCP-only deployment never issues OpenID or UserInfo grants.
            return Err(invalid());
        }
        let row = db
            .query_opt(
                r#"SELECT p."allowedScopes", p."accessTokenTtl", p."refreshTokenTtl",
                      COALESCE(p."dpopBoundAccessTokensRequired", false)
                 FROM neon_auth."oauthResource" p JOIN neon_auth."oauthClientResource" cr
                   ON cr."resourceId"=p.identifier AND cr."clientId"=$2
                 WHERE p.identifier=$1 AND NOT COALESCE(p.disabled,false)"#,
                &[&resource, &client_id],
            )
            .await?
            .ok_or_else(invalid)?;
        if let Some(allowed) = row.get::<_, Option<Value>>(0) {
            let allowed: Vec<String> =
                serde_json::from_value(allowed).map_err(|_| Error::Internal)?;
            result.scopes.retain(|scope| allowed.contains(scope));
            if result.scopes.is_empty() {
                return Err(Error::bad(
                    "invalid_scope",
                    "No scopes are available for the requested resource",
                ));
            }
        }
        if let Some(ttl) = row.get::<_, Option<i32>>(1) {
            result.access_ttl = result.access_ttl.min(ttl);
        }
        if let Some(ttl) = row.get::<_, Option<i32>>(2) {
            result.refresh_ttl = result.refresh_ttl.min(ttl);
        }
        result.dpop |= row.get::<_, bool>(3);
    }
    Ok(result)
}

#[derive(Deserialize)]
struct MachineRequest {
    client_id: Option<String>,
    client_secret: Option<String>,
    client_assertion: Option<String>,
    client_assertion_type: Option<String>,
    grant_type: Option<String>,
    code: Option<String>,
    code_verifier: Option<String>,
    redirect_uri: Option<String>,
    refresh_token: Option<String>,
    token: Option<String>,
    token_type_hint: Option<String>,
    resource: Option<Resources>,
    scope: Option<String>,
}

const ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";

enum Presented<'a> {
    Public(&'a str),
    Basic(String, String),
    Post(&'a str, &'a str),
    Assertion(Option<&'a str>, &'a str),
}

impl Presented<'_> {
    fn id(&self) -> Option<&str> {
        match self {
            Self::Public(id) | Self::Post(id, _) => Some(id),
            Self::Basic(id, _) => Some(id),
            Self::Assertion(id, _) => *id,
        }
    }
}

fn invalid_client() -> Error {
    Error::Protocol(
        StatusCode::UNAUTHORIZED,
        "invalid_client",
        "Invalid client authentication",
    )
}

fn presented<'a>(headers: &HeaderMap, raw: &'a MachineRequest) -> Result<Presented<'a>, Error> {
    if let Some(value) = oauth_dpop::one_header(headers, "authorization", 4096)? {
        if raw.client_secret.is_some()
            || raw.client_assertion.is_some()
            || raw.client_assertion_type.is_some()
        {
            return Err(Error::bad(
                "invalid_request",
                "Multiple client authentication methods",
            ));
        }
        let (scheme, encoded) = oauth_dpop::authorization(value).ok_or_else(invalid_client)?;
        if !scheme.eq_ignore_ascii_case("Basic") {
            return Err(invalid_client());
        }
        let decoded = String::from_utf8(STANDARD.decode(encoded).map_err(|_| invalid_client())?)
            .map_err(|_| invalid_client())?;
        let (id, secret) = decoded.split_once(':').ok_or_else(invalid_client)?;
        let decode = |value: &str| {
            form_urlencoded::parse(format!("v={value}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()
        };
        let (id, secret) = (decode(id), decode(secret));
        if id.is_empty()
            || secret.is_empty()
            || raw.client_id.as_ref().is_some_and(|body| body != &id)
        {
            return Err(invalid_client());
        }
        return Ok(Presented::Basic(id, secret));
    }
    match (
        raw.client_assertion_type.as_deref(),
        raw.client_assertion.as_deref(),
        raw.client_secret.as_deref(),
        raw.client_id.as_deref(),
    ) {
        (Some(ASSERTION_TYPE), Some(assertion), None, id) if assertion.len() <= 8192 => {
            Ok(Presented::Assertion(id, assertion))
        }
        (None, None, Some(secret), Some(id)) if !secret.is_empty() && !id.is_empty() => {
            Ok(Presented::Post(id, secret))
        }
        (None, None, None, Some(id)) if !id.is_empty() => Ok(Presented::Public(id)),
        _ => Err(invalid_client()),
    }
}

fn assertion_subject(assertion: &str) -> Option<String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let body = assertion.split('.').nth(1)?;
    let value: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()?;
    Some(value.get("sub")?.as_str()?.to_owned())
}

async fn authenticate(
    db: &tokio_postgres::Client,
    app: &App,
    path: &str,
    presented: Presented<'_>,
) -> Result<Client, Error> {
    let subject = match &presented {
        Presented::Assertion(None, assertion) => assertion_subject(assertion),
        _ => None,
    };
    let id = presented
        .id()
        .or(subject.as_deref())
        .ok_or_else(invalid_client)?;
    let client = oauth_client::lookup(db, &app.origin, &app.resource, id)
        .await?
        .ok_or_else(invalid_client)?;
    if client.disabled == Some(true) {
        return Err(invalid_client());
    }
    let verified = match presented {
        Presented::Public(_) => client.method() == "none",
        Presented::Basic(_, secret) => {
            client.method() == "client_secret_basic"
                && client.client_secret.as_deref().is_some_and(|encrypted| {
                    oauth_client::verify_secret(&app.secret, encrypted, &secret)
                })
        }
        Presented::Post(_, secret) => {
            client.method() == "client_secret_post"
                && client.client_secret.as_deref().is_some_and(|encrypted| {
                    oauth_client::verify_secret(&app.secret, encrypted, secret)
                })
        }
        Presented::Assertion(_, assertion) => {
            client.method() == "private_key_jwt"
                && oauth_private_jwt::verify(
                    db,
                    assertion,
                    id,
                    &app.issuer,
                    &format!("{}{}", app.origin, path),
                    client.jwks.as_deref(),
                    client.jwks_uri.as_deref(),
                )
                .await?
        }
    };
    if !verified {
        return Err(invalid_client());
    }
    Ok(client)
}

pub(crate) async fn machine(
    db: &mut deadpool_postgres::Client,
    app: &App,
    path: &str,
    headers: &HeaderMap,
    raw: Value,
) -> Result<Response, Error> {
    let raw: MachineRequest = serde_json::from_value(raw)
        .map_err(|_| Error::bad("invalid_request", "Invalid OAuth parameters"))?;
    if path.ends_with("/token")
        && !oauth_client::GRANTS.contains(&raw.grant_type.as_deref().unwrap_or_default())
    {
        return Err(Error::bad(
            "unsupported_grant_type",
            "Unsupported grant type",
        ));
    }
    let credentials = match presented(headers, &raw) {
        Ok(credentials) => credentials,
        Err(error) => return Ok(auth_error(headers, error)),
    };
    let client = match authenticate(db, app, path, credentials).await {
        Ok(client) => client,
        Err(error) => return Ok(auth_error(headers, error)),
    };
    match path {
        "/api/auth/oauth2/token" => {
            let raw_grant = raw.grant_type.as_deref().unwrap_or_default();
            if !client.allows_grant(raw_grant) {
                return Err(Error::bad(
                    "unauthorized_client",
                    "Client cannot use the requested grant",
                ));
            }
            let proof = oauth_dpop::one_header(headers, "dpop", 8192)?;
            let response = if raw_grant == "authorization_code" {
                from_code(db, app, &client, &raw, proof).await?
            } else {
                from_refresh(db, app, &client, &raw, proof).await?
            };
            Ok(oauth::json_response(StatusCode::OK, response))
        }
        "/api/auth/oauth2/revoke" => {
            revoke(db, &client, &raw).await?;
            Ok(StatusCode::OK.into_response())
        }
        "/api/auth/oauth2/introspect" => {
            if client.method() == "none" {
                return Ok(auth_error(headers, invalid_client()));
            }
            Ok(oauth::json_response(
                StatusCode::OK,
                introspect(db, app, &client, &raw).await?,
            ))
        }
        _ => unreachable!("the router owns the machine endpoints"),
    }
}

fn auth_error(headers: &HeaderMap, error: Error) -> Response {
    let basic = headers
        .get(header::AUTHORIZATION)
        .and_then(|raw| raw.to_str().ok())
        .is_some_and(|raw| {
            raw.get(..6)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("basic "))
        });
    let mut response = error.response();
    if basic && response.status() == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic error=\"invalid_client\""),
        );
    }
    response
}

fn invalid_grant() -> Error {
    Error::bad("invalid_grant", "Invalid or expired grant")
}

fn required(raw: Option<&str>) -> Result<&str, Error> {
    raw.filter(|value| !value.is_empty() && value.len() <= 4096)
        .ok_or_else(|| Error::bad("invalid_request", "Missing or invalid token parameter"))
}

fn valid_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
}

fn validated_proof(
    app: &App,
    client: &Client,
    policy: &Policy,
    proof: Option<&str>,
    jkt: Option<&str>,
) -> Result<Option<oauth_dpop::Proof>, Error> {
    let invalid = || {
        Error::bad(
            "invalid_dpop_proof",
            "DPoP proof is missing, invalid or already used",
        )
    };
    match proof {
        Some(proof) => oauth_dpop::verify(
            proof,
            "POST",
            &format!("{}/oauth2/token", app.issuer),
            jkt,
            None,
        )
        .map(Some)
        .ok_or_else(invalid),
        None if jkt.is_some() || client.requires_dpop() || policy.dpop => Err(invalid()),
        None => Ok(None),
    }
}

async fn reserve_proof(
    db: &impl GenericClient,
    proof: Option<oauth_dpop::Proof>,
) -> Result<Option<Value>, Error> {
    let Some(proof) = proof else { return Ok(None) };
    if !proof.reserve(db).await? {
        return Err(Error::bad(
            "invalid_dpop_proof",
            "DPoP proof is missing, invalid or already used",
        ));
    }
    Ok(Some(json!({"jkt":proof.jkt})))
}

async fn lock_family(db: &impl GenericClient, client: &str, user: &str) -> Result<(), Error> {
    db.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        &[&format!("oauth-refresh:{client}:{user}")],
    )
    .await?;
    Ok(())
}

async fn active_session(db: &impl GenericClient, session: &str, user: &str) -> Result<bool, Error> {
    Ok(db
        .query_opt(
            r#"SELECT id FROM neon_auth.session WHERE id=$1::text::uuid AND "userId"=$2::text::uuid
           AND "expiresAt">statement_timestamp() FOR KEY SHARE"#,
            &[&session, &user],
        )
        .await?
        .is_some())
}

async fn guard_session_mutations(db: &impl GenericClient) -> Result<(), Error> {
    // Session deletion updates token foreign keys. Take the same lock as sign-out and
    // password rotation before touching either token table.
    db.query_one(
        "SELECT pg_advisory_xact_lock_shared(hashtextextended('auth-mutations', 0))",
        &[],
    )
    .await?;
    Ok(())
}

struct TokenIssue<'a> {
    client_id: &'a str,
    user_id: &'a str,
    session_id: &'a str,
    code_id: Option<&'a str>,
    reference_id: Option<&'a str>,
    auth_time: Option<f64>,
    confirmation: Option<Value>,
    resources: &'a [String],
    refresh_resources: &'a [String],
    refresh: bool,
    policy: Policy,
}

impl TokenIssue<'_> {
    async fn save(self, db: &impl GenericClient) -> Result<Value, Error> {
        let now = oauth::now_seconds() as f64;
        let access_expires = now + f64::from(self.policy.access_ttl);
        let refresh_expires = now + f64::from(self.policy.refresh_ttl);
        let scope = json!(self.policy.scopes);
        let (refresh, refresh_id): (Option<String>, Option<String>) = if self.refresh {
            let refresh = oauth::random_token()?;
            let id = db.query_one(
                r#"INSERT INTO neon_auth."oauthRefreshToken" (token, "clientId", "sessionId", "userId",
                     "authorizationCodeId", "referenceId", "authTime", scopes, resources, confirmation, "createdAt", "expiresAt")
                   VALUES ($1, $2, $3::text::uuid, $4::text::uuid, $5, $6, to_timestamp($7), $8, $9, $10, to_timestamp($11), to_timestamp($12))
                   RETURNING id::text"#,
                &[&oauth::hash(refresh.as_bytes()), &self.client_id, &self.session_id, &self.user_id,
                  &self.code_id, &self.reference_id, &self.auth_time, &scope, &json!(self.refresh_resources), &self.confirmation, &now, &refresh_expires],
            ).await?.get(0);
            (Some(refresh), Some(id))
        } else {
            (None, None)
        };
        let access = oauth::random_token()?;
        db.execute(
            r#"INSERT INTO neon_auth."oauthAccessToken" (token, "clientId", "sessionId", "userId",
                 "authorizationCodeId", "referenceId", "refreshId", scopes, resources, confirmation, "createdAt", "expiresAt")
               VALUES ($1, $2, $3::text::uuid, $4::text::uuid, $5, $6, $7::text::uuid, $8, $9, $10, to_timestamp($11), to_timestamp($12))"#,
            &[&oauth::hash(access.as_bytes()), &self.client_id, &self.session_id, &self.user_id,
              &self.code_id, &self.reference_id, &refresh_id, &scope, &json!(self.resources), &self.confirmation, &now, &access_expires],
        ).await?;
        let mut response = json!({
            "token_type": if self.confirmation.is_some() { "DPoP" } else { "Bearer" },
            "access_token": access,
            "expires_in": self.policy.access_ttl,
            "expires_at": access_expires as i64,
            "scope": self.policy.scopes.join(" "),
        });
        if let Some(refresh) = refresh {
            response["refresh_token"] = json!(refresh);
        }
        Ok(response)
    }
}

async fn from_code(
    db: &mut deadpool_postgres::Client,
    app: &App,
    client: &Client,
    raw: &MachineRequest,
    proof: Option<&str>,
) -> Result<Value, Error> {
    let code = required(raw.code.as_deref())?;
    let code_id = oauth::hash(code.as_bytes());
    let tx = db.transaction().await?;
    guard_session_mutations(&*tx).await?;
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        &[&code_id],
    )
    .await?;
    let row = tx
        .query_opt(
            r#"SELECT id::text, value FROM neon_auth.verification
           WHERE identifier=$1 AND "expiresAt">statement_timestamp()
           ORDER BY "createdAt" DESC LIMIT 1 FOR UPDATE"#,
            &[&code_id],
        )
        .await?;
    let Some(row) = row else {
        if let Some(issued) = tx
            .query_opt(
                r#"SELECT "userId"::text FROM neon_auth."oauthRefreshToken"
               WHERE "authorizationCodeId"=$1 AND "clientId"=$2 LIMIT 1"#,
                &[&code_id, &client.client_id],
            )
            .await?
        {
            lock_family(&*tx, &client.client_id, &issued.get::<_, String>(0)).await?;
        }
        tx.execute(
            r#"UPDATE neon_auth."oauthAccessToken" SET revoked=now()
                       WHERE "authorizationCodeId"=$1 AND "clientId"=$2 AND revoked IS NULL"#,
            &[&code_id, &client.client_id],
        )
        .await?;
        tx.execute(
            r#"UPDATE neon_auth."oauthRefreshToken" SET revoked=now()
                       WHERE "authorizationCodeId"=$1 AND "clientId"=$2 AND revoked IS NULL"#,
            &[&code_id, &client.client_id],
        )
        .await?;
        tx.commit().await?;
        return Err(invalid_grant());
    };
    let verification_id: String = row.get(0);
    let verification: Code = serde_json::from_str(row.get(1)).map_err(|_| invalid_grant())?;
    let query = &verification.query;
    if verification.kind != "authorization_code" || query.client_id != client.client_id {
        return Err(invalid_grant());
    }
    if query.redirect_uri.is_some() && raw.redirect_uri.is_none() {
        return Err(Error::bad("invalid_request", "redirect_uri is required"));
    }
    if raw.redirect_uri != query.redirect_uri {
        return Err(invalid_grant());
    }
    let scopes = oauth::scopes(query.scope.as_deref().ok_or_else(invalid_grant)?);
    if !client.allows_scopes(&scopes) {
        return Err(Error::bad(
            "invalid_scope",
            "Client cannot use the requested scopes",
        ));
    }
    match (&query.code_challenge, raw.code_verifier.as_deref()) {
        (Some(challenge), Some(verifier)) => {
            if !valid_verifier(verifier)
                || query.code_challenge_method.as_deref() != Some("S256")
                || oauth::hash(verifier.as_bytes()) != *challenge
            {
                return Err(invalid_grant());
            }
        }
        (None, None) if !client.requires_pkce(&scopes) => (),
        _ => return Err(Error::bad("invalid_request", "Valid PKCE is required")),
    }
    let original = verification
        .resource
        .as_deref()
        .filter(|resources| !resources.is_empty())
        .or_else(|| {
            query
                .resource
                .as_ref()
                .map(Resources::list)
                .filter(|resources| !resources.is_empty())
        });
    let resources = raw
        .resource
        .as_ref()
        .map(Resources::list)
        .or(original)
        .unwrap_or_default();
    if original.is_some_and(|original| {
        resources
            .iter()
            .any(|resource| !original.contains(resource))
    }) {
        return Err(Error::bad(
            "invalid_target",
            "Requested resource is outside the grant",
        ));
    }
    let policy = policy(&*tx, app, &client.client_id, resources, &scopes).await?;
    let proof = validated_proof(app, client, &policy, proof, query.dpop_jkt.as_deref())?;
    lock_family(&*tx, &client.client_id, &verification.user_id).await?;
    if !active_session(&*tx, &verification.session_id, &verification.user_id).await? {
        return Err(invalid_grant());
    }
    let confirmation = reserve_proof(&*tx, proof).await?;
    tx.execute(
        "DELETE FROM neon_auth.verification WHERE id=$1::text::uuid",
        &[&verification_id],
    )
    .await?;
    let response = TokenIssue {
        client_id: &client.client_id,
        user_id: &verification.user_id,
        session_id: &verification.session_id,
        code_id: Some(&code_id),
        reference_id: verification.reference_id.as_deref(),
        auth_time: verification.auth_time.map(|ms| ms as f64 / 1000.0),
        resources,
        refresh_resources: original.unwrap_or(resources),
        confirmation,
        refresh: client.allows_grant("refresh_token")
            && scopes.iter().any(|scope| scope == "offline_access"),
        policy,
    }
    .save(&*tx)
    .await?;
    tx.commit().await?;
    Ok(response)
}

async fn revoke_family(db: &impl GenericClient, client: &str, user: &str) -> Result<(), Error> {
    db.execute(
        r#"UPDATE neon_auth."oauthAccessToken" SET revoked=now()
           WHERE "clientId"=$1 AND "userId"=$2::text::uuid AND "refreshId" IS NOT NULL AND revoked IS NULL"#,
        &[&client, &user],
    ).await?;
    db.execute(
        r#"UPDATE neon_auth."oauthRefreshToken" SET revoked=now()
           WHERE "clientId"=$1 AND "userId"=$2::text::uuid AND revoked IS NULL"#,
        &[&client, &user],
    )
    .await?;
    Ok(())
}

async fn from_refresh(
    db: &mut deadpool_postgres::Client,
    app: &App,
    client: &Client,
    raw: &MachineRequest,
    proof: Option<&str>,
) -> Result<Value, Error> {
    let token = required(raw.refresh_token.as_deref())?;
    let hash = oauth::hash(token.as_bytes());
    let tx = db.transaction().await?;
    guard_session_mutations(&*tx).await?;
    let identity = tx
        .query_opt(
            r#"SELECT "userId"::text, "sessionId"::text FROM neon_auth."oauthRefreshToken"
           WHERE token=$1 AND "clientId"=$2 AND "expiresAt">statement_timestamp()"#,
            &[&hash, &client.client_id],
        )
        .await?
        .ok_or_else(invalid_grant)?;
    let user: String = identity.get(0);
    let session: String = identity
        .get::<_, Option<String>>(1)
        .ok_or_else(invalid_grant)?;
    lock_family(&*tx, &client.client_id, &user).await?;
    if !active_session(&*tx, &session, &user).await? {
        return Err(invalid_grant());
    }
    let row = tx.query_opt(
        r#"SELECT id::text, "sessionId"::text, scopes, resources, confirmation, (revoked IS NOT NULL),
                  "authorizationCodeId", "referenceId", extract(epoch FROM "authTime")::double precision
           FROM neon_auth."oauthRefreshToken" WHERE token=$1 AND "clientId"=$2 AND "expiresAt">statement_timestamp()
             AND "sessionId"=$3::text::uuid AND "userId"=$4::text::uuid
           FOR UPDATE"#,
        &[&hash, &client.client_id, &session, &user],
    ).await?.ok_or_else(invalid_grant)?;
    let refresh_id: String = row.get(0);
    let prior_scopes: Vec<String> =
        serde_json::from_value(row.get(2)).map_err(|_| Error::Internal)?;
    let prior_resources: Option<Vec<String>> = row
        .get::<_, Option<Value>>(3)
        .map(|resources| serde_json::from_value(resources).map_err(|_| Error::Internal))
        .transpose()?;
    let confirmation: Option<Value> = row.get(4);
    let revoked: bool = row.get(5);
    let code_id: Option<String> = row.get(6);
    let reference_id: Option<String> = row.get(7);
    let auth_time: Option<f64> = row.get(8);
    let scopes = raw
        .scope
        .as_deref()
        .map(oauth::scopes)
        .unwrap_or_else(|| prior_scopes.clone());
    if scopes.iter().any(|scope| !prior_scopes.contains(scope)) || !client.allows_scopes(&scopes) {
        return Err(Error::bad(
            "invalid_scope",
            "Requested scopes are outside the grant",
        ));
    }
    let resources = raw
        .resource
        .as_ref()
        .map(Resources::list)
        .or(prior_resources.as_deref())
        .unwrap_or_default();
    if prior_resources
        .as_ref()
        .is_some_and(|allowed| resources.iter().any(|resource| !allowed.contains(resource)))
    {
        return Err(Error::bad(
            "invalid_target",
            "Requested resource is outside the grant",
        ));
    }
    if revoked {
        revoke_family(&*tx, &client.client_id, &user).await?;
        tx.commit().await?;
        return Err(invalid_grant());
    }
    let policy = policy(&*tx, app, &client.client_id, resources, &scopes).await?;
    let proof = validated_proof(
        app,
        client,
        &policy,
        proof,
        confirmation.as_ref().and_then(oauth_dpop::jkt),
    )?;
    let confirmation = reserve_proof(&*tx, proof).await?;
    let refresh = prior_scopes.iter().any(|scope| scope == "offline_access");
    if refresh {
        tx.execute(r#"UPDATE neon_auth."oauthRefreshToken" SET revoked=now(), "rotatedAt"=now() WHERE id=$1::text::uuid"#, &[&refresh_id]).await?;
    }
    let response = TokenIssue {
        client_id: &client.client_id,
        user_id: &user,
        session_id: &session,
        code_id: code_id.as_deref(),
        reference_id: reference_id.as_deref(),
        auth_time,
        resources,
        refresh_resources: prior_resources.as_deref().unwrap_or(resources),
        confirmation,
        refresh,
        policy,
    }
    .save(&*tx)
    .await?;
    tx.commit().await?;
    Ok(response)
}

async fn refresh_user(
    db: &impl GenericClient,
    hash: &str,
    client: &str,
) -> Result<Option<String>, Error> {
    Ok(db.query_opt(r#"SELECT "userId"::text FROM neon_auth."oauthRefreshToken" WHERE token=$1 AND "clientId"=$2"#,
        &[&hash, &client]).await?.map(|row| row.get(0)))
}

enum TokenHint {
    Access,
    Refresh,
    Unknown,
}

fn token_request(raw: &MachineRequest) -> Result<(String, TokenHint), Error> {
    let raw_token = required(raw.token.as_deref())?;
    let token = oauth_dpop::authorization(raw_token)
        .filter(|(scheme, _)| {
            scheme.eq_ignore_ascii_case("Bearer") || scheme.eq_ignore_ascii_case("DPoP")
        })
        .map_or(raw_token, |(_, token)| token);
    let hint = match raw.token_type_hint.as_deref() {
        Some("access_token") => TokenHint::Access,
        Some("refresh_token") => TokenHint::Refresh,
        _ => TokenHint::Unknown,
    };
    Ok((oauth::hash(token.as_bytes()), hint))
}

async fn revoke(
    db: &mut deadpool_postgres::Client,
    client: &Client,
    raw: &MachineRequest,
) -> Result<(), Error> {
    let (hash, hint) = token_request(raw)?;
    let tx = db.transaction().await?;
    guard_session_mutations(&*tx).await?;
    if !matches!(hint, TokenHint::Refresh) {
        let did_revoke = tx
            .execute(
                r#"UPDATE neon_auth."oauthAccessToken" SET revoked=now()
               WHERE token=$1 AND "clientId"=$2 AND revoked IS NULL"#,
                &[&hash, &client.client_id],
            )
            .await?;
        if did_revoke != 0 || matches!(hint, TokenHint::Access) {
            tx.commit().await?;
            return Ok(());
        }
    }
    if let Some(user) = refresh_user(&*tx, &hash, &client.client_id).await? {
        lock_family(&*tx, &client.client_id, &user).await?;
        if let Some(row) = tx
            .query_opt(
                r#"SELECT id::text, revoked IS NOT NULL FROM neon_auth."oauthRefreshToken"
               WHERE token=$1 AND "clientId"=$2 FOR UPDATE"#,
                &[&hash, &client.client_id],
            )
            .await?
        {
            let id: String = row.get(0);
            if row.get::<_, bool>(1) {
                revoke_family(&*tx, &client.client_id, &user).await?;
            } else {
                tx.execute(r#"UPDATE neon_auth."oauthRefreshToken" SET revoked=now() WHERE id=$1::text::uuid"#, &[&id]).await?;
                tx.execute(r#"UPDATE neon_auth."oauthAccessToken" SET revoked=now() WHERE "refreshId"=$1::text::uuid AND revoked IS NULL"#, &[&id]).await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

async fn introspect(
    db: &tokio_postgres::Client,
    app: &App,
    client: &Client,
    raw: &MachineRequest,
) -> Result<Value, Error> {
    let (hash, hint) = token_request(raw)?;
    let inactive = || json!({"active":false});
    if !matches!(hint, TokenHint::Refresh) {
        if let Some(row) = access_row(db, &hash, None).await? {
            let resources: Option<Vec<String>> = row
                .get::<_, Option<Value>>("resources")
                .map(|resources| serde_json::from_value(resources).map_err(|_| Error::Internal))
                .transpose()?;
            let issuing_client: String = row.get("client");
            if issuing_client != client.client_id {
                let Some(resources) = &resources else {
                    return Ok(inactive());
                };
                let linked: bool = db.query_one(
                    r#"SELECT EXISTS (SELECT 1 FROM neon_auth."oauthClientResource" WHERE "clientId"=$1 AND "resourceId"=ANY($2))"#,
                    &[&client.client_id, resources],
                ).await?.get(0);
                if !linked {
                    return Ok(inactive());
                }
            }
            if !access_client_active(db, &app.origin, &app.resource, &row).await {
                return Ok(inactive());
            }
            let mut result = introspection_response(&row, &app.issuer)?;
            result["azp"] = json!(issuing_client);
            if let Some(resources) = resources.filter(|resources| !resources.is_empty()) {
                result["aud"] = if resources.len() == 1 {
                    json!(resources[0])
                } else {
                    json!(resources)
                };
            }
            return Ok(result);
        }
        if matches!(hint, TokenHint::Access) {
            return Ok(inactive());
        }
    }
    let row = db.query_opt(
        r#"SELECT t."userId"::text AS "user", s.id::text AS session, t."clientId" AS client, t.scopes, t.confirmation,
                  floor(extract(epoch FROM t."expiresAt"))::bigint AS expires,
                  floor(extract(epoch FROM t."createdAt"))::bigint AS issued
           FROM neon_auth."oauthRefreshToken" t JOIN neon_auth.session s
             ON s.id=t."sessionId" AND s."userId"=t."userId"
           WHERE t.token=$1 AND t."clientId"=$2 AND t.revoked IS NULL
             AND t."expiresAt">statement_timestamp() AND s."expiresAt">statement_timestamp()"#,
        &[&hash, &client.client_id],
    ).await?;
    row.as_ref()
        .map(|row| introspection_response(row, &app.issuer))
        .unwrap_or_else(|| Ok(inactive()))
}

fn introspection_response(row: &Row, issuer: &str) -> Result<Value, Error> {
    let scopes: Vec<String> =
        serde_json::from_value(row.get("scopes")).map_err(|_| Error::Internal)?;
    let confirmation: Option<Value> = row.get("confirmation");
    let mut result = json!({
        "active":true, "iss":issuer, "client_id":row.get::<_, String>("client"),
        "sub":row.get::<_, String>("user"), "sid":row.get::<_, String>("session"),
        "iat":row.get::<_, i64>("issued"), "exp":row.get::<_, i64>("expires"), "scope":scopes.join(" "),
        "token_type":if confirmation.as_ref().and_then(oauth_dpop::jkt).is_some() { "DPoP" } else { "Bearer" },
    });
    if let Some(confirmation) = confirmation {
        result["cnf"] = confirmation;
    }
    Ok(result)
}
