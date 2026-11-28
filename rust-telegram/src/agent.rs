use std::{collections::HashSet, sync::Mutex, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::Instant;
use unicode_normalization::UnicodeNormalization;

use crate::{
    auth::{Access, Auth, Result, hex_digest, nonce},
    mcp::Mcp,
    screens::{Button, Screen},
    storage::now,
    worker::{Action, Language, normalize_digits},
};

mod attendance;
mod copy;
mod model;
mod schema;
mod tools;

#[cfg(test)]
mod tests;

use tools::{Allowed, Labels, Name};

const CONTEXT_TTL: i64 = 30 * 60_000;
const APPROVAL_TTL: i64 = 10 * 60_000;
const TURN_LIMIT: Duration = Duration::from_secs(30);
static STATE: Mutex<()> = Mutex::new(());
const WRITE_INTENT: &str = r"(?i)\b(mark|record|add|create|save|change|edit|update|remove|delete|enroll|correct|set)\b|سجّل|سجل|حط|أضف|اضف|أنشئ|انشئ|عدل|عدّل|صحح|غيّر|غير|احذف";

pub(crate) struct Agent {
    model: Option<model::Model>,
}

#[derive(Default, Serialize, Deserialize)]
struct Context {
    turns: Vec<String>,
    labels: Labels,
}

#[derive(Serialize, Deserialize)]
struct BoundContext {
    grant: String,
    context: Context,
}

#[derive(Deserialize)]
struct LegacyBoundContext {
    grant: String,
    context: LegacyContext,
}

#[derive(Deserialize)]
struct LegacyContext {
    turns: Vec<LegacyTurn>,
    labels: Labels,
}

#[derive(Deserialize)]
struct LegacyTurn {
    question: String,
}

#[derive(Serialize, Deserialize)]
struct Draft {
    name: Name,
    args: Value,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    handle: String,
    grant: String,
    epoch: String,
    draft: Draft,
    lang: Language,
}

#[derive(Deserialize, Serialize)]
struct LegacyReceipt {
    handle: String,
    user: u64,
    message: i32,
}

#[derive(Deserialize)]
struct LegacyPending {
    #[serde(flatten)]
    receipt: LegacyReceipt,
    chat: String,
    grant: String,
    epoch: String,
    tool: Name,
    args: Value,
    lang: Language,
}

#[derive(Clone, Copy)]
enum ApprovalBinding {
    Native,
    Legacy { user: u64, message: i32 },
}

impl ApprovalBinding {
    fn key(self, kind: &str, chat: &str) -> String {
        match self {
            Self::Native => key(kind, chat),
            Self::Legacy { .. } => legacy_key(kind, chat),
        }
    }

    fn receipt(self, handle: &str) -> Value {
        match self {
            Self::Native => json!(handle),
            Self::Legacy { user, message } => {
                json!({"handle": handle, "user": user, "message": message})
            }
        }
    }
}

enum Reply {
    Answer(String),
    Approval { text: String, draft: Draft },
    Report(Vec<u8>),
}

struct Turn<'a> {
    auth: &'a Auth,
    access: &'a Access,
    chat: &'a str,
    grant: &'a str,
    epoch: &'a str,
    called: HashSet<Name>,
    deadline: Instant,
}

