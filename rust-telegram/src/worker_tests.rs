use axum::{
    Json,
    extract::Path,
    routing::{get, post},
};
use serde_json::Value;
use std::{collections::VecDeque, sync::Mutex};

use super::*;

#[derive(Default)]
struct Fixtures {
    updates: Mutex<VecDeque<Value>>,
    calls: Mutex<Vec<(String, Value)>>,
    origin: Mutex<String>,
    mark: Mutex<Option<Value>>,
    course_name: Mutex<Option<String>>,
    fail_poll: AtomicBool,
    agent_catalog: AtomicBool,
}

const SECRET: &str = "synthetic-secret-that-is-over-32-characters";
const STUDENT: &str = "88888888-8888-4888-8888-888888888888";
const COURSE: &str = "99999999-9999-4999-8999-999999999999";
const ENROLLMENT: &str = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";

impl Fixtures {
    fn record(&self, method: &str, body: Value) {
        self.calls.lock().unwrap().push((method.into(), body));
    }
    fn count(&self, method: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _)| key == method)
            .count()
    }
    fn last(&self, method: &str) -> Value {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(key, _)| key == method)
            .unwrap()
            .1
            .clone()
    }
    fn update(&self, value: Value) {
        self.updates.lock().unwrap().push_back(value);
    }
}

async fn telegram(
    Path(method): Path<String>,
    State(mock): State<Arc<Fixtures>>,
    Json(body): Json<Value>,
) -> Response {
    mock.record(&method, body.clone());
    let result = match method.as_str() {
        "GetUpdates" => {
            if mock.fail_poll.load(Ordering::Relaxed) {
                return (StatusCode::BAD_GATEWAY, Json(json!({"ok": false, "error_code": 502, "description": "synthetic polling failure"}))).into_response();
            }
            json!(mock.updates.lock().unwrap().drain(..).collect::<Vec<_>>())
        }
        "AnswerCallbackQuery" => json!(true),
        "SendMessage" | "EditMessageText" => json!({
            "message_id": body["message_id"].as_i64().unwrap_or(mock.count("SendMessage") as i64),
            "date": 1, "chat": {"id": body["chat_id"], "type": "private"}, "text": body["text"],
        }),
        _ => panic!("unexpected Telegram method {method}"),
    };
    Json(json!({"ok": true, "result": result})).into_response()
}

async fn student(
    State(mock): State<Arc<Fixtures>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    assert_eq!(headers["authorization"], format!("Bearer {SECRET}"));
    let action = body["action"].as_str().unwrap();
    mock.record(action, body.clone());
    let value = match action {
        "login-phone" => {
            assert_eq!(body["phone"], "+963912345678");
            json!({"token": "synthetic-student", "role": "student"})
        }
        "attendance" => {
            assert_eq!(body["token"], "synthetic-student");
            json!({
                "student": {"studentId": STUDENT, "name": "Synthetic Student", "group": ""},
                "attendance": [{"date": "2026-11-23", "course":"Synthetic Course", "status":"excused"}],
            })
        }
        "unlink" => json!({"ok": true}),
        _ => panic!("unexpected phone request"),
    };
    Json(value)
}

