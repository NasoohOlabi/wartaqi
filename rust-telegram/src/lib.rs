pub mod polling;
pub mod storage;

use std::time::Duration;

use teloxide_core::{
    Bot, RequestError,
    payloads::{
        EditMessageTextSetters, GetUpdatesSetters, SendDocumentSetters, SendMessageSetters,
    },
    requests::{Request, Requester},
    types::{
        AllowedUpdate, InputFile, LinkPreviewOptions, MessageId, Recipient, ReplyMarkup, Update,
    },
};

/// Telegram transport for the worker. The caller controls update processing and durable offsets.
#[derive(Clone)]
pub struct Telegram {
    api: Bot,
    polling: Bot,
}

impl Telegram {
    pub fn new(token: &str, api_url: url::Url) -> Result<Self, reqwest::Error> {
        let client = |timeout| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(timeout))
                .redirect(reqwest::redirect::Policy::none())
                .build()
        };
        Ok(Self {
            api: Bot::with_client(token, client(15)?).set_api_url(api_url.clone()),
            polling: Bot::with_client(token, client(40)?).set_api_url(api_url),
        })
    }

    pub async fn updates(&self, offset: i32, ready: bool) -> Result<Vec<Update>, RequestError> {
        self.polling
            .get_updates()
            .offset(offset)
            .timeout(if ready { 30 } else { 1 })
            .allowed_updates([AllowedUpdate::Message, AllowedUpdate::CallbackQuery])
            .send()
            .await
    }

    pub async fn send_message(
        &self,
        chat: impl Into<Recipient>,
        text: impl Into<String>,
        markup: Option<ReplyMarkup>,
    ) -> Result<teloxide_core::types::Message, RequestError> {
        let mut request = self
            .api
            .send_message(chat, text)
            .protect_content(true)
            .link_preview_options(disabled_preview());
        if let Some(markup) = markup {
            request = request.reply_markup(markup);
        }
        request.send().await
    }

    pub async fn edit_message(
        &self,
        chat: impl Into<Recipient>,
        message: MessageId,
        text: impl Into<String>,
        markup: Option<teloxide_core::types::InlineKeyboardMarkup>,
    ) -> Result<teloxide_core::types::Message, RequestError> {
        let mut request = self
            .api
            .edit_message_text(chat, message, text)
            .link_preview_options(disabled_preview());
        if let Some(markup) = markup {
            request = request.reply_markup(markup);
        }
        request.send().await
    }

    pub async fn answer_callback(
        &self,
        id: impl Into<String>,
    ) -> Result<teloxide_core::types::True, RequestError> {
        self.api
            .answer_callback_query(teloxide_core::types::CallbackQueryId(id.into()))
            .send()
            .await
    }

    pub async fn send_document(
        &self,
        chat: impl Into<Recipient>,
        name: String,
        bytes: Vec<u8>,
    ) -> Result<teloxide_core::types::Message, RequestError> {
        self.api
            .send_document(chat, InputFile::memory(bytes).file_name(name))
            .protect_content(true)
            .send()
            .await
    }
}

fn disabled_preview() -> LinkPreviewOptions {
    LinkPreviewOptions {
        is_disabled: true,
        url: None,
        prefer_small_media: false,
        prefer_large_media: false,
        show_above_text: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use serde_json::{Value, json};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn polling_uses_existing_offset_timeout_and_update_types() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let app = Router::new()
            .route(
                "/bottest/GetUpdates",
                post(
                    |State(sender): State<mpsc::UnboundedSender<Value>>,
                     Json(body): Json<Value>| async move {
                        sender.send(body).unwrap();
                        Json(json!({"ok": true, "result": []}))
                    },
                ),
            )
            .route(
                "/bottest/SendMessage",
                post(
                    |State(sender): State<mpsc::UnboundedSender<Value>>,
                     Json(body): Json<Value>| async move {
                        sender.send(body).unwrap();
                        Json(json!({"ok": true, "result": {"message_id": 7, "date": 1, "chat": {"id": 2, "type": "private"}, "text": "hello"}}))
                    },
                ),
            )
            .with_state(sender);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let telegram =
            Telegram::new("test", format!("http://{address}/").parse().unwrap()).unwrap();
        assert!(telegram.updates(42, false).await.unwrap().is_empty());
        assert_eq!(
            receiver.recv().await.unwrap(),
            json!({"offset": 42, "timeout": 1, "allowed_updates": ["message", "callback_query"]})
        );
        assert!(telegram.updates(43, true).await.unwrap().is_empty());
        assert_eq!(
            receiver.recv().await.unwrap(),
            json!({"offset": 43, "timeout": 30, "allowed_updates": ["message", "callback_query"]})
        );
        telegram
            .send_message(teloxide_core::types::ChatId(2), "hello", None)
            .await
            .unwrap();
        assert_eq!(
            receiver.recv().await.unwrap(),
            json!({"chat_id": 2, "text": "hello", "protect_content": true, "link_preview_options": {"is_disabled": true}})
        );
        task.abort();
    }
}
