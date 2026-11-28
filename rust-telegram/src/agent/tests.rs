use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use base64::{Engine, engine::general_purpose::STANDARD};

use crate::storage::Storage;

use super::*;

const CHAT: &str = "101";
const WORKER_EPOCH: &str = "synthetic-worker";
const COURSE: &str = "10000000-0000-4000-8000-000000000001";
const STUDENT: &str = "10000000-0000-4000-8000-000000000002";
const ENROLLMENT: &str = "10000000-0000-4000-8000-000000000003";
const OLD_EVENT: &str = "10000000-0000-4000-8000-000000000004";
const DATE: &str = "2026-11-23";

#[derive(Clone, Copy, Default)]
enum ServerOutcome {
    #[default]
    Saved,
    Conflict,
    Lost,
}

#[derive(Default)]
struct Fake {
    requests: Vec<(String, Value)>,
    writes: Vec<Value>,
    denied: bool,
    outcome: ServerOutcome,
}

fn catalog() -> Value {
    serde_json::from_str(include_str!("../../../rust-sync/src/mcp_tool_catalog.json")).unwrap()
}

fn turn(agent: &Agent, auth: &Auth, question: &str) -> impl Future<Output = Result<Screen>> {
    agent.respond(auth, CHAT, question, Language::En, WORKER_EPOCH)
}

fn selected(name: &str, args: Value) -> Json<Value> {
    Json(json!({"choices":[{"message":{
        "role":"assistant","tool_calls":[{
            "id":"call-synthetic","type":"function","function":{"name":name,"arguments":args.to_string()}
        }]
    }}]}))
}

async fn fake_model(State(fake): State<Arc<Mutex<Fake>>>, Json(request): Json<Value>) -> Response {
    let mut fake = fake.lock().unwrap();
    let messages = request["messages"].as_array().unwrap();
    let last = messages.last().unwrap();
    fake.requests.push(("model".into(), request.clone()));
    let tools = request["tools"].as_array().unwrap();
    assert!(
        !tools
            .iter()
            .any(|tool| tool["function"]["name"] == "reset_account_password")
    );
    assert!(!request.to_string().contains("private@example.invalid"));
    assert!(!request.to_string().contains("PRIVATE_NOTE"));
    if last["role"] == "user" {
        if last["content"]
            .as_str()
            .is_some_and(|text| text.starts_with("طيب"))
        {
            return selected(
                "get_course_attendance_view",
                json!({"courseId": COURSE, "date": "2026-11-20", "view":"session"}),
            )
            .into_response();
        }
        return selected("list_courses", json!({})).into_response();
    }
    let data: Value = serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
    if data.is_array() {
        if messages.iter().any(|message| {
            message["content"] == "Download the Demo Recitation course report as XLSX"
        }) {
            return selected(
                "export_course_report",
                json!({"courseId": COURSE, "format":"xlsx"}),
            )
            .into_response();
        }
        return selected(
            "get_course_attendance_view",
            json!({"courseId": COURSE, "date": "2026-11-20", "view":"session"}),
        )
        .into_response();
    }
    Json(json!({"choices":[{"message":{"role":"assistant","content":"Incorrect model prose. Demo Lina was present."}}]})).into_response()
}

