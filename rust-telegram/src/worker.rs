use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::{
    Router,
    extract::{Query, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::json;
use teloxide_core::types::{
    ButtonRequest, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton,
    KeyboardMarkup, KeyboardRemove, Message, MessageId, ParseMode, Update, UpdateKind,
};
use url::Url;

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;

use crate::agent::Agent;
use crate::assistant::{ActiveQuestions, HelpModel};
use crate::screens::{Screen, render};
use crate::{
    Telegram,
    auth::{Access, Auth, PhoneRole, RequestFailure, Result, nonce},
    polling::Poller,
    storage::{Storage, now},
};

const PHONE_TTL: i64 = 5 * 60 * 1000;
const BUTTON_TTL: i64 = 15 * 60 * 1000;
const LINK_PAGE: &str = include_str!("link.html");

pub struct Config {
    token: String,
    origin: Url,
    public: Url,
    path: PathBuf,
    storage_key: String,
    worker_secret: Option<String>,
    agent_enabled: bool,
    pub port: u16,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        fn required(name: &str) -> Result<String> {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("missing {name}").into())
        }
        fn origin(name: &str) -> Result<Url> {
            let value = required(name)?;
            let url: Url = value.parse().map_err(|_| format!("invalid {name}"))?;
            if url.scheme() != "https"
                || url.origin().ascii_serialization() != value
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(format!("{name} must be a canonical HTTPS origin").into());
            }
            Ok(url)
        }
        let worker_secret = std::env::var("TELEGRAM_WORKER_SECRET").ok();
        if worker_secret
            .as_deref()
            .is_some_and(|secret| secret.len() < 32)
        {
            return Err("invalid TELEGRAM_WORKER_SECRET".into());
        }
        let port = std::env::var("PORT")
            .unwrap_or_else(|_| "3000".into())
            .parse()
            .ok()
            .filter(|port| *port > 0)
            .ok_or("invalid PORT")?;
        let agent_enabled = match std::env::var("TELEGRAM_LANGGRAPH_ENABLED").ok().as_deref() {
            Some("1") => true,
            Some("0") | None => false,
            Some(_) => return Err("invalid TELEGRAM_LANGGRAPH_ENABLED".into()),
        };
        Ok(Self {
            token: required("TELEGRAM_BOT_TOKEN")?,
            origin: origin("WARTAQI_ORIGIN")?,
            public: origin("TELEGRAM_PUBLIC_ORIGIN")?,
            path: required("TELEGRAM_DB_PATH")?.into(),
            storage_key: required("TELEGRAM_STORAGE_KEY")?,
            worker_secret,
            agent_enabled,
            port,
        })
    }
}