impl Turn<'_> {
    fn current(&self) -> Result<()> {
        current(self.auth, self.chat, self.epoch, self.grant)
    }

    fn mcp(&self) -> Mcp<'_> {
        Mcp::new(&self.auth.http, &self.auth.origin, self.access)
    }

    async fn catalog(&self) -> Result<Vec<Allowed>> {
        self.current()?;
        if matches!(self.access, Access::Student(_)) {
            return Ok(vec![Allowed::student()]);
        }
        let catalog = tokio::time::timeout_at(self.deadline, self.mcp().catalog()).await??;
        self.current()?;
        Ok(catalog.into_iter().filter_map(Allowed::from_tool).collect())
    }

    async fn read(&mut self, tool: &Allowed, args: Value) -> Result<Value> {
        if tool.name.write() {
            return Err("agent read required".into());
        }
        self.current()?;
        tool.validate(&args)?;
        let result = match self.access {
            Access::Student(token) if tool.name == Name::GetMyAttendance => {
                tokio::time::timeout_at(
                    self.deadline,
                    self.auth
                        .phone(json!({"action":"attendance","token":token})),
                )
                .await??
            }
            Access::Student(_) => return Err("agent student tool denied".into()),
            _ => {
                tokio::time::timeout_at(self.deadline, self.mcp().call(tool.name.wire(), args))
                    .await??
            }
        };
        self.current()?;
        self.called.insert(tool.name);
        Ok(result)
    }

    async fn recheck(&self, draft: Option<Name>) -> Result<()> {
        if (!self.called.is_empty() || draft.is_some())
            && !matches!(self.access, Access::Student(_))
        {
            let fresh = self.catalog().await?;
            if !self
                .called
                .iter()
                .copied()
                .chain(draft)
                .all(|name| fresh.iter().any(|tool| tool.name == name))
            {
                return Err("agent role changed".into());
            }
        }
        self.current()
    }
}