async fn mcp(
    State(mock): State<Arc<Fixtures>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    let teacher = headers.contains_key("x-telegram-grant");
    if teacher {
        assert_eq!(headers["authorization"], format!("Bearer {SECRET}"));
        assert_eq!(headers["x-telegram-grant"], "synthetic-teacher");
    } else {
        assert_eq!(headers["authorization"], "Bearer synthetic-access");
    }
    let result = match body["method"].as_str().unwrap() {
        "tools/list" => {
            let mut names = vec!["get_account", "get_notebook", "sync_attendance"];
            if !teacher {
                names.extend(["get_admin_board", "save_course"]);
            }
            if mock.agent_catalog.load(Ordering::Relaxed) {
                names.extend(["list_courses", "get_course_attendance_view"]);
                let catalog: Value =
                    serde_json::from_str(include_str!("../../rust-sync/src/mcp_tool_catalog.json"))
                        .unwrap();
                json!({"tools": names.iter().map(|name| &catalog[*name]).collect::<Vec<_>>()})
            } else {
                json!({ "tools": names.into_iter().map(|name| json!({"name": name})).collect::<Vec<_>>() })
            }
        }
        "tools/call" => {
            let name = body["params"]["name"].as_str().unwrap();
            let args = &body["params"]["arguments"];
            mock.record(name, args.clone());
            let result = match name {
                "get_account" => json!({"account":{"name":"Synthetic Teacher"}}),
                "get_notebook" => json!({
                    "courses":[{"id":COURSE,"name":mock.course_name.lock().unwrap().as_deref().unwrap_or("Synthetic Course"),"open":true}],
                    "students":[{"studentId":STUDENT,"name":"Synthetic Student","active":true,"group":""}],
                    "enrollments":[{"id":ENROLLMENT,"studentId":STUDENT,"courseId":COURSE,"active":true}],
                    "events": mock.mark.lock().unwrap().iter().collect::<Vec<_>>(),
                }),
                "save_course" => {
                    assert!(!teacher);
                    assert_eq!(args["input"]["id"], COURSE);
                    *mock.course_name.lock().unwrap() =
                        Some(args["input"]["name"].as_str().unwrap().to_owned());
                    json!({"ok": true})
                }
                "list_courses" => json!([{"id":COURSE,"name":"Synthetic Course","open":true}]),
                "get_course_attendance_view" => {
                    assert_eq!(args["courseId"], COURSE);
                    let mark = mock.mark.lock().unwrap();
                    json!({
                        "course":{"id":COURSE,"name":"Synthetic Course","open":true},
                        "date":args["date"], "session":[{
                            "studentId":STUDENT,"enrollmentId":ENROLLMENT,"name":"Synthetic Student",
                            "eventId":mark.as_ref().map_or(Value::Null, |entry| entry["id"].clone()),
                            "status":mark.as_ref().map_or("unmarked", |entry| entry["payload"]["status"].as_str().unwrap()),
                        }],
                    })
                }
                "sync_attendance" => {
                    let record = &args["records"][0];
                    assert_eq!(record["event"]["payload"]["studentId"], STUDENT);
                    assert_eq!(record["event"]["payload"]["enrollmentId"], ENROLLMENT);
                    assert_eq!(record["event"]["payload"]["status"], "present");
                    let mut mark = mock.mark.lock().unwrap();
                    let mut kind = "conflict";
                    if record["expectedEventId"]
                        == mark
                            .as_ref()
                            .map_or(Value::Null, |entry| entry["id"].clone())
                    {
                        *mark = Some(record["event"].clone());
                        kind = "saved";
                    }
                    json!({"outcomes":[{"id": record["event"]["id"],"kind":kind}]})
                }
                _ => panic!("unexpected MCP call {name}"),
            };
            json!({"structuredContent": result})
        }
        _ => panic!("unexpected MCP method"),
    };
    Json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
}

fn private(id: u64) -> Value {
    json!({"id":id,"type":"private"})
}

fn sender(id: u64) -> Value {
    json!({"id":id,"is_bot":false,"first_name":"Synthetic"})
}

fn message(update: i32, who: u64, text: &str) -> Value {
    json!({"update_id":update,"message":{"message_id":update,"date":1,"chat":private(who),"from":sender(who),"text":text}})
}

fn contact(update: i32, who: u64, owner: u64, reply: Value) -> Value {
    json!({"update_id":update,"message":{"message_id":update,"date":1,"chat":private(who),"from":sender(who),
        "contact":{"first_name":"Synthetic","user_id":owner,"phone_number":"963912345678"},
        "reply_to_message":{"message_id":reply,"date":1,"chat":private(who)}}})
}

fn press(update: i32, who: u64, sent: i32, handle: &str) -> Value {
    json!({"update_id":update,"callback_query":{"id":format!("callback-{update}"),"from":sender(who),"chat_instance":"synthetic","data":handle,
        "message":{"message_id":sent,"date":1,"chat":private(who)}}})
}

fn handle(body: Value, label: &str) -> String {
    body["reply_markup"]["inline_keyboard"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row.as_array().unwrap())
        .find(|button| button["text"] == label)
        .unwrap()["callback_data"]
        .as_str()
        .unwrap()
        .into()
}

async fn poll(worker: &Worker, poller: &mut Poller<'_>) -> Result<()> {
    poller
        .poll_once(
            &mut |_| true,
            &mut |update| worker.handle(update),
            &mut |update| worker.notify_failure(update),
        )
        .await
}

