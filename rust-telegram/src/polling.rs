use std::{
    future::Future,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use teloxide_core::types::{Update, UpdateKind};

use crate::{Telegram, storage::Storage};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const RECEIPT_TTL_MS: i64 = 6 * 24 * 60 * 60 * 1_000;

/// A poller persists each accepted update before invoking its handler.
/// Run only one poller against a given Telegram bot and state database.
pub struct Poller<'a> {
    telegram: &'a Telegram,
    storage: &'a Storage,
    ready: bool,
    last_prune: Option<Instant>,
}

impl<'a> Poller<'a> {
    pub fn new(telegram: &'a Telegram, storage: &'a Storage) -> Self {
        Self {
            telegram,
            storage,
            ready: false,
            last_prune: None,
        }
    }

    pub fn ready(&self) -> bool {
        self.ready
    }

    /// Processes a single provider batch. A validator checks the worker's input contract
    /// before the durable receipt is written. Failed handlers invoke `notify_failure`.
    pub async fn poll_once<V, H, F, N, NF>(
        &mut self,
        validate: &mut V,
        handle: &mut H,
        notify_failure: &mut N,
    ) -> Result<()>
    where
        V: FnMut(&Update) -> bool,
        H: FnMut(Update) -> F,
        F: Future<Output = Result<()>>,
        N: FnMut(Update) -> NF,
        NF: Future<Output = ()>,
    {
        if self
            .last_prune
            .is_none_or(|last| last.elapsed() > Duration::from_secs(300))
        {
            self.storage.prune()?;
            self.last_prune = Some(Instant::now());
        }
        let offset = read_offset(self.storage)?;
        let updates = self.telegram.updates(offset, self.ready).await?;
        self.ready = true;
        for update in updates {
            if matches!(update.kind, UpdateKind::Error(_)) || !validate(&update) {
                notify_failure(update).await;
                continue;
            }
            if i64::from(update.id.0) < i64::from(read_offset(self.storage)?) {
                continue;
            }
            // Teloxide accepts i32 offsets. Refuse to acknowledge an unrepresentable
            // offset so a later poll cannot silently discard the update.
            let next = i32::try_from(u64::from(update.id.0) + 1)?;
            let now: i64 = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .try_into()?;
            if self
                .storage
                .set("offset", &json!(next), now + RECEIPT_TTL_MS)
                .is_err()
            {
                notify_failure(update).await;
                continue;
            }
            if handle(update.clone()).await.is_err() {
                notify_failure(update).await;
            }
        }
        Ok(())
    }

    /// Retries provider or state failures after the worker's existing three-second delay.
    pub async fn run<V, H, F, N, NF>(
        &mut self,
        mut validate: V,
        mut handle: H,
        mut notify_failure: N,
    ) -> !
    where
        V: FnMut(&Update) -> bool,
        H: FnMut(Update) -> F,
        F: Future<Output = Result<()>>,
        N: FnMut(Update) -> NF,
        NF: Future<Output = ()>,
    {
        loop {
            if self
                .poll_once(&mut validate, &mut handle, &mut notify_failure)
                .await
                .is_err()
            {
                self.ready = false;
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

fn read_offset(storage: &Storage) -> Result<i32> {
    match storage.get("offset")? {
        None => Ok(0),
        Some(value) => Ok(value.as_i64().ok_or("invalid stored offset")?.try_into()?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn receipt_survives_failure_restart_and_duplicate_delivery() -> Result<()> {
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let app = Router::new().route("/bottest/GetUpdates", post(
            |State(requests): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                requests.lock().unwrap().push(body);
                Json(json!({"ok": true, "result": [{"update_id": 42, "message": {"message_id": 7, "date": 1, "chat": {"id": 2, "type": "private"}, "text": "hello"}}]}))
            }
        )).with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let telegram = Telegram::new("test", format!("http://{address}/").parse()?)?;
        let path = std::env::temp_dir().join(format!(
            "wartaq-poll-{}-{}.sqlite",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        let key = STANDARD.encode([37; 32]);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let mut validate = |_: &Update| true;
        let mut handler = {
            let calls = calls.clone();
            move |update: Update| {
                calls.lock().unwrap().push(update.id.0);
                async { Err("synthetic handler failure".into()) }
            }
        };
        let mut notify = {
            let failures = failures.clone();
            move |update: Update| {
                failures.lock().unwrap().push(update.id.0);
                async {}
            }
        };
        {
            let storage = Storage::open(&path, &key)?;
            let mut poller = Poller::new(&telegram, &storage);
            poller
                .poll_once(&mut validate, &mut handler, &mut notify)
                .await?;
            assert!(poller.ready());
            assert_eq!(storage.get("offset")?, Some(json!(43)));
        }
        {
            let storage = Storage::open(&path, &key)?;
            let mut restarted = Poller::new(&telegram, &storage);
            restarted
                .poll_once(&mut validate, &mut handler, &mut notify)
                .await?;
        }
        assert_eq!(*calls.lock().unwrap(), vec![42]);
        assert_eq!(*failures.lock().unwrap(), vec![42]);
        let sent = requests.lock().unwrap();
        assert_eq!(sent[0]["offset"], 0);
        assert_eq!(sent[1]["offset"], 43);
        assert_eq!(sent[1]["timeout"], 1);
        task.abort();
        std::fs::remove_file(path)?;
        Ok(())
    }
}