impl Agent {
    pub(crate) fn from_env() -> Result<Self> {
        Ok(Self {
            model: model::Model::from_env()?,
        })
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn without_model() -> Self {
        Self { model: None }
    }

    pub(crate) fn invalidate(auth: &Auth, chat: &str) -> Result<()> {
        let _guard = STATE.lock().map_err(|_| "agent state lock failed")?;
        invalidate(auth, chat)
    }

    pub(crate) async fn respond(
        &self,
        auth: &Auth,
        chat: &str,
        question: &str,
        lang: Language,
        worker_epoch: &str,
    ) -> Result<Screen> {
        let question = question.trim();
        let lang = detect_language(question, lang);
        let epoch = nonce::<18>()?;
        {
            let _guard = STATE.lock().map_err(|_| "agent state lock failed")?;
            if auth
                .get::<String>(&format!("assistant-epoch:{chat}"))?
                .as_deref()
                != Some(worker_epoch)
            {
                return Err("agent turn cancelled".into());
            }
            auth.store.replace(
                &[
                    &key("pending", chat),
                    &legacy_key("pending", chat),
                    &legacy_key("epoch", chat),
                ],
                &[(&key("epoch", chat), &json!(epoch), now()? + CONTEXT_TTL)],
            )?;
        }
        if question.is_empty()
            || question.encode_utf16().count() > 1000
            || has_private_input(question)
        {
            diagnostic("rejected");
            return Ok(screen(invalid(lang)));
        }
        if !matches(
            r"(?i)\bhow (?:many|much|often)\b|\bcan (?:i|you) see\b|قديش|كم (?:مرة|عدد)",
            question,
        ) && (!matches(WRITE_INTENT, question)
            || matches(r"(?i)\b(how|steps|guide)\b|كيف|شلون", question))
            && let Some(text) = copy::canned(question, lang)
        {
            return Ok(screen(text));
        }
        if matches(
            r"(?i)\b(password|passcode|sign.in phone|verification code)\b|كلمة (?:السر|المرور)|رقم الدخول|رمز التحقق",
            question,
        ) {
            return Ok(screen(lang.text(
                "Open /menu → My account → Profile for account changes. Ask an administrator for help with access.",
                "افتح /menu ثم حسابي ثم الملف الشخصي لتعديل الحساب. اسأل المسؤول للمساعدة في صلاحية الدخول.",
            )));
        }
        let access = match auth.current_access(chat) {
            Ok(Some(access)) => access,
            Ok(None) => return Ok(screen(denied(lang))),
            Err(_) => return Ok(screen(unavailable(lang))),
        };
        let grant = fingerprint(&access);
        let deadline = Instant::now() + TURN_LIMIT;
        let mut turn = Turn {
            auth,
            access: &access,
            chat,
            grant: &grant,
            epoch: &epoch,
            called: HashSet::new(),
            deadline,
        };
        let catalog = match turn.catalog().await {
            Ok(catalog) if catalog.is_empty() => return Ok(screen(denied(lang))),
            Ok(catalog) => catalog,
            Err(_) => return Ok(screen(unavailable(lang))),
        };
        let mut context = load_context(auth, chat, &grant)?;
        let available = shortlist(question, &context, &catalog);
        // Leave time for a final scope check when inference exhausts its per-turn budget.
        turn.deadline = deadline - Duration::from_secs(2);
        let reply = if let Some(reply) = attendance::plan(question, lang, &catalog, &mut turn).await
        {
            reply
        } else if let Some(model) = &self.model
            && let Some(reply) = model
                .answer(question, lang, &mut context, &available, &mut turn)
                .await
        {
            reply
        } else {
            attendance::fallback(question, lang, &catalog, &mut turn).await
        };
        turn.deadline = deadline;
        let name = match &reply {
            Reply::Approval { draft, .. } => Some(draft.name),
            _ => None,
        };
        if turn.recheck(name).await.is_err() {
            diagnostic("scope_changed");
            return Ok(screen(denied(lang)));
        }
        let _guard = STATE.lock().map_err(|_| "agent state lock failed")?;
        turn.current()?;
        context.turns.push(question.into());
        if context.turns.len() > 4 {
            context.turns.drain(..context.turns.len() - 4);
        }
        let context = serde_json::to_value(BoundContext {
            grant: grant.clone(),
            context,
        })?;
        let context_key = key("context", chat);
        let expiry = now()?;
        match reply {
            Reply::Answer(text) => {
                auth.store
                    .set(&context_key, &context, expiry + CONTEXT_TTL)?;
                Ok(screen(text))
            }
            Reply::Report(bytes) => {
                auth.store
                    .set(&context_key, &context, expiry + CONTEXT_TTL)?;
                let mut answer = screen(lang.text("Course report", "تقرير الدورة"));
                answer.document = Some(("course-report.xlsx".into(), bytes));
                Ok(answer)
            }
            Reply::Approval { text, draft } => {
                let handle = nonce::<18>()?;
                let pending = serde_json::to_value(Pending {
                    handle: handle.clone(),
                    grant,
                    epoch,
                    draft,
                    lang,
                })?;
                auth.store.replace(
                    &[],
                    &[
                        (&context_key, &context, expiry + CONTEXT_TTL),
                        (&key("pending", chat), &pending, expiry + APPROVAL_TTL),
                    ],
                )?;
                diagnostic("proposed");
                let mut answer = screen(text);
                answer.rows = vec![vec![
                    Button::Choice(
                        lang.text("Confirm", "تأكيد").into(),
                        Action::new("agent_confirm").arg("approval", handle.clone()),
                    ),
                    Button::Choice(
                        lang.text("Cancel", "إلغاء").into(),
                        Action::new("agent_cancel").arg("approval", handle),
                    ),
                ]];
                Ok(answer)
            }
        }
    }

    pub(crate) async fn cancel(
        &self,
        auth: &Auth,
        chat: &str,
        action: &Action,
        lang: Language,
    ) -> Result<Screen> {
        let _guard = STATE.lock().map_err(|_| "agent state lock failed")?;
        let Some(pending) = matching_pending(auth, chat, action)? else {
            return receipt_reply(
                auth,
                chat,
                action.value("approval"),
                lang,
                ApprovalBinding::Native,
            );
        };
        invalidate(auth, chat)?;
        diagnostic("cancelled");
        Ok(screen(pending.lang.text("Cancelled.", "تم الإلغاء.")))
    }

    pub(crate) async fn confirm(
        &self,
        auth: &Auth,
        chat: &str,
        action: &Action,
        lang: Language,
    ) -> Result<Screen> {
        let binding = ApprovalBinding::Native;
        let pending = {
            let _guard = STATE.lock().map_err(|_| "agent state lock failed")?;
            let Some(pending) = matching_pending(auth, chat, action)? else {
                return receipt_reply(auth, chat, action.value("approval"), lang, binding);
            };
            auth.store.delete(&key("pending", chat))?;
            pending
        };
        self.submit(auth, chat, pending, binding).await
    }

    pub(crate) async fn legacy_callback(
        &self,
        auth: &Auth,
        chat: &str,
        user: u64,
        message: i32,
        data: &str,
        lang: Language,
    ) -> Result<Screen> {
        let Some((decision, handle)) = data
            .strip_prefix("agent:")
            .and_then(|data| data.split_once(':'))
        else {
            return Ok(screen(expired(lang)));
        };
        if !matches!(decision, "c" | "x")
            || !matches(r"^[a-zA-Z0-9_-]{24}$", handle)
            || chat.parse::<u64>() != Ok(user)
            || message <= 0
        {
            return Ok(screen(expired(lang)));
        }
        let binding = ApprovalBinding::Legacy { user, message };
        let pending = {
            let _guard = STATE.lock().map_err(|_| "agent state lock failed")?;
            let receipt = binding.receipt(handle);
            let key = binding.key("pending", chat);
            let pending = auth
                .get::<LegacyPending>(&key)
                .ok()
                .flatten()
                .filter(|pending| {
                    pending.chat == chat
                        && pending.receipt.handle == handle
                        && pending.receipt.user == user
                        && pending.receipt.message == message
                });
            let already_dispatched =
                auth.store.get(&binding.key("receipt", chat))? == Some(receipt);
            let Some(pending) = pending.filter(|_| !already_dispatched) else {
                return receipt_reply(auth, chat, handle, lang, binding);
            };
            if decision == "x" {
                invalidate(auth, chat)?;
                diagnostic("cancelled");
                return Ok(screen(pending.lang.text("Cancelled.", "تم الإلغاء.")));
            }
            auth.store.delete(&key)?;
            Pending {
                handle: handle.into(),
                grant: pending.grant,
                epoch: pending.epoch,
                draft: Draft {
                    name: pending.tool,
                    args: pending.args,
                },
                lang: pending.lang,
            }
        };
        self.submit(auth, chat, pending, binding).await
    }

    async fn submit(
        &self,
        auth: &Auth,
        chat: &str,
        pending: Pending,
        binding: ApprovalBinding,
    ) -> Result<Screen> {
        let receipt = binding.key("receipt", chat);
        let check = || {
            current_at(
                auth,
                chat,
                &binding.key("epoch", chat),
                &pending.epoch,
                &pending.grant,
            )
        };
        let lang = pending.lang;
        let target = async {
            let access = auth.current_access(chat)?.ok_or("agent access missing")?;
            if matches!(access, Access::Student(_)) {
                return Err("student cannot confirm agent writes".into());
            }
            check()?;
            let mcp = Mcp::new(&auth.http, &auth.origin, &access);
            let catalog = mcp.catalog().await?;
            let tool = catalog
                .into_iter()
                .filter_map(Allowed::from_tool)
                .find(|tool| tool.name == pending.draft.name && tool.name.write())
                .ok_or("agent tool no longer available")?;
            tool.validate(&pending.draft.args)?;
            Result::Ok((access, tool.name))
        }
        .await;
        let Ok((access, name)) = target else {
            diagnostic("approval_unavailable");
            return Ok(screen(lang.text(
                "This request is no longer available. Use /menu.",
                "لم يعد الطلب متاحاً. افتح القائمة.",
            )));
        };
        {
            let _guard = STATE.lock().map_err(|_| "agent state lock failed")?;
            check()?;
            auth.store.set(
                &receipt,
                &binding.receipt(&pending.handle),
                now()? + APPROVAL_TTL,
            )?;
        }
        diagnostic("dispatched");
        let result = Mcp::new(&auth.http, &auth.origin, &access)
            .call(name.wire(), pending.draft.args.clone())
            .await;
        if check().is_err() {
            return Ok(screen(denied(lang)));
        }
        let outcome = result
            .ok()
            .and_then(|result| outcome(name, &pending.draft.args, &result));
        let Some(saved) = outcome else {
            diagnostic("uncertain");
            return Ok(screen(uncertain(lang)));
        };
        auth.store.delete(&receipt)?;
        if !saved {
            diagnostic("conflict");
            return Ok(screen(if name == Name::SyncAttendance {
                lang.text(
                    "Attendance changed or could not be saved. Review before trying again.",
                    "تغير الحضور أو تعذر حفظه. راجع النتيجة قبل المحاولة.",
                )
            } else {
                lang.text(
                    "The record changed or could not be saved. Review before trying again.",
                    "تغير السجل أو تعذر حفظه. راجع النتيجة.",
                )
            }));
        }
        diagnostic("submitted");
        Ok(screen(lang.text(
            "Request submitted. Open the menu to check the result.",
            "تم إرسال الطلب. افتح القائمة للتحقق من النتيجة.",
        )))
    }
}

fn outcome(name: Name, args: &Value, result: &Value) -> Option<bool> {
    fn matching_result<'a>(id: &str, result: &'a Value) -> Option<&'a str> {
        (result["id"].as_str()? == id).then_some(result["kind"].as_str()?)
    }
    match name {
        Name::SyncAttendance => {
            let expected = args["records"]
                .as_array()?
                .iter()
                .map(|record| record["event"]["id"].as_str())
                .collect::<Option<HashSet<_>>>()?;
            let outcomes = result["outcomes"].as_array()?;
            if expected.is_empty() || expected.len() != outcomes.len() {
                return None;
            }
            let mut seen = HashSet::new();
            let mut saved = true;
            for outcome in outcomes {
                let id = outcome["id"].as_str()?;
                if !expected.contains(id) || !seen.insert(id) {
                    return None;
                }
                let version = outcome.get("current")?;
                if !version.is_null()
                    && !(version["eventId"].as_str().is_some_and(schema::uuid)
                        && version.get("status").is_some_and(|status| {
                            status.is_null()
                                || matches!(status.as_str(), Some("present" | "absent" | "excused"))
                        }))
                {
                    return None;
                }
                match matching_result(id, outcome)? {
                    "saved" | "unchanged" => {}
                    "conflict" | "forbidden" | "id_conflict" => saved = false,
                    _ => return None,
                }
            }
            Some(saved)
        }
        Name::SyncStudent => {
            let operation = &args["operation"];
            let id = if operation["kind"] == "note" {
                &operation["event"]["id"]
            } else {
                &operation["id"]
            };
            let whatsapp = result.get("currentWhatsapp")?;
            if !whatsapp.is_null()
                && !whatsapp
                    .as_str()
                    .is_some_and(|phone| phone.encode_utf16().count() <= 40)
            {
                return None;
            }
            match matching_result(id.as_str()?, result)? {
                "saved" => Some(true),
                "conflict" | "forbidden" | "id_conflict" => Some(false),
                _ => None,
            }
        }
        _ => Some(!result.is_null()),
    }
}