#[tokio::test]
async fn poller_enforces_phone_ownership_and_attendance_replay() -> Result<()> {
    let mock = Arc::new(Fixtures::default());
    let server = Router::new()
        .route("/bottest/{method}", post(telegram))
        .route("/api/telegram/student", post(student))
        .route("/api/telegram/teacher-mcp", post(mcp));
    let (url, server_task) = spawn(server.with_state(mock.clone())).await?;
    let worker = test_worker(&url, &url)?;
    let mut poller = Poller::new(&worker.telegram, &worker.auth.store);
    let (http, http_task) = spawn(worker.router(poller.readiness())).await?;
    let client = reqwest::Client::new();
    assert_eq!(
        client.get(http.join("/health")?).send().await?.status(),
        503
    );
    let mut malformed = message(1, 8, "/sharephone");
    malformed["update_id"] = json!("invalid");
    mock.update(malformed);
    mock.update(message(1, 1, "/sharephone"));
    poll(&worker, &mut poller).await?;
    assert_eq!(
        client.get(http.join("/health")?).send().await?.status(),
        200
    );
    let first = mock.count("SendMessage");
    assert_eq!(first, 1);
    assert_eq!(worker.auth.store.get("offset")?, Some(json!(2)));
    assert!(worker.auth.store.get("phone:8")?.is_none());
    assert_eq!(
        mock.last("SendMessage")["reply_markup"]["keyboard"][0][0]["request_contact"],
        true
    );
    mock.update(contact(2, 1, 2, json!(first)));
    poll(&worker, &mut poller).await?;
    assert_eq!(mock.count("login-phone"), 0);
    let second = mock.count("SendMessage");
    mock.update(contact(3, 1, 1, json!(second)));
    poll(&worker, &mut poller).await?;
    assert_eq!(mock.count("login-phone"), 1);
    assert_eq!(mock.count("attendance"), 1);
    let sent = mock.last("SendMessage");
    assert_eq!(sent["protect_content"], true);
    assert!(
        sent["text"]
            .as_str()
            .unwrap()
            .contains("2026-11-23 · Synthetic Course · excused")
    );
    assert_eq!(worker.auth.store.get("offset")?, Some(json!(4)));
    let mut group = message(4, 1, "/menu");
    group["message"]["chat"]["type"] = json!("group");
    mock.update(group);
    poll(&worker, &mut poller).await?;
    assert_eq!(mock.count("attendance"), 1);
    worker
        .auth
        .store
        .set("teacher:9", &json!("synthetic-teacher"), 0)?;
    mock.update(message(5, 9, "/menu"));
    poll(&worker, &mut poller).await?;
    let menu_id = mock.count("SendMessage") as i32;
    let attendance = handle(mock.last("SendMessage"), "Take attendance");
    mock.update(press(6, 9, menu_id, &attendance));
    poll(&worker, &mut poller).await?;
    let course = handle(mock.last("EditMessageText"), "Synthetic Course");
    mock.update(press(7, 9, menu_id, &course));
    poll(&worker, &mut poller).await?;
    let mark = handle(mock.last("EditMessageText"), "Present");
    mock.update(press(8, 9, menu_id + 1, &mark));
    poll(&worker, &mut poller).await?;
    assert_eq!(mock.count("sync_attendance"), 0);
    mock.update(press(9, 9, menu_id, &mark));
    mock.update(press(10, 9, menu_id, &mark));
    mock.update(press(9, 9, menu_id, &mark));
    poll(&worker, &mut poller).await?;
    assert_eq!(mock.count("sync_attendance"), 1);
    assert!(
        mock.last("EditMessageText")["text"]
            .as_str()
            .unwrap()
            .contains("Attendance saved: Synthetic Student · Present")
    );
    assert_eq!(worker.auth.store.get("offset")?, Some(json!(11)));
    mock.fail_poll.store(true, Ordering::Relaxed);
    assert!(poll(&worker, &mut poller).await.is_err());
    assert_eq!(
        client.get(http.join("/health")?).send().await?.status(),
        503
    );
    server_task.abort();
    http_task.abort();
    Ok(())
}

async fn provider(State(mock): State<Arc<Fixtures>>) -> Json<Value> {
    let origin = mock.origin.lock().unwrap().clone();
    Json(
        json!({"issuer": format!("{origin}/api/auth"), "authorization_endpoint":format!("{origin}/api/auth/authorize"),
        "token_endpoint":format!("{origin}/api/auth/token"), "registration_endpoint":format!("{origin}/api/auth/register")}),
    )
}