#[derive(Clone)]
pub struct Worker {
    telegram: Telegram,
    auth: Arc<Auth>,
    assistant: Arc<HelpModel>,
    agent: Option<Arc<Agent>>,
    active_questions: Arc<ActiveQuestions>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Language {
    En,
    Ar,
}

impl Language {
    pub(crate) fn text<'a>(self, en: &'a str, ar: &'a str) -> &'a str {
        match self {
            Self::En => en,
            Self::Ar => ar,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Action {
    pub(crate) route: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) args: HashMap<String, String>,
}

impl Action {
    pub(crate) fn new(route: impl Into<String>) -> Self {
        Self {
            route: route.into(),
            args: HashMap::new(),
        }
    }
    pub(crate) fn arg(mut self, name: &str, value: impl Into<String>) -> Self {
        self.args.insert(name.into(), value.into());
        self
    }
    pub(crate) fn value(&self, name: &str) -> &str {
        self.args.get(name).map(String::as_str).unwrap_or_default()
    }
}

#[derive(Deserialize, Serialize)]
struct Choice {
    chat: String,
    user: u64,
    message: i32,
    generation: String,
    action: Action,
}

#[derive(Deserialize, Serialize)]
struct Prompt {
    user: u64,
    generation: String,
    action: Action,
}

#[derive(Deserialize, Serialize)]
struct PhonePrompt {
    user: u64,
    message: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<String>,
}

enum AssistantAction {
    Bound(Action),
    Legacy { data: String, message: MessageId },
}

#[derive(Clone, Copy)]
struct PrivateChat {
    id: ChatId,
    user: u64,
}

impl PrivateChat {
    fn new(chat: &teloxide_core::types::Chat, user: teloxide_core::types::UserId) -> Option<Self> {
        (chat.is_private() && u64::try_from(chat.id.0).ok() == Some(user.0)).then_some(Self {
            id: chat.id,
            user: user.0,
        })
    }
    fn name(self) -> String {
        self.id.0.to_string()
    }
    fn key(self, prefix: &str) -> String {
        format!("{prefix}:{}", self.id.0)
    }
}

impl Worker {
    pub fn new(config: Config) -> Result<Self> {
        let telegram = Telegram::new(&config.token, "https://api.telegram.org/".parse()?)?;
        let store = Arc::new(Storage::open(config.path, &config.storage_key)?);
        let auth = Auth::new(store, config.origin, config.public, config.worker_secret)?;
        Ok(Self {
            telegram,
            auth: Arc::new(auth),
            assistant: Arc::new(HelpModel::from_env()?),
            agent: config
                .agent_enabled
                .then(Agent::from_env)
                .transpose()?
                .map(Arc::new),
            active_questions: Arc::default(),
        })
    }

    pub async fn run(self, listener: tokio::net::TcpListener) -> Result<()> {
        if self.assistant.configured() {
            let model = self.assistant.clone();
            tokio::spawn(async move {
                let _ = model.smoke("startup").await;
            });
        }
        let mut poller = Poller::new(&self.telegram, &self.auth.store);
        let http = axum::serve(listener, self.router(poller.readiness())).into_future();
        let poll = poller.run(
            |_| true,
            |update| self.handle(update),
            |update| self.notify_failure(update),
        );
        tokio::select! {
            result = http => result?,
            _ = poll => {},
            result = shutdown() => result?,
        }
        Ok(())
    }

    fn router(&self, ready: Arc<AtomicBool>) -> Router {
        Router::new()
            .route("/health", any(health))
            .route("/callback", any(callback))
            .fallback(|| async { (StatusCode::NOT_FOUND, "Not found") })
            .with_state(HttpState {
                auth: self.auth.clone(),
                ready,
            })
    }

    fn language(&self, chat: PrivateChat) -> Result<Language> {
        Ok(self.auth.get(&chat.key("lang"))?.unwrap_or(Language::En))
    }
    async fn say(&self, chat: PrivateChat, text: impl Into<String>) -> Result<()> {
        self.telegram.send_message(chat.id, text, None).await?;
        Ok(())
    }
    async fn remove_keyboard(&self, chat: PrivateChat, text: &str) -> Result<()> {
        self.telegram
            .send_message(chat.id, text, Some(KeyboardRemove::new().into()))
            .await?;
        Ok(())
    }

    fn clear(&self, chat: PrivateChat, keys: &[&str]) -> Result<()> {
        let keys: Vec<String> = keys.iter().map(|key| chat.key(key)).collect();
        self.auth
            .store
            .replace(&keys.iter().map(String::as_str).collect::<Vec<_>>(), &[])
    }

    fn invalidate_assistant(&self, chat: PrivateChat) -> Result<()> {
        self.clear(chat, &["assistant-epoch", "agent-generation"])?;
        Agent::invalidate(&self.auth, &chat.name())
    }

    async fn request_phone(
        &self,
        chat: PrivateChat,
        lang: Language,
        expected: Option<String>,
    ) -> Result<()> {
        if !self.auth.phone_available() || self.auth.linked(&chat.name())? {
            return Ok(());
        }
        self.clear(chat, &["prompt", "generation", "phone-input"])?;
        let keyboard = KeyboardMarkup::new([[KeyboardButton::new(
            lang.text("Share my phone", "مشاركة رقم هاتفي"),
        )
        .request(ButtonRequest::Contact)]])
        .resize_keyboard()
        .one_time_keyboard();
        let sent = self.telegram.send_message(chat.id, lang.text(
            "Tap Share my phone to confirm your number with Telegram. If the button is hidden, open the bot keyboard beside the message field.",
            "اضغط «مشاركة رقم هاتفي» لتأكيد رقمك عبر تيليجرام. إذا لم يظهر الزر، افتح لوحة مفاتيح البوت بجانب حقل الرسالة."
        ), Some(keyboard.into())).await?;
        self.auth.store.set(
            &chat.key("phone"),
            &serde_json::to_value(PhonePrompt {
                user: chat.user,
                message: sent.id.0,
                expected,
            })?,
            now()? + PHONE_TTL,
        )
    }

    async fn show(
        &self,
        chat: PrivateChat,
        mut action: Action,
        input: Option<&str>,
        message: Option<MessageId>,
    ) -> Result<()> {
        let mut lang = self.language(chat)?;
        let chat_name = chat.name();
        let browser_link = action.route == "sys.linkaccount";
        match action.route.as_str() {
            "sys.language" => {
                lang = match lang {
                    Language::En => Language::Ar,
                    Language::Ar => Language::En,
                };
                self.auth
                    .store
                    .set(&chat.key("lang"), &json!(lang.text("en", "ar")), 0)?;
                action = Action::new("home");
            }
            "sys.phone" => return self.request_phone(chat, lang, None).await,
            "sys.unlink" => {
                self.auth.unlink(&chat_name).await?;
                self.clear(
                    chat,
                    &[
                        "generation",
                        "prompt",
                        "phone",
                        "phone-input",
                        "agent-pending",
                        "agent-context",
                    ],
                )?;
                return self
                    .remove_keyboard(chat, lang.text("Account unlinked.", "تم إلغاء الربط."))
                    .await;
            }
            "sys.linkaccount" | "sys.home" => action = Action::new("home"),
            _ => {}
        }
        let Some(access) = self.auth.access(&chat_name).await? else {
            return self.unlinked(chat, lang, browser_link).await;
        };
        let screen = match render(&self.auth, &access, lang, &action, input).await {
            Ok(screen) => screen,
            Err(error) => {
                let status = error
                    .downcast_ref::<RequestFailure>()
                    .map(|failure| failure.0);
                if status == Some(StatusCode::UNAUTHORIZED)
                    || (!matches!(access, Access::OAuth(_))
                        && status == Some(StatusCode::FORBIDDEN))
                {
                    self.auth.unlink(&chat_name).await?;
                }
                return Err(error);
            }
        };
        self.deliver(chat, screen, message).await
    }

    async fn unlinked(&self, chat: PrivateChat, lang: Language, browser_link: bool) -> Result<()> {
        self.clear(chat, &["prompt", "generation"])?;
        let chat_name = chat.name();
        let pending = lang.text(
            "Send the eight-digit code shown in your browser here. To start again, use /cancel then /linkaccount.",
            "أرسل الرمز المكوّن من ٨ أرقام المعروض في المتصفح هنا. للبدء من جديد، استخدم /cancel ثم /linkaccount.",
        );
        let waiting = lang.text(
            "Linking is in progress. Wait for the browser code, or use /cancel then /linkaccount to try again.",
            "جارٍ إكمال الربط. انتظر رمز المتصفح، أو استخدم /cancel ثم /linkaccount للمحاولة من جديد.",
        );
        if self.auth.pending(&chat_name)? {
            return self.say(chat, pending).await;
        }
        if self.auth.exchanging(&chat_name)? {
            return self.say(chat, waiting).await;
        }
        if self.auth.phone_available() && !browser_link {
            self.telegram.send(chat.id, lang.text(
                "<b>Type your phone number</b> or click on /sharephone to share your Telegram number directly.",
                "<b>اكتب رقم هاتفك</b> أو اضغط على /sharephone لمشاركة رقم تيليجرام مباشرة."
            ), None, Some(ParseMode::Html)).await?;
            self.auth.store.set(
                &chat.key("phone-input"),
                &json!(chat.user),
                now()? + PHONE_TTL,
            )?;
            return Ok(());
        }
        let url = match self.auth.begin(&chat_name).await {
            Ok(url) => url,
            Err(_) if self.auth.pending(&chat_name)? => return self.say(chat, pending).await,
            Err(_) if self.auth.exchanging(&chat_name)? => return self.say(chat, waiting).await,
            Err(_) => return self.say(chat, lang.text(
                "The account link is temporarily unavailable. Use /linkaccount to try again.",
                "تعذر فتح رابط الحساب مؤقتاً. استخدم /linkaccount للمحاولة مجدداً."
            )).await,
        };
        let keyboard = InlineKeyboardMarkup::new([[InlineKeyboardButton::url(
            lang.text("Link account", "ربط الحساب"),
            url,
        )]]);
        self.telegram.send_message(chat.id, lang.text(
            "Open Wartaq in your browser, sign in and approve the link, then paste the eight-digit code here.",
            "افتح وارتق في المتصفح، وسجّل الدخول ووافق على الربط، ثم أرسل الرمز المكوّن من ثمانية أرقام هنا."
        ), Some(keyboard.into())).await?;
        Ok(())
    }

    async fn deliver(
        &self,
        chat: PrivateChat,
        screen: Screen,
        message: Option<MessageId>,
    ) -> Result<()> {
        let agent_proposal = screen.rows.iter().flatten().any(|button| {
            matches!(
                button, crate::screens::Button::Choice(_, action) if is_agent_action(&action.route)
            )
        });
        if screen.rows.is_empty() && screen.prompt.is_none() && message.is_none() {
            self.say(chat, screen.text).await?;
            if let Some((name, bytes)) = screen.document {
                self.telegram.send_document(chat.id, name, bytes).await?;
            }
            return Ok(());
        }
        let generation = nonce::<18>()?;
        let mut pending = Vec::new();
        let keyboard = InlineKeyboardMarkup::new(
            screen
                .rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|button| match button {
                            crate::screens::Button::Link(label, url) => {
                                Ok(InlineKeyboardButton::url(label, url))
                            }
                            crate::screens::Button::Choice(label, action) => {
                                let handle = nonce::<18>()?;
                                pending.push((handle.clone(), action));
                                Ok(InlineKeyboardButton::callback(label, handle))
                            }
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<Vec<_>>>()?,
        );
        let sent = if let Some(message) = message {
            self.telegram
                .edit_message(chat.id, message, screen.text, Some(keyboard))
                .await?
        } else {
            self.telegram
                .send_message(chat.id, screen.text, Some(keyboard.into()))
                .await?
        };
        let expires = now()? + BUTTON_TTL;
        let generation_key = if agent_proposal {
            "agent-generation"
        } else {
            "generation"
        };
        let mut values = vec![(
            chat.key(generation_key),
            json!(generation),
            if agent_proposal { expires } else { 0 },
        )];
        for (handle, action) in pending {
            values.push((
                format!("button:{handle}"),
                serde_json::to_value(Choice {
                    chat: chat.name(),
                    user: chat.user,
                    message: sent.id.0,
                    generation: generation.clone(),
                    action,
                })?,
                expires,
            ));
        }
        if let Some(action) = screen.prompt {
            values.push((
                chat.key("prompt"),
                serde_json::to_value(Prompt {
                    user: chat.user,
                    generation,
                    action,
                })?,
                expires,
            ));
        }
        let prompt_key = chat.key("prompt");
        let remove = [prompt_key.as_str()];
        self.auth.store.replace(
            if agent_proposal { &[] } else { &remove },
            &values
                .iter()
                .map(|(key, value, ttl)| (key.as_str(), value, *ttl))
                .collect::<Vec<_>>(),
        )?;
        if let Some((name, bytes)) = screen.document {
            self.telegram.send_document(chat.id, name, bytes).await?;
        }
        Ok(())
    }

    async fn handle(&self, update: Update) -> Result<()> {
        match update.kind {
            UpdateKind::CallbackQuery(callback) => {
                let _ = self.telegram.answer_callback(callback.id.0).await;
                let Some(message) = callback.message else {
                    return Ok(());
                };
                let Some(chat) = PrivateChat::new(message.chat(), callback.from.id) else {
                    return Ok(());
                };
                let Some(handle) = callback.data else {
                    return Ok(());
                };
                if handle.len() > 64 {
                    return Ok(());
                }
                if handle.starts_with("agent:c:") || handle.starts_with("agent:x:") {
                    return self
                        .assistant_callback(
                            chat,
                            AssistantAction::Legacy {
                                data: handle,
                                message: message.id(),
                            },
                        )
                        .await;
                }
                let key = format!("button:{handle}");
                let choice: Option<Choice> = self.auth.get(&key)?;
                let agent_action = choice
                    .as_ref()
                    .is_some_and(|choice| is_agent_action(&choice.action.route));
                let current: Option<String> = self.auth.get(&chat.key(if agent_action {
                    "agent-generation"
                } else {
                    "generation"
                }))?;
                let Some(choice) = choice.filter(|choice| {
                    choice.chat == chat.name()
                        && choice.user == chat.user
                        && choice.message == message.id().0
                        && current.as_deref() == Some(choice.generation.as_str())
                }) else {
                    return self
                        .say(
                            chat,
                            self.language(chat)?.text(
                                "This choice has expired. Use /menu.",
                                "انتهت صلاحية هذا الخيار. استخدم /menu.",
                            ),
                        )
                        .await;
                };
                if agent_action {
                    return self
                        .assistant_callback(chat, AssistantAction::Bound(choice.action))
                        .await;
                }
                self.invalidate_assistant(chat)?;
                self.auth
                    .store
                    .replace(&[&key, &chat.key("generation"), &chat.key("prompt")], &[])?;
                self.show(chat, choice.action, None, Some(message.id()))
                    .await
            }
            UpdateKind::Message(message) => {
                let Some(user) = message.from.as_ref() else {
                    return Ok(());
                };
                let Some(chat) = PrivateChat::new(&message.chat, user.id) else {
                    return Ok(());
                };
                if self.auth.store.get(&chat.key("lang"))?.is_none()
                    && user
                        .language_code
                        .as_deref()
                        .is_some_and(|lang| lang.starts_with("ar"))
                {
                    self.auth.store.set(&chat.key("lang"), &json!("ar"), 0)?;
                }
                self.message(chat, self.language(chat)?, message).await
            }
            _ => Ok(()),
        }
    }

    async fn message(&self, chat: PrivateChat, lang: Language, message: Message) -> Result<()> {
        let pending: Option<PhonePrompt> = self.auth.get(&chat.key("phone"))?;
        if let Some(contact) = message.contact() {
            self.invalidate_assistant(chat)?;
            return self.contact(chat, lang, &message, contact, pending).await;
        }
        let Some(text) = message.text().map(str::trim) else {
            return Ok(());
        };
        let chat_name = chat.name();
        let command = text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .split('@')
            .next()
            .unwrap_or("");
        if matches!(
            command,
            "/cancel"
                | "/menu"
                | "/start"
                | "/unlink"
                | "/linkaccount"
                | "/sharephone"
                | "/language"
        ) {
            self.invalidate_assistant(chat)?;
        }
        let is_code = text.len() == 8 && text.bytes().all(|byte| byte.is_ascii_digit());
        let has_code = self.auth.pending(&chat_name)?;
        if is_code && has_code && self.auth.redeem(&chat_name, text)? {
            self.invalidate_assistant(chat)?;
            self.dismiss_phone(chat, lang, pending.is_some()).await?;
            return self.show(chat, Action::new("home"), None, None).await;
        }
        if let Some(phone) = &pending {
            if !text.starts_with('/') && !(is_code && has_code) {
                self.say(
                    chat,
                    lang.text(
                        "Use the button to share your number.",
                        "استخدم الزر لمشاركة رقمك.",
                    ),
                )
                .await?;
                return self.request_phone(chat, lang, phone.expected.clone()).await;
            }
        }
        let digits = normalize_digits(text);
        if !digits.is_empty()
            && digits
                .chars()
                .all(|ch| ch.is_ascii_digit() || " ()+-".contains(ch))
            && digits.bytes().any(|byte| byte.is_ascii_digit())
            && self.auth.get::<u64>(&chat.key("phone-input"))? == Some(chat.user)
            && !has_code
            && !self.auth.linked(&chat_name)?
        {
            if let Some(phone) = normalize_phone(text, false) {
                return self.request_phone(chat, lang, Some(phone)).await;
            }
            return self.phone_issue(chat, lang,
                "Check the digits and try again. Syrian mobile numbers can start with 09 or +9639.",
                "تحقق من الأرقام وحاول مجدداً. يمكن أن تبدأ الأرقام السورية بـ 09 أو +9639.",
            ).await;
        }
        if is_code && (has_code || self.auth.store.get(&chat.key("credentials"))?.is_none()) {
            return self.say(chat, lang.text(
                "That code is incorrect or expired. Try again, or use /cancel then /linkaccount to link again.",
                "الرمز غير صحيح أو انتهت صلاحيته. حاول مجدداً، أو استخدم /cancel ثم /linkaccount لبدء الربط من جديد."
            )).await;
        }
        match command {
            "/linkaccount" | "/language" | "/menu" | "/start" => {
                self.dismiss_phone(chat, lang, pending.is_some()).await?;
                self.clear(chat, &["prompt", "generation"])?;
                let route = match command {
                    "/linkaccount" => "sys.linkaccount",
                    "/language" => "sys.language",
                    _ => "home",
                };
                self.show(chat, Action::new(route), None, None).await
            }
            "/sharephone" if self.auth.phone_available() && !self.auth.linked(&chat_name)? => {
                self.request_phone(chat, lang, None).await
            }
            "/sharephone" => self.show(chat, Action::new("home"), None, None).await,
            "/unlink" => self.show(chat, Action::new("sys.unlink"), None, None).await,
            "/cancel" => {
                self.auth.cancel(&chat_name)?;
                self.clear(
                    chat,
                    &[
                        "generation",
                        "prompt",
                        "phone",
                        "phone-input",
                        "agent-pending",
                        "agent-context",
                    ],
                )?;
                let text = lang.text(
                    "Cancelled. Use /menu to open the menu.",
                    "تم الإلغاء. استخدم /menu لفتح القائمة.",
                );
                if pending.is_some() {
                    self.remove_keyboard(chat, text).await
                } else {
                    self.say(chat, text).await
                }
            }
            "/help" => {
                let commands = lang.text(
                    "/menu opens the menu, /linkaccount links through your browser, /sharephone shares your phone, /cancel cancels, /language changes language, /unlink disconnects.",
                    "/menu القائمة، /linkaccount ربط الحساب عبر المتصفح، /sharephone مشاركة رقم الهاتف، /cancel إلغاء، /language اللغة، /unlink إلغاء الربط.",
                );
                let help = if self.agent.is_some() {
                    lang.text(
                        "Try asking “Show my attendance”. Type your question directly and leave out personal information.",
                        "جرّب «أرني سجل حضوري». اكتب سؤالك مباشرة وتجنّب المعلومات الشخصية.",
                    )
                } else {
                    lang.text(
                        "Type a question directly for help. Administrators may review assistant questions and replies for three days; leave out personal information.",
                        "اكتب سؤالك مباشرة للمساعدة. يمكن لمسؤولي وارتق مراجعة أسئلتك للمساعد وردوده لمدة ثلاثة أيام. تجنّب إرسال المعلومات الشخصية في أسئلتك.",
                    )
                };
                self.say(chat, format!("{commands} {help}")).await
            }
            _ if text.starts_with('/') => {
                self.say(
                    chat,
                    lang.text("Use /menu to open the menu.", "استخدم /menu لفتح القائمة."),
                )
                .await
            }
            _ => {
                if let Some(prompt) = self.auth.get::<Prompt>(&chat.key("prompt"))? {
                    let current: Option<String> = self.auth.get(&chat.key("generation"))?;
                    if prompt.user == chat.user
                        && current.as_deref() == Some(prompt.generation.as_str())
                    {
                        self.clear(chat, &["generation", "prompt"])?;
                        self.invalidate_assistant(chat)?;
                        return self.show(chat, prompt.action, Some(text), None).await;
                    }
                }
                self.ask(chat, lang, &message, text).await
            }
        }
    }

    async fn ask(
        &self,
        chat: PrivateChat,
        lang: Language,
        message: &Message,
        text: &str,
    ) -> Result<()> {
        if message.forward_origin().is_some()
            || text.is_empty()
            || text.encode_utf16().count() > 1000
            || crate::assistant::sensitive(text)
        {
            return self
                .say(
                    chat,
                    lang.text(
                        "Send a short question. Leave out personal information.",
                        "أرسل سؤالك مباشرة دون معلومات شخصية.",
                    ),
                )
                .await;
        }
        if self.auth.pending(&chat.name())? || self.auth.exchanging(&chat.name())? {
            return self
                .say(
                    chat,
                    lang.text(
                        "Finish signing in or use /cancel, then send your question.",
                        "أكمل تسجيل الدخول أو استخدم /cancel، ثم أرسل سؤالك.",
                    ),
                )
                .await;
        }
        let Some(active) = self.active_questions.claim(chat.id.0)? else {
            return self
                .say(
                    chat,
                    lang.text(
                        "The assistant is busy. Try again shortly.",
                        "المساعد مشغول الآن. حاول بعد قليل.",
                    ),
                )
                .await;
        };
        let epoch = nonce::<18>()?;
        let key = chat.key("assistant-epoch");
        self.clear(chat, &["agent-generation"])?;
        self.auth
            .store
            .set(&key, &json!(epoch), now()? + PHONE_TTL)?;
        let worker = self.clone();
        let question = text.to_owned();
        tokio::spawn(async move {
            let _active = active;
            if let Some(agent) = &worker.agent {
                let screen = agent
                    .respond(&worker.auth, &chat.name(), &question, lang, &epoch)
                    .await;
                if worker.auth.get::<String>(&key).ok().flatten().as_deref() != Some(epoch.as_str())
                {
                    return;
                }
                if let Ok(screen) = screen {
                    if worker.deliver(chat, screen, None).await.is_err() {
                        let _ = worker.invalidate_assistant(chat);
                    }
                } else {
                    let _ = worker
                        .say(
                            chat,
                            lang.text(
                                "The assistant is unavailable. Use /menu to open Wartaq.",
                                "المساعد غير متاح الآن. استخدم /menu لفتح وارتق.",
                            ),
                        )
                        .await;
                }
                return;
            }
            if let Ok(turn) = worker.assistant.request(&question, lang, "telegram").await {
                if worker.auth.get::<String>(&key).ok().flatten().as_deref() != Some(epoch.as_str())
                {
                    return;
                }
                let reply = turn.answer.as_deref().unwrap_or_else(|| {
                    lang.text(
                        "The assistant is unavailable. Try later or ask your teacher.",
                        "المساعد غير متاح حالياً. حاول لاحقاً أو اسأل معلّمك.",
                    )
                });
                let sent = worker.say(chat, reply).await.is_ok();
                turn.delivered(&worker.auth, &chat.name(), &question, reply, sent)
                    .await;
            }
        });
        Ok(())
    }

    async fn assistant_callback(&self, chat: PrivateChat, action: AssistantAction) -> Result<()> {
        let lang = self.language(chat)?;
        let Some(agent) = &self.agent else {
            return self
                .say(
                    chat,
                    lang.text(
                        "This choice has expired. Use /menu.",
                        "انتهت صلاحية هذا الخيار. استخدم /menu.",
                    ),
                )
                .await;
        };
        let Some(active) = self.active_questions.claim(chat.id.0)? else {
            return self
                .say(
                    chat,
                    lang.text(
                        "The assistant is busy. Try again shortly.",
                        "المساعد مشغول الآن. حاول بعد قليل.",
                    ),
                )
                .await;
        };
        let agent = agent.clone();
        let worker = self.clone();
        tokio::spawn(async move {
            let _active = active;
            let chat_name = chat.name();
            let result = match action {
                AssistantAction::Bound(action) if action.route == "agent_confirm" => {
                    agent.confirm(&worker.auth, &chat_name, &action, lang).await
                }
                AssistantAction::Bound(action) => {
                    agent.cancel(&worker.auth, &chat_name, &action, lang).await
                }
                AssistantAction::Legacy { data, message } => {
                    agent
                        .legacy_callback(
                            &worker.auth,
                            &chat_name,
                            chat.user,
                            message.0,
                            &data,
                            lang,
                        )
                        .await
                }
            };
            if let Ok(screen) = result {
                let _ = worker.deliver(chat, screen, None).await;
            } else {
                let _ = worker.say(chat, lang.text(
                    "The request could not be completed. Check the result before trying again, or use /menu.",
                    "تعذر إكمال الطلب. تحقق من النتيجة قبل المحاولة مجدداً، أو استخدم /menu."
                )).await;
            }
        });
        Ok(())
    }

    async fn phone_issue(
        &self,
        chat: PrivateChat,
        lang: Language,
        en: &str,
        ar: &str,
    ) -> Result<()> {
        self.say(chat, format!("{}\n\n{}", lang.text(en, ar), lang.text(
            "You can also use /linkaccount. Sign in and approve in your browser, then paste the eight-digit code here.",
            "يمكنك أيضاً استخدام /linkaccount. سجّل الدخول ووافق في المتصفح، ثم أرسل الرمز من ثمانية أرقام هنا."
        ))).await
    }

    async fn dismiss_phone(
        &self,
        chat: PrivateChat,
        lang: Language,
        has_keyboard: bool,
    ) -> Result<()> {
        self.clear(chat, &["phone", "phone-input"])?;
        if has_keyboard {
            let _ = self
                .remove_keyboard(
                    chat,
                    lang.text("Phone sharing closed.", "تم إغلاق مشاركة الرقم."),
                )
                .await;
        }
        Ok(())
    }

    async fn contact(
        &self,
        chat: PrivateChat,
        lang: Language,
        message: &Message,
        contact: &teloxide_core::types::Contact,
        pending: Option<PhonePrompt>,
    ) -> Result<()> {
        let chat_name = chat.name();
        let Some(prompt) = pending else {
            if !self.auth.linked(&chat_name)? {
                self.phone_issue(
                    chat,
                    lang,
                    "The phone sharing request expired. Use /sharephone to try again.",
                    "انتهت صلاحية طلب مشاركة الرقم. استخدم /sharephone للمحاولة مجدداً.",
                )
                .await?;
            }
            return Ok(());
        };
        if self.auth.linked(&chat_name)? {
            return Ok(());
        }
        if prompt.user != chat.user
            || contact.user_id.map(|user| user.0) != Some(chat.user)
            || message.forward_origin().is_some()
            || message.reply_to_message().map(|reply| reply.id.0) != Some(prompt.message)
        {
            self.phone_issue(
                chat,
                lang,
                "Share your own contact using the button below so Telegram can confirm it.",
                "شارك رقمك من الزر أدناه حتى يتمكن تيليجرام من تأكيده.",
            )
            .await?;
            return self.request_phone(chat, lang, prompt.expected).await;
        }
        self.clear(chat, &["phone", "phone-input"])?;
        let phone = normalize_phone(&contact.phone_number, true);
        let Some(phone) = phone.filter(|phone| {
            prompt
                .expected
                .as_ref()
                .is_none_or(|expect| expect == phone)
        }) else {
            return self.remove_keyboard(chat, lang.text(
                "The shared number could not be confirmed. Check the digits and use /menu to try again.",
                "تعذر تأكيد الرقم المُشارك. تحقق من الأرقام واستخدم /menu للمحاولة مجدداً.",
            )).await;
        };
        let _ = self
            .remove_keyboard(chat, lang.text("Signing in…", "جارٍ تسجيل الدخول…"))
            .await;
        let role = match self.auth.login_phone(&chat_name, &phone).await {
            Ok(role) => role,
            Err(error)
                if error
                    .downcast_ref::<RequestFailure>()
                    .is_some_and(|error| error.0 == StatusCode::FORBIDDEN) =>
            {
                return self.phone_issue(chat, lang,
                    "Students, check that your Telegram number matches the phone number on your roster. Teachers, use your verified Wartaq sign-in number.",
                    "للطلاب، تأكّدوا من أن رقم تيليجرام يطابق الرقم المسجل في قائمة الطلاب. للمعلمين، استخدموا رقم تسجيل الدخول المؤكد في وارتق.",
                ).await;
            }
            Err(_) => {
                return self
                    .phone_issue(
                        chat,
                        lang,
                        "Phone sign-in is temporarily unavailable. Try /sharephone again.",
                        "تعذر تسجيل الدخول بالهاتف مؤقتاً. حاول مجدداً باستخدام /sharephone.",
                    )
                    .await;
            }
        };
        self.show(
            chat,
            Action::new(match role {
                PhoneRole::Student => "nb.mine",
                PhoneRole::Teacher => "home",
            }),
            None,
            None,
        )
        .await
    }

    async fn notify_failure(&self, update: Update) {
        let chat = match update.kind {
            UpdateKind::Message(message) => message
                .from
                .as_ref()
                .and_then(|user| PrivateChat::new(&message.chat, user.id)),
            UpdateKind::CallbackQuery(callback) => callback
                .message
                .as_ref()
                .and_then(|message| PrivateChat::new(message.chat(), callback.from.id)),
            _ => None,
        };
        if let Some(chat) = chat {
            let _ = self.say(chat, self.language(chat).unwrap_or(Language::En).text(
                "The request could not be completed. Check the result before trying again, or use /menu.",
                "تعذر إكمال الطلب. تحقق من النتيجة قبل المحاولة مجدداً، أو استخدم /menu.",
            )).await;
        }
    }
}

fn is_agent_action(route: &str) -> bool {
    matches!(route, "agent_confirm" | "agent_cancel")
}

pub(crate) fn normalize_digits(text: &str) -> String {
    text.chars()
        .map(|char| match char {
            '\u{660}'..='\u{669}' => char::from(b'0' + (char as u32 - 0x660) as u8),
            '\u{6f0}'..='\u{6f9}' => char::from(b'0' + (char as u32 - 0x6f0) as u8),
            _ => char,
        })
        .collect()
}

pub(crate) fn normalize_phone(text: &str, contact: bool) -> Option<String> {
    let text = text.trim();
    if text.chars().count() > 40 {
        return None;
    }
    let mut compact: String = normalize_digits(text)
        .chars()
        .filter(|ch| !" ()-".contains(*ch))
        .collect();
    if let Some(number) = compact.strip_prefix("00") {
        compact = format!("+{number}");
    } else if compact.starts_with("09") && compact.len() == 10 {
        compact = format!("+963{}", &compact[1..]);
    } else if contact
        && text
            .as_bytes()
            .first()
            .is_some_and(|b| matches!(b, b'1'..=b'9'))
    {
        compact.insert(0, '+');
    }
    let number = compact.strip_prefix('+')?;
    ((7..=15).contains(&number.len())
        && !number.starts_with('0')
        && number.bytes().all(|byte| byte.is_ascii_digit()))
    .then_some(compact)
}

#[derive(Clone)]
struct HttpState {
    auth: Arc<Auth>,
    ready: Arc<AtomicBool>,
}

async fn health(State(state): State<HttpState>) -> (StatusCode, &'static str) {
    if state.ready.load(Ordering::Relaxed) {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    }
}

#[derive(Deserialize)]
struct Callback {
    state: String,
    code: String,
}

async fn callback(
    method: Method,
    State(state): State<HttpState>,
    query: std::result::Result<Query<Callback>, axum::extract::rejection::QueryRejection>,
) -> Response {
    if method != Method::GET {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let no_store = [
        (header::CACHE_CONTROL, "no-store"),
        (header::REFERRER_POLICY, "no-referrer"),
    ];
    if let Ok(Query(query)) = query {
        if let Ok(code) = state.auth.callback(&query.state, &query.code).await {
            let hash = |start, end| {
                let (_, contents) = LINK_PAGE
                    .split_once(start)
                    .expect("link page template start");
                let (contents, _) = contents.split_once(end).expect("link page template end");
                STANDARD.encode(digest::digest(&digest::SHA256, contents.as_bytes()))
            };
            let policy = format!(
                "default-src 'none'; style-src 'sha256-{}'; script-src 'sha256-{}'; frame-ancestors 'none'",
                hash("<style>", "</style>"),
                hash("<script>", "</script>")
            );
            return (
                no_store,
                [
                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                    (header::CONTENT_SECURITY_POLICY, &policy),
                ],
                LINK_PAGE.replace("WARTAQ_CONFIRMATION_CODE", &code),
            )
                .into_response();
        }
    }
    (
        StatusCode::BAD_REQUEST,
        no_store,
        "Link expired or failed. Start again in your private chat.",
    )
        .into_response()
}

async fn shutdown() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