fn matching_pending(auth: &Auth, chat: &str, action: &Action) -> Result<Option<Pending>> {
    Ok(auth
        .get::<Pending>(&key("pending", chat))?
        .filter(|pending| pending.handle == action.value("approval")))
}

fn receipt_reply(
    auth: &Auth,
    chat: &str,
    handle: &str,
    lang: Language,
    binding: ApprovalBinding,
) -> Result<Screen> {
    Ok(screen(
        if auth.store.get(&binding.key("receipt", chat))? == Some(binding.receipt(handle)) {
            uncertain(lang)
        } else {
            expired(lang)
        },
    ))
}

fn load_context(auth: &Auth, chat: &str, grant: &str) -> Result<Context> {
    let current = auth
        .get::<BoundContext>(&key("context", chat))?
        .filter(|saved| saved.grant == grant);
    let mut context = if let Some(saved) = current {
        saved.context
    } else {
        auth.get::<LegacyBoundContext>(&legacy_key("context", chat))
            .ok()
            .flatten()
            .filter(|saved| saved.grant == grant)
            .map(|saved| Context {
                turns: saved
                    .context
                    .turns
                    .into_iter()
                    .map(|turn| turn.question)
                    .collect(),
                labels: saved.context.labels,
            })
            .unwrap_or_default()
    };
    context
        .turns
        .retain(|question| question.encode_utf16().count() <= 1000 && !has_private_input(question));
    if context.turns.len() > 4 {
        context.turns.drain(..context.turns.len() - 4);
    }
    context.labels.retain(|id, label| {
        schema::uuid(id)
            && !has_private_input(label)
            && label.encode_utf16().count() <= 200
            && !label.contains(char::is_control)
    });
    Ok(context)
}