#[tokio::test]
async fn browser_callback_checks_pkce_and_binds_code_to_originating_chat() -> Result<()> {
    let mock = Arc::new(Fixtures::default());
    let server = Router::new().route("/bottest/{method}", post(telegram))
        .route("/mcp", post(mcp))
        .route("/.well-known/oauth-protected-resource/mcp", get(|State(mock): State<Arc<Fixtures>>| async move {
            let origin = mock.origin.lock().unwrap().clone();
            Json(json!({"resource":format!("{origin}/mcp"), "authorization_servers":[format!("{origin}/api/auth")]}))
        }))
        .route("/.well-known/oauth-authorization-server/api/auth", get(provider))
        .route("/api/auth/register", post(|State(mock): State<Arc<Fixtures>>, Json(body): Json<Value>| async move {
            mock.record("register", body);
            Json(json!({"client_id":"synthetic-client"}))
        }))
        .route("/api/auth/token", post(|State(mock): State<Arc<Fixtures>>, axum::Form(form): axum::Form<HashMap<String,String>>| async move {
            assert_eq!(form["code"], "synthetic-browser-code");
            assert_eq!(form["client_id"], "synthetic-client");
            assert_eq!(form["grant_type"], "authorization_code");
            mock.record("token", json!(form));
            Json(json!({"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":3600}))
        }));
    let (url, server_task) = spawn(server.with_state(mock.clone())).await?;
    *mock.origin.lock().unwrap() = url.origin().ascii_serialization();
    let worker = test_worker(&url, &url)?;
    let mut poller = Poller::new(&worker.telegram, &worker.auth.store);
    let (http, http_task) = spawn(worker.router(poller.readiness())).await?;
    mock.update(message(1, 1, "/linkaccount"));
    mock.update(message(2, 1, "/linkaccount"));
    poll(&worker, &mut poller).await?;
    assert_eq!(mock.count("register"), 1);
    let login: Url = mock.last("SendMessage")["reply_markup"]["inline_keyboard"][0][0]["url"]
        .as_str()
        .unwrap()
        .parse()?;
    let params: HashMap<_, _> = login.query_pairs().into_owned().collect();
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["resource"], url.join("/mcp")?.as_str());
    let mut callback = http.join("/callback")?;
    callback
        .query_pairs_mut()
        .append_pair("state", &params["state"])
        .append_pair("code", "synthetic-browser-code");
    let client = reqwest::Client::new();
    let response = client.get(callback.clone()).send().await?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()?
            .contains("frame-ancestors 'none'")
    );
    let markup = response.text().await?;
    let code = markup
        .split_once("<strong>")
        .unwrap()
        .1
        .split_once("</strong>")
        .unwrap()
        .0;
    assert_eq!(code.len(), 8);
    let verifier = mock.last("token")["code_verifier"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        params["code_challenge"],
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(digest::digest(&digest::SHA256, verifier.as_bytes()))
    );
    assert_eq!(client.get(callback).send().await?.status(), 400);
    assert_eq!(mock.count("token"), 1);
    mock.update(message(3, 2, code));
    poll(&worker, &mut poller).await?;
    assert!(worker.auth.store.get("credentials:2")?.is_none());
    mock.update(message(4, 1, code));
    poll(&worker, &mut poller).await?;
    assert!(worker.auth.store.get("credentials:1")?.is_some());
    assert_eq!(mock.count("get_account"), 1);
    assert!(
        mock.last("SendMessage")["text"]
            .as_str()
            .unwrap()
            .starts_with("Welcome, Synthetic Teacher")
    );
    server_task.abort();
    http_task.abort();
    Ok(())
}