async fn fake_mcp(State(fake): State<Arc<Mutex<Fake>>>, Json(request): Json<Value>) -> Response {
    let mut fake = fake.lock().unwrap();
    let args = &request["params"]["arguments"];
    let name = request["params"]["name"].as_str().unwrap_or("catalog");
    fake.requests.push((name.into(), args.clone()));
    let result = if request["method"] == "tools/list" {
        let catalog = catalog();
        let mut names = vec![
            "list_courses",
            "get_course_attendance_view",
            "export_course_report",
            "reset_account_password",
        ];
        if !fake.denied {
            names.push("sync_attendance");
        }
        json!({"tools": names.iter().map(|name| &catalog[*name]).collect::<Vec<_>>()})
    } else {
        let content = match name {
            "list_courses" => {
                json!([{"id":COURSE,"name":"Demo Recitation","open":true,"email":"private@example.invalid"}])
            }
            "get_course_attendance_view" => json!({
                "course":{"id":COURSE,"name":"Demo Recitation","open":true},
                "date":args["date"],
                "session":[{"studentId":STUDENT,"enrollmentId":ENROLLMENT,"eventId":OLD_EVENT,"name":"Demo Lina","status":"absent","note":"PRIVATE_NOTE"}],
            }),
            "export_course_report" => {
                json!({"blob": STANDARD.encode(b"PK\x03\x04synthetic-workbook")})
            }
            "sync_attendance" => {
                fake.writes.push(args.clone());
                let records = args["records"].as_array().unwrap();
                assert_eq!(records.len(), 1);
                assert_eq!(records[0]["expectedEventId"], OLD_EVENT);
                assert!(schema::uuid(records[0]["event"]["id"].as_str().unwrap()));
                match fake.outcome {
                    ServerOutcome::Lost => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
                    ServerOutcome::Conflict => {
                        json!({"outcomes":[{"id": records[0]["event"]["id"], "kind":"conflict", "current":null}]})
                    }
                    ServerOutcome::Saved => {
                        json!({"outcomes":[{"id": records[0]["event"]["id"], "kind":"saved", "current":null}]})
                    }
                }
            }
            _ => panic!("unexpected synthetic tool"),
        };
        json!({"structuredContent":content})
    };
    Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result})).into_response()
}

async fn fake_student(
    State(fake): State<Arc<Mutex<Fake>>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    assert_eq!(request["action"], "attendance");
    assert_eq!(request["token"], "synthetic-student-grant");
    fake.lock()
        .unwrap()
        .requests
        .push(("student".into(), Value::Null));
    Json(
        json!({"student":{"name":"Demo Lina"},"attendance":[{"course":"Demo Recitation","date":DATE,"status":"excused"}]}),
    )
}