fn invalidate(auth: &Auth, chat: &str) -> Result<()> {
    auth.store.replace(
        &[
            &key("epoch", chat),
            &key("context", chat),
            &key("pending", chat),
            &legacy_key("epoch", chat),
            &legacy_key("context", chat),
            &legacy_key("pending", chat),
        ],
        &[],
    )
}

fn key(kind: &str, chat: &str) -> String {
    format!("native-agent-{kind}:{chat}")
}

fn legacy_key(kind: &str, chat: &str) -> String {
    format!("agent-{kind}:{chat}")
}

fn current(auth: &Auth, chat: &str, epoch: &str, grant: &str) -> Result<()> {
    current_at(auth, chat, &key("epoch", chat), epoch, grant)
}

fn current_at(auth: &Auth, chat: &str, key: &str, epoch: &str, grant: &str) -> Result<()> {
    if auth.get::<String>(key)?.as_deref() == Some(epoch)
        && auth
            .current_access(chat)?
            .as_ref()
            .is_some_and(|access| fingerprint(access) == grant)
    {
        Ok(())
    } else {
        Err("agent state or access changed".into())
    }
}

fn fingerprint(access: &Access) -> String {
    let (kind, token) = match access {
        Access::Student(token) => ("student", token),
        Access::Teacher { token, .. } => ("teacher", token),
        Access::OAuth(token) => ("oauth", token),
    };
    hex_digest(format!("{kind}:{token}").as_bytes())
}