#[tokio::test]
async fn course_rename_requires_current_record_and_single_confirmation() -> Result<()> {
    let mock = Arc::new(Fixtures::default());
    let server = Router::new()
        .route("/bottest/{method}", post(telegram))
        .route("/mcp", post(mcp));
    let (url, task) = spawn(server.with_state(mock.clone())).await?;
    let worker = test_worker(&url, &url)?;
    worker.auth.store.set(
        "credentials:7",
        &json!({
            "access_token": "synthetic-access", "refresh_token": "synthetic-refresh",
            "client": "synthetic-client", "expires": now()? + 3600_000,
        }),
        0,
    )?;
    let mut poller = Poller::new(&worker.telegram, &worker.auth.store);
    let mut updates = 0;
    for (name, rename, concurrent) in [
        (
            "Synthetic Course",
            "First Proposed Name",
            Some("Newer Course Name"),
        ),
        ("Newer Course Name", "Confirmed Course Name", None),
    ] {
        updates += 1;
        mock.update(message(updates, 7, "/menu"));
        poll(&worker, &mut poller).await?;
        let id = mock.count("SendMessage") as i32;
        let courses = handle(mock.last("SendMessage"), "Courses");
        updates += 1;
        mock.update(press(updates, 7, id, &courses));
        poll(&worker, &mut poller).await?;
        let course = handle(mock.last("EditMessageText"), name);
        updates += 1;
        mock.update(press(updates, 7, id, &course));
        poll(&worker, &mut poller).await?;
        let rename_button = handle(mock.last("EditMessageText"), "Rename");
        updates += 1;
        mock.update(press(updates, 7, id, &rename_button));
        poll(&worker, &mut poller).await?;
        updates += 1;
        mock.update(message(updates, 7, rename));
        poll(&worker, &mut poller).await?;
        let confirm_id = mock.count("SendMessage") as i32;
        let proposal = mock.last("SendMessage");
        assert!(proposal["text"].as_str().unwrap().contains(rename));
        assert_eq!(mock.count("save_course"), 0);
        let confirm = handle(proposal, "Confirm");
        if let Some(concurrent) = concurrent {
            *mock.course_name.lock().unwrap() = Some(concurrent.into());
        }
        updates += 1;
        mock.update(press(updates, 7, confirm_id, &confirm));
        poll(&worker, &mut poller).await?;
        if concurrent.is_some() {
            assert_eq!(mock.count("save_course"), 0);
            assert!(
                mock.last("EditMessageText")["text"]
                    .as_str()
                    .unwrap()
                    .contains("The record changed")
            );
        } else {
            assert_eq!(mock.count("save_course"), 1);
            assert_eq!(mock.course_name.lock().unwrap().as_deref(), Some(rename));
        }
        updates += 1;
        mock.update(press(updates, 7, confirm_id, &confirm));
        poll(&worker, &mut poller).await?;
        assert_eq!(mock.count("save_course"), usize::from(concurrent.is_none()));
    }
    task.abort();
    Ok(())
}

#[tokio::test]
async fn refreshed_grant_cannot_be_replayed_or_restore_unlinked_account() -> Result<()> {
    use tokio::sync::Notify;
    let mock = Arc::new(Fixtures::default());
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let token = {
        let started = started.clone();
        let release = release.clone();
        |State(mock): State<Arc<Fixtures>>,
         axum::Form(form): axum::Form<HashMap<String, String>>| async move {
            assert_eq!(form["grant_type"], "refresh_token");
            assert_eq!(form["refresh_token"], "synthetic-refresh");
            mock.record("refresh", json!(form));
            started.notify_one();
            release.notified().await;
            Json(
                json!({"access_token":"synthetic-rotated", "refresh_token":"synthetic-next", "expires_in":3600}),
            )
        }
    };
    let server = Router::new()
        .route("/.well-known/oauth-protected-resource/mcp", get(|State(mock): State<Arc<Fixtures>>| async move {
            let origin = mock.origin.lock().unwrap().clone();
            Json(json!({"resource":format!("{origin}/mcp"), "authorization_servers":[format!("{origin}/api/auth")]}))
        }))
        .route("/.well-known/oauth-authorization-server/api/auth", get(provider))
        .route("/api/auth/token", post(token));
    let (url, task) = spawn(server.with_state(mock.clone())).await?;
    *mock.origin.lock().unwrap() = url.origin().ascii_serialization();
    let worker = test_worker(&url, &url)?;
    worker.auth.store.set(
        "credentials:1",
        &json!({
            "access_token": "synthetic-access", "refresh_token": "synthetic-refresh",
            "client": "synthetic-client", "expires": now()? - 1,
        }),
        0,
    )?;
    let auth = worker.auth.clone();
    let request = tokio::spawn(async move { auth.access("1").await });
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified()).await?;
    assert!(worker.auth.access("1").await?.is_none());
    assert_eq!(mock.count("refresh"), 1);
    worker.auth.unlink("1").await?;
    release.notify_one();
    assert!(request.await??.is_none());
    assert!(worker.auth.store.get("credentials:1")?.is_none());
    assert!(worker.auth.store.get("oauth-refresh:1")?.is_none());
    task.abort();
    Ok(())
}

