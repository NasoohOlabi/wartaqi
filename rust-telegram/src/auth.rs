use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, StatusCode};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use url::Url;

use crate::storage::{Storage, now};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const SCOPE: &str = "mcp:read mcp:write offline_access";
const MINUTE: i64 = 60_000;

#[derive(Deserialize)]
struct ResourceMetadata {
    resource: String,
    authorization_servers: Vec<Url>,
}

#[derive(Deserialize)]
struct Provider {
    issuer: String,
    authorization_endpoint: Url,
    token_endpoint: Url,
    registration_endpoint: Url,
    revocation_endpoint: Option<Url>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Grant {
    access_token: String,
    refresh_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in: Option<f64>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Credentials {
    #[serde(flatten)]
    grant: Grant,
    client: String,
    expires: i64,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    chat: String,
    verifier: String,
    client: String,
}

#[derive(Serialize, Deserialize)]
struct Challenge {
    challenge: String,
    credentials: Credentials,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PhoneRole {
    Student,
    Teacher,
}

#[derive(Deserialize)]
struct PhoneGrant {
    token: String,
    role: PhoneRole,
}

pub(crate) enum Access {
    Student(String),
    Teacher { token: String, secret: String },
    OAuth(String),
}

#[derive(Debug)]
pub(crate) struct RequestFailure(pub StatusCode);

impl std::fmt::Display for RequestFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "provider request failed ({})", self.0.as_u16())
    }
}

impl std::error::Error for RequestFailure {}

pub(crate) struct Auth {
    pub(crate) store: Arc<Storage>,
    pub(crate) origin: Url,
    pub(crate) http: Client,
    secret: Option<String>,
    resource: String,
    redirect: String,
    registration: tokio::sync::Mutex<()>,
    oauth_state: Mutex<()>,
}

impl Auth {
    pub(crate) fn new(
        store: Arc<Storage>,
        origin: Url,
        public: Url,
        secret: Option<String>,
    ) -> Result<Self> {
        let resource = origin.join("/mcp")?.to_string();
        let redirect = public.join("/callback")?.to_string();
        Ok(Self {
            store,
            origin,
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()?,
            secret,
            resource,
            redirect,
            registration: tokio::sync::Mutex::new(()),
            oauth_state: Mutex::new(()),
        })
    }