fn shortlist<'a>(question: &str, context: &Context, tools: &'a [Allowed]) -> Vec<&'a Allowed> {
    use Name::*;
    let subject = format!("{} {question}", context.turns.join(" "));
    let mut names = HashSet::from([
        ListCourses,
        FindStudents,
        GetStudentDetails,
        GetMyAttendance,
    ]);
    let mut matched = false;
    let mut include = |pattern, tools: &[Name]| {
        if matches(pattern, &subject) {
            matched = true;
            names.extend(tools);
        }
    };
    include(
        r"(?i)attendance|absen|present|excused|mark|حضور|غياب|غائب|حاضر|معذور",
        &[GetCourseAttendanceView, SyncAttendance],
    );
    include(
        r"(?i)note|history|correct|ملاحظ|تصحيح",
        &[GetNotebookHistory, AppendEvents, SyncStudent],
    );
    include(
        r"(?i)course|class|enroll|member|grade|result|handbook|report|export|xlsx|دور|تسجيل|علام|درج|تقرير",
        &[
            GetCourseWorkflow,
            GetCourseAttendanceView,
            ExportCourseReport,
            SaveCourse,
            SetCourseStatus,
            SetCourseThresholds,
            SaveEnrollment,
            SaveGrades,
            SaveGradeDecision,
            SetHandbookReceived,
            CreateStudentInCourse,
        ],
    );
    include(
        r"(?i)student|roster|contact|phone|whatsapp|طالب|طلاب|رقم|هاتف",
        &[
            UpdateStudent,
            SyncStudent,
            SaveStudent,
            CreateStudentInCourse,
        ],
    );
    include(
        r"(?i)profile|account|ملف|حساب",
        &[GetAccount, GetProfile, UpdateProfile],
    );
    tools
        .iter()
        .filter(|tool| !matched || names.contains(&tool.name))
        .collect()
}

fn has_private_input(question: &str) -> bool {
    if matches(
        r"(?i)(password|passcode|token|secret|كلمة (?:السر|المرور)|رمز التحقق)\s*(?:[:=]|\bis\b|ه[وي]\s)\s*\S+",
        question,
    ) || matches(r"(?i)[\w.+-]+@[\w.-]+\.[a-z]{2,}", question)
    {
        return true;
    }
    let text = normalize_digits(question);
    let mut masked = text.clone().into_bytes();
    for (start, end) in date_spans(&text) {
        masked[start..end].fill(b' ');
    }
    String::from_utf8(masked).is_ok_and(|text| matches(r"(?:\p{Nd}[\s()+-]*){8,}", &text))
}