#[tokio::test]
async fn help_mode_discloses_capture_and_redacts_transcripts_even_without_model() -> Result<()> {
    let mock = Arc::new(Fixtures::default());
    let server = Router::new()
        .route("/bottest/{method}", post(telegram))
        .route(
            "/api/telegram/conversations",
            post(
                |State(mock): State<Arc<Fixtures>>,
                 headers: axum::http::HeaderMap,
                 Json(body): Json<Value>| async move {
                    assert_eq!(headers["authorization"], format!("Bearer {SECRET}"));
                    mock.record("transcript", body);
                    StatusCode::CREATED
                },
            ),
        );
    let (url, server_task) = spawn(server.with_state(mock.clone())).await?;
    let worker = test_worker(&url, &url)?;
    let mut poller = Poller::new(&worker.telegram, &worker.auth.store);
    mock.update(message(1, 1, "/help"));
    poll(&worker, &mut poller).await?;
    assert!(
        mock.last("SendMessage")["text"]
            .as_str()
            .unwrap()
            .contains("for three days")
    );
    assert_eq!(mock.count("transcript"), 0);
    let mut forwarded = message(2, 1, "How do I sign in?");
    forwarded["message"]["forward_date"] = json!(1);
    mock.update(forwarded);
    poll(&worker, &mut poller).await?;
    assert!(
        mock.last("SendMessage")["text"]
            .as_str()
            .unwrap()
            .contains("Leave out personal information.")
    );
    assert_eq!(mock.count("transcript"), 0);
    mock.update(message(3, 1, "Will q@synthetic.example help me sign in?"));
    poll(&worker, &mut poller).await?;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while mock.count("transcript") == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    let transcript = mock.last("transcript");
    assert_eq!(transcript["outcome"], "unconfigured");
    assert_eq!(transcript["delivery"], "sent");
    assert_eq!(transcript["question"], "Will [redacted] help me sign in?");
    assert_eq!(transcript["conversationId"].as_str().unwrap().len(), 64);
    assert!(
        transcript["reply"]
            .as_str()
            .unwrap()
            .contains("The assistant is unavailable.")
    );
    server_task.abort();
    Ok(())
}

#[tokio::test]
async fn worker_binds_detached_agent_approval_to_message_and_mutates_once() -> Result<()> {
    let mock = Arc::new(Fixtures::default());
    mock.agent_catalog.store(true, Ordering::Relaxed);
    let server = Router::new()
        .route("/bottest/{method}", post(telegram))
        .route("/api/telegram/teacher-mcp", post(mcp));
    let (url, task) = spawn(server.with_state(mock.clone())).await?;
    let mut worker = test_worker(&url, &url)?;
    worker.agent = Some(Arc::new(Agent::without_model()));
    worker
        .auth
        .store
        .set("teacher:9", &json!("synthetic-teacher"), 0)?;
    let mut poller = Poller::new(&worker.telegram, &worker.auth.store);
    mock.update(message(
        1,
        9,
        "Mark Synthetic Student present in Synthetic Course on 2026-11-23.",
    ));
    poll(&worker, &mut poller).await?;
    wait_for_calls(&mock, "SendMessage", 1).await?;
    let sent = mock.last("SendMessage");
    assert!(sent["text"].as_str().unwrap().contains("Synthetic Student"));
    let confirm = handle(sent, "Confirm");
    assert_eq!(mock.count("sync_attendance"), 0);
    mock.update(press(2, 9, 2, &confirm));
    poll(&worker, &mut poller).await?;
    assert_eq!(mock.count("sync_attendance"), 0);
    mock.update(press(3, 9, 1, &confirm));
    poll(&worker, &mut poller).await?;
    wait_for_calls(&mock, "SendMessage", 3).await?;
    assert_eq!(mock.count("sync_attendance"), 1);
    assert_eq!(
        mock.mark.lock().unwrap().as_ref().unwrap()["payload"]["date"],
        "2026-11-23"
    );
    mock.update(press(4, 9, 1, &confirm));
    poll(&worker, &mut poller).await?;
    wait_for_calls(&mock, "SendMessage", 4).await?;
    assert_eq!(mock.count("sync_attendance"), 1);
    task.abort();
    Ok(())
}

async fn wait_for_calls(mock: &Fixtures, method: &str, count: usize) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while mock.count(method) < count {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}

fn test_worker(telegram: &Url, origin: &Url) -> Result<Worker> {
    let auth = Auth::new(
        Arc::new(Storage::open(":memory:", &STANDARD.encode([71; 32]))?),
        origin.clone(),
        origin.clone(),
        Some(SECRET.into()),
    )?;
    Ok(Worker {
        telegram: Telegram::new("test", telegram.clone())?,
        auth: Arc::new(auth),
        assistant: Arc::new(HelpModel::unconfigured()?),
        agent: None,
        active_questions: Arc::default(),
    })
}

async fn spawn(router: Router) -> Result<(Url, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}/", listener.local_addr()?).parse()?;
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Ok((origin, server))
}