async fn server() -> Result<(Auth, Agent, Arc<Mutex<Fake>>, tokio::task::JoinHandle<()>)> {
    let fake = Arc::new(Mutex::new(Fake::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin: url::Url = format!("http://{}", listener.local_addr()?).parse()?;
    let app = Router::new()
        .route("/api/telegram/teacher-mcp", post(fake_mcp))
        .route("/api/telegram/student", post(fake_student))
        .route("/v1/chat/completions", post(fake_model))
        .with_state(fake.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let auth = Auth::new(
        Arc::new(Storage::open(":memory:", &STANDARD.encode([38; 32]))?),
        origin.clone(),
        origin.clone(),
        Some("synthetic-worker-secret".into()),
    )?;
    auth.store
        .set("teacher:101", &json!("synthetic-teacher-grant"), 0)?;
    auth.store
        .set("assistant-epoch:101", &json!(WORKER_EPOCH), 0)?;
    let agent = Agent {
        model: model::Model::configured(origin.as_str(), "synthetic".into(), "synthetic".into())?,
    };
    assert!(agent.model.is_some());
    Ok((auth, agent, fake, task))
}

fn approval(screen: &Screen, position: usize) -> &Action {
    match &screen.rows[0][position] {
        Button::Choice(_, action) => action,
        _ => panic!("expected approval choice"),
    }
}

#[tokio::test]
async fn model_reads_current_scoped_records_and_uses_retained_context() -> Result<()> {
    let (auth, agent, fake, server) = server().await?;
    let first = turn(
        &agent,
        &auth,
        "Who was absent from Demo Recitation on 2026-11-23?",
    )
    .await?;
    assert!(first.text.contains(DATE));
    assert!(first.text.contains("Demo Lina: Absent"));
    assert!(!first.text.contains("Incorrect model prose"));
    let second = turn(&agent, &auth, "طيب ويوم ٢٠٢٦-١١-٢٤؟").await?;
    assert!(second.text.contains("2026-11-24"));
    assert!(second.text.contains("Demo Lina: غائب"));
    let calls = &fake.lock().unwrap().requests;
    assert_eq!(
        calls
            .iter()
            .filter(|(name, _)| name == "get_course_attendance_view")
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(name, _)| name == "list_courses")
            .count(),
        1
    );
    assert!(
        calls
            .iter()
            .filter(|(name, _)| name == "model")
            .any(|(_, body)| {
                body["messages"].as_array().unwrap().iter().any(|message| {
                    message["content"].as_str().is_some_and(|text| {
                        text.contains("Earlier questions") && text.contains("Who was absent")
                    })
                })
            })
    );
    server.abort();
    Ok(())
}

#[tokio::test]
async fn attendance_approval_survives_reload_and_consumes_exact_draft_once() -> Result<()> {
    let (auth, agent, fake, server) = server().await?;
    let request = "Mark Demo Lina present in Demo Recitation on 2026-11-23.";
    let review = turn(&agent, &auth, request).await?;
    assert!(
        review.text.contains("Demo Lina")
            && review.text.contains(DATE)
            && review.text.contains("Present")
    );
    let pending: Pending = auth.get(&key("pending", CHAT))?.unwrap();
    assert!(fake.lock().unwrap().writes.is_empty());
    let handler = Agent::without_model();
    let other = handler
        .confirm(&auth, "102", approval(&review, 0), Language::En)
        .await?;
    assert!(other.text.contains("expired"));
    let saved = handler
        .confirm(&auth, CHAT, approval(&review, 0), Language::En)
        .await?;
    assert!(saved.text.contains("Request submitted"), "{}", saved.text);
    let replay = handler
        .confirm(&auth, CHAT, approval(&review, 0), Language::En)
        .await?;
    assert!(replay.text.contains("expired"));
    assert_eq!(
        fake.lock().unwrap().writes.as_slice(),
        &[pending.draft.args]
    );

    let review = turn(&agent, &auth, request).await?;
    fake.lock().unwrap().outcome = ServerOutcome::Conflict;
    let result = handler
        .confirm(&auth, CHAT, approval(&review, 0), Language::En)
        .await?;
    assert!(result.text.contains("Attendance changed"));

    let review = turn(&agent, &auth, request).await?;
    fake.lock().unwrap().outcome = ServerOutcome::Lost;
    let result = handler
        .confirm(&auth, CHAT, approval(&review, 0), Language::En)
        .await?;
    let replay = handler
        .confirm(&auth, CHAT, approval(&review, 0), Language::En)
        .await?;
    assert_eq!(result.text, replay.text);
    assert!(replay.text.contains("outcome is unknown"));

    let review = turn(&agent, &auth, request).await?;
    let cancelled = handler
        .cancel(&auth, CHAT, approval(&review, 1), Language::En)
        .await?;
    assert!(cancelled.text.contains("Cancelled"));
    let replay = handler
        .confirm(&auth, CHAT, approval(&review, 0), Language::En)
        .await?;
    assert!(replay.text.contains("expired"));

    let review = turn(
        &agent,
        &auth,
        "سجل Demo Lina حاضرة في Demo Recitation بتاريخ 2026-11-23.",
    )
    .await?;
    assert!(review.text.contains("حاضر"));
    fake.lock().unwrap().denied = true;
    let denied = handler
        .confirm(&auth, CHAT, approval(&review, 0), Language::En)
        .await?;
    assert!(denied.text.contains("متاح"));
    assert_eq!(fake.lock().unwrap().writes.len(), 3);
    server.abort();
    Ok(())
}

#[tokio::test]
async fn student_tool_scope_and_report_delivery_keep_documents_and_grants_private() -> Result<()> {
    let (auth, agent, fake, server) = server().await?;
    let report = turn(
        &agent,
        &auth,
        "Download the Demo Recitation course report as XLSX",
    )
    .await?;
    let (name, bytes) = report.document.unwrap();
    assert_eq!(name, "course-report.xlsx");
    assert_eq!(bytes, b"PK\x03\x04synthetic-workbook");
    assert!(!fake.lock().unwrap().requests.iter().any(|(name, args)| {
        name == "model" && args.to_string().contains("synthetic-workbook")
    }));

    fake.lock().unwrap().requests.clear();
    auth.store
        .set("student:101", &json!("synthetic-student-grant"), 0)?;
    let show = turn(&Agent::without_model(), &auth, "Show my attendance").await?;
    assert!(show.text.contains("Excused: 1"));
    assert_eq!(
        fake.lock()
            .unwrap()
            .requests
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["student"]
    );
    server.abort();
    Ok(())
}

#[tokio::test]
async fn legacy_approvals_bind_the_original_message_and_keep_uncertain_receipts() -> Result<()> {
    let (auth, agent, fake, server) = server().await?;
    let old_handle = "abcdefghijklmnopqrstuvwx";
    let callback = format!("agent:c:{old_handle}");
    let original = tools::prepare(
        Name::SyncAttendance,
        json!({"records":[{
            "event":{"payload":{"kind":"attendance","studentId":STUDENT,"enrollmentId":ENROLLMENT,"date":DATE,"status":"present"}},
            "expectedEventId":OLD_EVENT
        }]}),
    )?;
    let grant = fingerprint(&auth.current_access(CHAT)?.unwrap());
    auth.store.set(
        &legacy_key("epoch", CHAT),
        &json!("legacy-epoch"),
        now()? + CONTEXT_TTL,
    )?;
    auth.store.set(&legacy_key("pending", CHAT), &json!({
        "handle":old_handle,"chat":CHAT,"user":101,"message":7,"grant":grant,"epoch":"legacy-epoch",
        "tool":"sync_attendance","args":original,"lang":"en"
    }), now()? + APPROVAL_TTL)?;
    auth.store.set(&legacy_key("context", CHAT), &json!({
        "grant":grant,"context":{"turns":[{"question":"Legacy question about Demo Recitation"}],"labels":{COURSE:"Demo Recitation"}}
    }), now()? + CONTEXT_TTL)?;
    assert_eq!(load_context(&auth, CHAT, &grant)?.turns.len(), 1);
    assert!(load_context(&auth, CHAT, "other")?.turns.is_empty());
    let wrong = agent
        .legacy_callback(&auth, CHAT, 101, 8, &callback, Language::En)
        .await?;
    assert!(wrong.text.contains("expired"));
    assert!(fake.lock().unwrap().writes.is_empty());
    fake.lock().unwrap().outcome = ServerOutcome::Lost;
    let uncertain = agent
        .legacy_callback(&auth, CHAT, 101, 7, &callback, Language::En)
        .await?;
    let replay = agent
        .legacy_callback(&auth, CHAT, 101, 7, &callback, Language::En)
        .await?;
    assert!(uncertain.text.contains("outcome is unknown"));
    assert_eq!(replay.text, uncertain.text);
    assert_eq!(fake.lock().unwrap().writes.as_slice(), &[original]);
    server.abort();
    Ok(())
}

#[test]
fn live_schemas_validate_versions_and_fail_closed_on_unlisted_fields() -> Result<()> {
    let catalog = catalog();
    for (name, definition) in catalog.as_object().unwrap() {
        if serde_json::from_value::<Name>(json!(name)).is_ok() {
            let tool = Allowed::from_tool(serde_json::from_value(definition.clone())?);
            assert!(
                tool.is_some(),
                "{name} schema does not fit the native agent"
            );
        }
    }
    let attendance =
        Allowed::from_tool(serde_json::from_value(catalog["sync_attendance"].clone())?).unwrap();
    let args = tools::prepare(
        Name::SyncAttendance,
        json!({"records":[{
            "event":{"payload":{"kind":"attendance","studentId":STUDENT,"enrollmentId":ENROLLMENT,"date":DATE,"status":"present"}},
            "expectedEventId":null
        }]}),
    )?;
    attendance.validate(&args)?;
    let mut unversioned = args.clone();
    unversioned["records"][0]
        .as_object_mut()
        .unwrap()
        .remove("expectedEventId");
    assert!(attendance.validate(&unversioned).is_err());
    let mut inject = args.clone();
    inject["records"][0]["role"] = json!("administrator");
    assert!(attendance.validate(&inject).is_err());
    assert!(!has_private_input("Show records for ٢٠٢٦-١١-٢٣"));
    assert!(has_private_input("password: 2026-11-23"));
    assert!(has_private_input("+2026-11-23"));
    assert!(has_private_input("041 111 1111"));
    assert!(has_private_input("student@example.invalid"));
    Ok(())
}