fn date_spans(text: &str) -> Vec<(usize, usize)> {
    schema::regex(r"\b[0-9]{4}-[0-9]{2}-[0-9]{2}\b")
        .into_iter()
        .flat_map(|regex| {
            regex
                .find_iter(text)
                .filter_map(|found| {
                    let prefix = text[..found.start()].chars().next_back();
                    let suffix = &text[found.end()..];
                    (schema::date(found.as_str())
                        && !prefix.is_some_and(|ch| ch == '+' || ch == '-' || ch.is_numeric())
                        && !matches(r"^-\p{Nd}", suffix))
                    .then_some((found.start(), found.end()))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn explicit_date(question: &str) -> Option<String> {
    let text = normalize_digits(question);
    let found: HashSet<_> = date_spans(&text)
        .iter()
        .map(|&(start, end)| &text[start..end])
        .collect();
    if found.len() != 1 {
        return None;
    }
    found.into_iter().next().map(str::to_owned)
}

fn detect_language(question: &str, lang: Language) -> Language {
    if matches(r"\p{Arabic}{3}", question) {
        Language::Ar
    } else if matches(r"(?i)[a-z]{3}", question) {
        Language::En
    } else {
        lang
    }
}

fn clean_words(text: &str) -> String {
    text.nfkc()
        .flat_map(char::to_lowercase)
        .filter_map(|ch| match ch {
            '\u{064b}'..='\u{065f}' | '\u{0670}' | '\u{0640}' => None,
            'إ' | 'أ' | 'آ' => Some('ا'),
            'ى' => Some('ي'),
            ch if ch.is_alphanumeric() || ch.is_whitespace() => Some(ch),
            _ => Some(' '),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn matches(pattern: &str, text: &str) -> bool {
    schema::regex(pattern).is_some_and(|regex| regex.is_match(text))
}

fn bounded(text: &str, max: usize) -> String {
    let mut size = 0;
    text.chars()
        .filter(|ch| !ch.is_control() || *ch == '\n')
        .take_while(|ch| {
            size += ch.len_utf16();
            size <= max
        })
        .collect()
}

fn screen(text: impl Into<String>) -> Screen {
    Screen {
        text: bounded(&text.into(), 3500),
        rows: vec![],
        prompt: None,
        document: None,
    }
}

fn diagnostic(outcome: &'static str) {
    eprintln!(
        "{}",
        json!({"message": "telegram_agent", "outcome": outcome})
    );
}

fn expired(lang: Language) -> &'static str {
    lang.text(
        "This approval has expired. Use /menu.",
        "انتهت صلاحية التأكيد. استخدم /menu.",
    )
}

fn uncertain(lang: Language) -> &'static str {
    lang.text(
        "The outcome is unknown. Check the result before trying again.",
        "النتيجة غير مؤكدة. تحقق منها قبل المحاولة مجدداً.",
    )
}

fn unavailable(lang: Language) -> &'static str {
    lang.text("Sorry, I cannot answer right now. Please try again, or use /menu and ask your teacher for help.", "عذراً، ما قدرت جاوب هلق. جرّب مرة ثانية، أو افتح /menu واطلب المساعدة من معلمك.")
}

fn invalid(lang: Language) -> &'static str {
    lang.text(
        "Sorry, I could not prepare a reliable answer. Please try asking another way or use /menu.",
        "عذراً، ما قدرت أعطيك جواب موثوق. جرّب تسأل بطريقة ثانية أو افتح /menu.",
    )
}

fn denied(lang: Language) -> &'static str {
    lang.text("Open /menu to check that you’re signed in. If you still can’t see the record, ask your teacher or an administrator.", "افتح /menu لتتأكد أنك مسجّل الدخول. إذا ما ظهر السجل، اسأل معلمك أو المسؤول.")
}

fn unsupported(lang: Language) -> &'static str {
    lang.text("I can help with Wartaq questions. For other requests, please ask your teacher. Use /menu to open Wartaq.", "فيني ساعدك بأسئلة وارتق. للطلبات الثانية اسأل معلمك، واستخدم /menu لفتح القائمة.")
}