    pub(crate) fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.store
            .get(key)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(Into::into)
    }

    async fn provider(&self) -> Result<Provider> {
        let url = self
            .origin
            .join("/.well-known/oauth-protected-resource/mcp")?;
        let resource: ResourceMetadata = response(self.http.get(url)).await?;
        let issuer = resource
            .authorization_servers
            .first()
            .ok_or("missing OAuth issuer")?;
        if resource.resource != self.resource || !self.trusted(issuer) {
            return Err("unexpected OAuth issuer or resource".into());
        }
        let discovery = self.origin.join(&format!(
            "/.well-known/oauth-authorization-server{}",
            issuer.path().trim_end_matches('/')
        ))?;
        let provider: Provider = response(self.http.get(discovery)).await?;
        if provider.issuer != issuer.as_str().trim_end_matches('/')
            || [
                &provider.authorization_endpoint,
                &provider.token_endpoint,
                &provider.registration_endpoint,
            ]
            .into_iter()
            .chain(provider.revocation_endpoint.iter())
            .any(|url| !self.trusted(url))
        {
            return Err("unexpected OAuth issuer or endpoint".into());
        }
        Ok(provider)
    }

    fn trusted(&self, url: &Url) -> bool {
        url.origin() == self.origin.origin()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    }

    async fn client(&self, provider: &Provider) -> Result<String> {
        #[derive(Deserialize)]
        struct Registered {
            client_id: String,
        }
        let _guard = self.registration.lock().await;
        let key = format!(
            "client:{}",
            hex_digest(format!("{} {}", provider.issuer, self.redirect).as_bytes())
        );
        if let Some(value) = self.get(&key)? {
            return Ok(value);
        }
        let registered: Registered = response(
            self.http
                .post(provider.registration_endpoint.clone())
                .json(&json!({
                    "client_name": "Wartaq Telegram",
                    "redirect_uris": [&self.redirect],
                    "token_endpoint_auth_method": "none",
                    "grant_types": ["authorization_code", "refresh_token"]
                })),
        )
        .await?;
        if registered.client_id.is_empty() {
            return Err("invalid client id".into());
        }
        self.store.set(&key, &json!(registered.client_id), 0)?;
        Ok(registered.client_id)
    }

    pub(crate) async fn begin(&self, chat: &str) -> Result<Url> {
        let provider = self.provider().await?;
        let client = self.client(&provider).await?;
        let guard = self.oauth_state.lock().map_err(|_| "OAuth lock failed")?;
        if self.pending(chat)? {
            return Err("OAuth code ready".into());
        }
        let active = format!("oauth-active:{chat}");
        let (state, pending) = match self.get::<String>(&active)? {
            Some(state) => {
                let pending = self
                    .get::<Pending>(&format!("oauth:{state}"))?
                    .ok_or("OAuth exchange in progress")?;
                if pending.chat != chat {
                    return Err("invalid OAuth state".into());
                }
                (state, pending)
            }
            None => {
                let state = nonce::<32>()?;
                let pending = Pending {
                    chat: chat.into(),
                    verifier: nonce::<32>()?,
                    client,
                };
                let timeout = now()? + 10 * MINUTE;
                self.store.replace(
                    &[],
                    &[
                        (&active, &json!(state), timeout),
                        (
                            &format!("oauth:{state}"),
                            &serde_json::to_value(&pending)?,
                            timeout,
                        ),
                    ],
                )?;
                (state, pending)
            }
        };
        drop(guard);
        let challenge =
            URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, pending.verifier.as_bytes()));
        let mut url = provider.authorization_endpoint;
        url.set_query(None);
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", &pending.client),
            ("redirect_uri", &self.redirect),
            ("resource", &self.resource),
            ("scope", SCOPE),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ]);
        Ok(url)
    }

    pub(crate) fn pending(&self, chat: &str) -> Result<bool> {
        Ok(self.store.get(&format!("link:{chat}"))?.is_some())
    }

    pub(crate) fn exchanging(&self, chat: &str) -> Result<bool> {
        let active = self.get::<String>(&format!("oauth-active:{chat}"))?;
        match active {
            Some(state) => Ok(self.store.get(&format!("oauth:{state}"))?.is_none()),
            None => Ok(false),
        }
    }

    pub(crate) async fn callback(&self, state: &str, code: &str) -> Result<String> {
        if state.len() != 43
            || !state
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || code.is_empty()
        {
            return Err("invalid callback".into());
        }
        let pending = {
            let _guard = self.oauth_state.lock().map_err(|_| "OAuth lock failed")?;
            let key = format!("oauth:{state}");
            let pending = self.get::<Pending>(&key)?.ok_or("expired OAuth state")?;
            if self
                .get::<String>(&format!("oauth-active:{}", pending.chat))?
                .as_deref()
                != Some(state)
            {
                return Err("cancelled OAuth state".into());
            }
            self.store.delete(&key)?;
            pending
        };
        let active = format!("oauth-active:{}", pending.chat);
        let exchanged = async {
            let provider = self.provider().await?;
            let grant = self
                .token(
                    provider.token_endpoint,
                    &[
                        ("grant_type", "authorization_code"),
                        ("code", code),
                        ("client_id", &pending.client),
                        ("code_verifier", &pending.verifier),
                        ("redirect_uri", &self.redirect),
                        ("resource", &self.resource),
                    ],
                )
                .await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(Challenge {
                challenge: confirmation_code()?,
                credentials: credentials(grant, pending.client)?,
            })
        }
        .await;
        let _guard = self.oauth_state.lock().map_err(|_| "OAuth lock failed")?;
        if self.get::<String>(&active)?.as_deref() != Some(state) {
            return Err("cancelled OAuth state".into());
        }
        match exchanged {
            Ok(challenge) => {
                let key = format!("link:{}", pending.chat);
                self.store.replace(
                    &[&active],
                    &[(
                        &key,
                        &serde_json::to_value(&challenge)?,
                        now()? + 5 * MINUTE,
                    )],
                )?;
                Ok(challenge.challenge)
            }
            Err(error) => {
                self.store.delete(&active)?;
                Err(error)
            }
        }
    }

    pub(crate) fn redeem(&self, chat: &str, challenge: &str) -> Result<bool> {
        let _guard = self.oauth_state.lock().map_err(|_| "OAuth lock failed")?;
        let key = format!("link:{chat}");
        let Some(pending) = self.get::<Challenge>(&key)? else {
            return Ok(false);
        };
        let attempts = format!("attempts:{chat}");
        if pending.challenge != challenge {
            let count = self.get::<u8>(&attempts)?.unwrap_or(0);
            if count >= 4 {
                self.store.delete(&key)?;
            } else {
                self.store
                    .set(&attempts, &json!(count + 1), now()? + 5 * MINUTE)?;
            }
            return Ok(false);
        }
        self.store.replace(
            &[&attempts, &key, &format!("oauth-refresh:{chat}")],
            &[(
                &format!("credentials:{chat}"),
                &serde_json::to_value(pending.credentials)?,
                0,
            )],
        )?;
        Ok(true)
    }

    pub(crate) fn cancel(&self, chat: &str) -> Result<()> {
        let _guard = self.oauth_state.lock().map_err(|_| "OAuth lock failed")?;
        let active = format!("oauth-active:{chat}");
        let mut remove = vec![
            active.clone(),
            format!("attempts:{chat}"),
            format!("link:{chat}"),
        ];
        if let Some(state) = self.get::<String>(&active)? {
            remove.push(format!("oauth:{state}"));
        }
        self.store
            .replace(&remove.iter().map(String::as_str).collect::<Vec<_>>(), &[])
    }

    pub(crate) fn phone_available(&self) -> bool {
        self.secret.is_some()
    }

    pub(crate) fn worker_secret(&self) -> Option<&str> {
        self.secret.as_deref()
    }

    pub(crate) fn linked(&self, chat: &str) -> Result<bool> {
        for kind in ["student", "teacher", "credentials"] {
            if self.store.get(&format!("{kind}:{chat}"))?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) async fn phone<T: DeserializeOwned>(&self, body: Value) -> Result<T> {
        let secret = self.secret.as_ref().ok_or("phone sign-in unavailable")?;
        response(
            self.http
                .post(self.origin.join("/api/telegram/student")?)
                .bearer_auth(secret)
                .json(&body),
        )
        .await
    }

    pub(crate) async fn login_phone(&self, chat: &str, phone: &str) -> Result<PhoneRole> {
        let grant: PhoneGrant = self
            .phone(json!({ "action": "login-phone", "phone": phone }))
            .await?;
        if grant.token.is_empty() {
            return Err("invalid phone grant".into());
        }
        self.unlink(chat).await?;
        let role = match grant.role {
            PhoneRole::Student => "student",
            PhoneRole::Teacher => "teacher",
        };
        self.store.set(
            &format!("{role}:{chat}"),
            &json!(grant.token),
            now()? + 30 * 24 * 60 * MINUTE,
        )?;
        Ok(grant.role)
    }

    pub(crate) async fn unlink(&self, chat: &str) -> Result<()> {
        let (tokens, grant) = {
            let _guard = self.oauth_state.lock().map_err(|_| "OAuth lock failed")?;
            self.take_links(chat)?
        };
        self.revoke_links(tokens, grant).await;
        Ok(())
    }

    // Call under oauth_state so retiring a grant and any in-flight refresh is atomic.
    fn take_links(&self, chat: &str) -> Result<([Option<String>; 2], Option<Credentials>)> {
        let teacher = format!("teacher:{chat}");
        let student = format!("student:{chat}");
        let credentials = format!("credentials:{chat}");
        let link = format!("link:{chat}");
        let active = format!("oauth-active:{chat}");
        let attempts = format!("attempts:{chat}");
        let refreshing = format!("oauth-refresh:{chat}");
        let tokens = [self.get::<String>(&student)?, self.get(&teacher)?];
        let grant = self
            .get::<Credentials>(&credentials)?
            .or(self.get::<Challenge>(&link)?.map(|value| value.credentials));
        let state = self
            .get::<String>(&active)?
            .map(|state| format!("oauth:{state}"));
        let mut removed = vec![
            teacher.as_str(),
            &student,
            &credentials,
            &link,
            &active,
            &attempts,
            &refreshing,
        ];
        removed.extend(state.as_deref());
        self.store.replace(&removed, &[])?;
        Ok((tokens, grant))
    }

    async fn revoke_links(&self, tokens: [Option<String>; 2], grant: Option<Credentials>) {
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            for token in tokens.into_iter().flatten() {
                let _ = self
                    .phone::<Value>(json!({ "action": "unlink", "token": token }))
                    .await;
            }
            if let Some(grant) = grant {
                if let Ok(provider) = self.provider().await {
                    self.revoke(grant, provider.revocation_endpoint).await;
                }
            }
        })
        .await;
    }

    async fn revoke(&self, grant: Credentials, endpoint: Option<Url>) {
        if let Some(endpoint) = endpoint {
            let revoke = |token: String| {
                self.http
                    .post(endpoint.clone())
                    .timeout(Duration::from_secs(5))
                    .form(&[("token", token), ("client_id", grant.client.clone())])
                    .send()
            };
            let _ = tokio::join!(
                revoke(grant.grant.access_token),
                revoke(grant.grant.refresh_token)
            );
        }
    }

    fn phone_access(&self, chat: &str) -> Result<Option<Access>> {
        if let Some(token) = self.get::<String>(&format!("student:{chat}"))? {
            return Ok(Some(Access::Student(token)));
        }
        if let (Some(token), Some(secret)) = (
            self.get::<String>(&format!("teacher:{chat}"))?,
            &self.secret,
        ) {
            return Ok(Some(Access::Teacher {
                token,
                secret: secret.clone(),
            }));
        }
        Ok(None)
    }

    pub(crate) fn current_access(&self, chat: &str) -> Result<Option<Access>> {
        if let Some(access) = self.phone_access(chat)? {
            return Ok(Some(access));
        }
        let now = now()?;
        Ok(self
            .get::<Credentials>(&format!("credentials:{chat}"))?
            .filter(|grant| grant.expires > now)
            .map(|grant| Access::OAuth(grant.grant.access_token)))
    }

    pub(crate) async fn access(&self, chat: &str) -> Result<Option<Access>> {
        if let Some(access) = self.phone_access(chat)? {
            return Ok(Some(access));
        }
        let key = format!("credentials:{chat}");
        let Some(saved) = self.get::<Credentials>(&key)? else {
            return Ok(None);
        };
        if saved.expires > now()? + MINUTE {
            return Ok(Some(Access::OAuth(saved.grant.access_token)));
        }
        let provider = self.provider().await?;
        let refresh = format!("oauth-refresh:{chat}");
        let claim = nonce::<18>()?;
        {
            let _guard = self.oauth_state.lock().map_err(|_| "OAuth lock failed")?;
            let current = self.get::<Credentials>(&key)?;
            if !current.is_some_and(|current| {
                current.client == saved.client
                    && current.grant.refresh_token == saved.grant.refresh_token
            }) {
                return self.current_access(chat);
            }
            // A refresh token with an uncertain response must never be sent again.
            self.store
                .replace(&[&key], &[(&refresh, &json!(claim), now()? + MINUTE)])?;
        }
        let result = self
            .token(
                provider.token_endpoint,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &saved.grant.refresh_token),
                    ("client_id", &saved.client),
                    ("resource", &self.resource),
                    ("scope", SCOPE),
                ],
            )
            .await
            .and_then(|grant| credentials(grant, saved.client));
        match result {
            Ok(next) => {
                {
                    let _guard = self.oauth_state.lock().map_err(|_| "OAuth lock failed")?;
                    if self.get::<String>(&refresh)?.as_deref() == Some(claim.as_str()) {
                        let access = Access::OAuth(next.grant.access_token.clone());
                        self.store
                            .replace(&[&refresh], &[(&key, &serde_json::to_value(next)?, 0)])?;
                        return Ok(Some(access));
                    }
                }
                self.revoke(next, provider.revocation_endpoint).await;
                self.current_access(chat)
            }
            Err(error) => {
                let links = {
                    let _guard = self.oauth_state.lock().map_err(|_| "OAuth lock failed")?;
                    if self.get::<String>(&refresh)?.as_deref() != Some(claim.as_str()) {
                        return self.current_access(chat);
                    }
                    if error.downcast_ref::<RequestFailure>().is_some_and(|error| {
                        matches!(error.0, StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED)
                    }) {
                        Some(self.take_links(chat)?)
                    } else {
                        self.store.delete(&refresh)?;
                        None
                    }
                };
                if let Some((tokens, grant)) = links {
                    self.revoke_links(tokens, grant).await;
                }
                Err(error)
            }
        }
    }

    async fn token(&self, endpoint: Url, data: &[(&str, &str)]) -> Result<Grant> {
        let grant: Grant = response(
            self.http
                .post(endpoint)
                .form(&data.iter().copied().collect::<BTreeMap<_, _>>()),
        )
        .await?;
        if grant.access_token.is_empty()
            || grant.refresh_token.is_empty()
            || grant.expires_in.is_some_and(|seconds| {
                !seconds.is_finite() || seconds <= 0.0 || seconds > i64::MAX as f64 / 2000.0
            })
        {
            return Err("invalid OAuth grant".into());
        }
        Ok(grant)
    }
}

fn credentials(grant: Grant, client: String) -> Result<Credentials> {
    Ok(Credentials {
        expires: now()? + (grant.expires_in.unwrap_or(3600.0) * 1000.0) as i64,
        grant,
        client,
    })
}

pub(crate) async fn response<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T> {
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(RequestFailure(response.status()).into());
    }
    Ok(response.json().await?)
}

pub(crate) fn nonce<const N: usize>() -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(random::<N>()?))
}

pub(crate) fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "random generation failed")?;
    Ok(bytes)
}

pub(crate) fn hex_digest(input: &[u8]) -> String {
    digest::digest(&digest::SHA256, input)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn confirmation_code() -> Result<String> {
    const WIDTH: u32 = 90_000_000;
    const MAX: u32 = (u32::MAX / WIDTH) * WIDTH;
    loop {
        let sample = u32::from_ne_bytes(random()?);
        if sample < MAX {
            return Ok((sample % WIDTH + 10_000_000).to_string());
        }
    }
}
