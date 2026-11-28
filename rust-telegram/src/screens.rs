use std::collections::{HashMap, HashSet};

use chrono::{FixedOffset, NaiveDate, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::json;
use url::Url;
use wartaq_common::rank_by_name;

use crate::{
    auth::{Access, Auth, Result, random},
    mcp::Mcp,
    worker::{Action, Language},
};

mod admin;
mod courses;

const PAGE: usize = 8;

pub(crate) enum Button {
    Choice(String, Action),
    Link(String, Url),
}

pub(crate) struct Screen {
    pub(crate) text: String,
    pub(crate) rows: Vec<Vec<Button>>,
    pub(crate) prompt: Option<Action>,
    pub(crate) document: Option<(String, Vec<u8>)>,
}

impl Screen {
    fn new(text: impl Into<String>, rows: Vec<Vec<Button>>) -> Self {
        Self {
            text: text.into(),
            rows,
            prompt: None,
            document: None,
        }
    }

    fn prompt(mut self, action: Action) -> Self {
        self.prompt = Some(action);
        self
    }
}

fn choice(label: &str, action: Action) -> Button {
    Button::Choice(label.chars().take(60).collect(), action)
}

fn home(lang: Language) -> Button {
    choice(lang.text("Home", "الرئيسية"), Action::new("home"))
}

fn unavailable(lang: Language) -> Screen {
    Screen::new(
        lang.text("This action is unavailable.", "هذا الإجراء غير متاح."),
        vec![vec![home(lang)]],
    )
}

fn back(lang: Language, action: Action) -> Button {
    choice(lang.text("Back", "رجوع"), action)
}

fn page(action: &Action) -> usize {
    action
        .value("page")
        .parse::<usize>()
        .unwrap_or_default()
        .min(100_000)
}

fn nav(lang: Language, action: Action, index: usize, size: usize) -> Vec<Vec<Button>> {
    let mut row = Vec::new();
    if index > 0 {
        row.push(choice(
            lang.text("Previous", "السابق"),
            action.clone().arg("page", (index - 1).to_string()),
        ));
    }
    if (index + 1) * PAGE < size {
        row.push(choice(
            lang.text("Next", "التالي"),
            action.arg("page", (index + 1).to_string()),
        ));
    }
    if row.is_empty() { vec![] } else { vec![row] }
}

fn short(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.into();
    }
    value
        .chars()
        .take(max)
        .chain(std::iter::once('…'))
        .collect()
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    #[serde(alias = "early", alias = "late")]
    Present,
    Absent,
    Excused,
}

impl Status {
    fn wire(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Absent => "absent",
            Self::Excused => "excused",
        }
    }
    fn text(self, lang: Language) -> &'static str {
        match self {
            Self::Present => lang.text("Present", "حاضر"),
            Self::Absent => lang.text("Absent", "غائب"),
            Self::Excused => lang.text("Excused", "معذور"),
        }
    }
}

#[derive(Deserialize)]
struct Attendance {
    date: NaiveDate,
    course: Option<String>,
    status: Status,
}

#[derive(Deserialize)]
struct MyStudent {
    name: String,
}

#[derive(Deserialize)]
struct Mine {
    student: MyStudent,
    attendance: Vec<Attendance>,
}

#[derive(Deserialize)]
struct AccountResult {
    account: Account,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    name: String,
    invitation_profile: Option<Invitation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Invitation {
    requires_setup: bool,
}

#[derive(Deserialize)]
struct Profile {
    name: String,
    image: Option<String>,
}

#[derive(Deserialize)]
struct Course {
    id: String,
    name: String,
    open: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Student {
    student_id: String,
    name: String,
    active: bool,
    group: String,
    whatsapp: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    location: Option<String>,
    birth_year: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Enrollment {
    id: String,
    student_id: String,
    course_id: String,
    active: bool,
}

#[derive(Deserialize)]
struct Event {
    id: String,
    payload: Payload,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
enum Payload {
    Attendance {
        student_id: String,
        enrollment_id: Option<String>,
        date: NaiveDate,
        status: Option<Status>,
    },
    Reversal {
        target_id: String,
    },
    Note {
        student_id: String,
        date: NaiveDate,
        tags: Vec<String>,
        text: String,
    },
    Student,
}

#[derive(Deserialize)]
struct Notebook {
    courses: Vec<Course>,
    students: Vec<Student>,
    enrollments: Vec<Enrollment>,
    events: Vec<Event>,
}

#[derive(Deserialize)]
struct SyncResponse {
    outcomes: Vec<Outcome>,
}

#[derive(Deserialize)]
struct Outcome {
    id: String,
    kind: String,
}

pub(crate) async fn render(
    auth: &Auth,
    access: &Access,
    lang: Language,
    action: &Action,
    input: Option<&str>,
) -> Result<Screen> {
    if let Access::Student(token) = access {
        let mine: Mine = auth
            .phone(json!({ "action": "attendance", "token": token }))
            .await?;
        return match action.route.as_str() {
            "nb.mine" => Ok(mine_screen(lang, action, mine)),
            _ => Ok(welcome(
                lang,
                &mine.student.name,
                vec![
                    vec![choice(
                        lang.text("My attendance", "سجل حضوري"),
                        Action::new("nb.mine"),
                    )],
                    vec![choice(
                        lang.text("العربية", "English"),
                        Action::new("sys.language"),
                    )],
                ],
            )),
        };
    }
    let mcp = Mcp::new(&auth.http, &auth.origin, access);
    let tools = mcp.tools().await?;
    match action.route.as_str() {
        "home" if tools.contains("get_account") => {
            let account: AccountResult =
                serde_json::from_value(mcp.call("get_account", json!({})).await?)?;
            if account
                .account
                .invitation_profile
                .is_some_and(|profile| profile.requires_setup)
            {
                let mut screen = welcome(
                    lang,
                    &account.account.name,
                    vec![vec![Button::Link(
                        lang.text("Finish account setup", "إكمال إعداد الحساب")
                            .into(),
                        auth.origin.join("/?page=app")?,
                    )]],
                );
                screen.text.push('\n');
                screen.text.push_str(lang.text(
                    "Finish setting up your Wartaq account to open your notebook.",
                    "أكمل إعداد حسابك في وارتق لفتح دفترك.",
                ));
                return Ok(screen);
            }
            let mut rows = Vec::new();
            if tools.contains("get_my_attendance") {
                rows.push(vec![choice(
                    lang.text("My attendance", "سجل حضوري"),
                    Action::new("nb.mine"),
                )]);
            }
            if tools.contains("get_notebook") {
                rows.push(vec![choice(
                    lang.text("Take attendance", "تسجيل الحضور"),
                    Action::new("nb.attendance"),
                )]);
                let mut admin_row = Vec::new();
                if tools.contains("save_student") {
                    admin_row.push(choice(
                        lang.text("Add student", "إضافة طالب"),
                        Action::new("nb.add"),
                    ));
                }
                if tools.contains("get_admin_board") {
                    admin_row.push(choice(
                        lang.text("Accounts", "الحسابات"),
                        Action::new("ac.list"),
                    ));
                }
                if !admin_row.is_empty() {
                    rows.push(admin_row);
                }
                rows.push(vec![
                    choice(lang.text("Students", "الطلاب"), Action::new("nb.students")),
                    choice(lang.text("Courses", "الدورات"), Action::new("co.list")),
                ]);
                rows.push(vec![choice(
                    lang.text("Attendance summary", "ملخص الحضور"),
                    Action::new("nb.summary"),
                )]);
            }
            let mut row = Vec::new();
            if tools.contains("get_profile") {
                row.push(choice(
                    lang.text("My account", "حسابي"),
                    Action::new("ac.profile"),
                ));
            }
            row.push(choice(
                lang.text("العربية", "English"),
                Action::new("sys.language"),
            ));
            rows.push(row);
            Ok(welcome(lang, &account.account.name, rows))
        }
        "nb.mine" if tools.contains("get_my_attendance") => {
            let mine = serde_json::from_value(mcp.call("get_my_attendance", json!({})).await?)?;
            Ok(mine_screen(lang, action, mine))
        }
        "ac.profile" | "ac.profile.name" | "ac.profile.save" if tools.contains("get_profile") => {
            profile(&mcp, &auth.origin, &tools, lang, action, input).await
        }
        "ac.list" | "ac.search" | "ac.view" | "ac.invite" | "ac.invite.name"
        | "ac.invite.phone" | "ac.invite.send" => {
            admin::accounts(&mcp, &auth.origin, &tools, lang, action, input).await
        }
        "nb.followup" | "nb.chart" if tools.contains("get_course_attendance_view") => {
            insights(&mcp, &auth.origin, lang, action).await
        }
        route
            if tools.contains("get_notebook")
                && (matches!(
                    route,
                    "nb.students"
                        | "nb.add"
                        | "nb.add.save"
                        | "nb.add.level"
                        | "nb.edit"
                        | "nb.phone"
                        | "nb.phone.save"
                        | "nb.search"
                        | "nb.student"
                        | "nb.history"
                        | "nb.history.item"
                        | "nb.reverse.reason"
                        | "nb.reverse.save"
                        | "nb.note.tags"
                        | "nb.note.text"
                        | "nb.note.save"
                        | "nb.attendance"
                        | "nb.roll"
                        | "nb.mark"
                        | "nb.date"
                        | "nb.summary"
                        | "nb.summary.course"
                        | "co.list"
                        | "co.search"
                        | "co.view"
                        | "co.reports"
                        | "co.report"
                ) || courses::is_route(route)) =>
        {
            let snapshot = mcp.call("get_notebook", json!({})).await?;
            let notebook = Notebook::deserialize(&snapshot)?;
            match route {
                "co.search" if action.value("target") == "co.members" => {
                    if let Some(input) = input {
                        let action = Action::new("co.members")
                            .arg("id", action.value("id"))
                            .arg("q", input.chars().take(200).collect::<String>());
                        courses::render(&mcp, &tools, lang, &action, None, &snapshot, &notebook)
                            .await
                    } else {
                        Ok(Screen::new(
                            lang.text(
                                "Send a name to search, or /cancel.",
                                "أرسل اسماً للبحث، أو /cancel للإلغاء.",
                            ),
                            vec![vec![
                                back(
                                    lang,
                                    Action::new("co.members").arg("id", action.value("id")),
                                ),
                                home(lang),
                            ]],
                        )
                        .prompt(action.clone()))
                    }
                }
                route if courses::is_route(route) => {
                    courses::render(&mcp, &tools, lang, action, input, &snapshot, &notebook).await
                }
                "nb.add" | "nb.add.save" | "nb.add.level" => {
                    admin::add_student(&mcp, &tools, lang, action, input, &notebook).await
                }
                "nb.edit" | "nb.phone" | "nb.phone.save" => {
                    admin::edit_student(&mcp, &tools, lang, action, input, &notebook).await
                }
                "nb.roll" | "nb.mark" | "nb.date" => {
                    roll(&mcp, &tools, lang, action, input, notebook).await
                }
                "nb.note.tags" | "nb.note.text" | "nb.note.save" => {
                    note(&mcp, &tools, lang, action, input, &notebook).await
                }
                "nb.history" | "nb.history.item" | "nb.reverse.reason" | "nb.reverse.save" => {
                    history(&mcp, &tools, lang, action, input, &notebook).await
                }
                "nb.attendance" | "nb.summary" => Ok(choose_course(lang, action, &notebook)),
                "nb.summary.course" => Ok(summary(
                    lang,
                    action,
                    &notebook,
                    tools.contains("get_course_attendance_view"),
                )),
                "co.list" | "co.search" | "co.view" | "co.reports" | "co.report" => {
                    course(&mcp, &tools, lang, action, input, &notebook).await
                }
                _ => Ok(roster(lang, action, input, &notebook, &tools)),
            }
        }
        _ => Ok(unavailable(lang)),
    }
}

fn welcome(lang: Language, name: &str, rows: Vec<Vec<Button>>) -> Screen {
    Screen::new(
        format!("{} {}.", lang.text("Welcome,", "مرحباً"), short(name, 120)),
        rows,
    )
}

fn mine_screen(lang: Language, action: &Action, mut mine: Mine) -> Screen {
    let index = page(action);
    mine.attendance
        .sort_by_key(|entry| std::cmp::Reverse(entry.date));
    let mut lines = vec![format!(
        "{} — {}",
        lang.text("My attendance", "حضوري"),
        short(&mine.student.name, 200)
    )];
    lines.extend(
        mine.attendance
            .iter()
            .skip(index * PAGE)
            .take(PAGE)
            .map(|entry| {
                format!(
                    "{} · {} · {}",
                    entry.date,
                    short(
                        entry
                            .course
                            .as_deref()
                            .unwrap_or(lang.text("Previous / no course", "سابق / دون دورة")),
                        80
                    ),
                    lang.text(entry.status.wire(), entry.status.text(lang)),
                )
            }),
    );
    if lines.len() == 1 {
        lines.push(
            lang.text("No attendance recorded.", "لا يوجد حضور مسجل.")
                .into(),
        );
    }
    let mut rows = nav(lang, Action::new("nb.mine"), index, mine.attendance.len());
    rows.push(vec![home(lang)]);
    Screen::new(lines.join("\n"), rows)
}

async fn profile(
    mcp: &Mcp<'_>,
    origin: &Url,
    tools: &HashSet<String>,
    lang: Language,
    action: &Action,
    input: Option<&str>,
) -> Result<Screen> {
    let profile: Profile = serde_json::from_value(mcp.call("get_profile", json!({})).await?)?;
    let profile_action = Action::new("ac.profile");
    let back_row = || vec![vec![back(lang, profile_action.clone())]];
    match action.route.as_str() {
        "ac.profile" => {
            let mut rows = Vec::new();
            if tools.contains("update_profile") {
                rows.push(vec![choice(
                    lang.text("Edit name", "تعديل الاسم"),
                    Action::new("ac.profile.name"),
                )]);
            }
            rows.push(vec![Button::Link(
                lang.text("Photo, phone and password", "الصورة والهاتف وكلمة المرور")
                    .into(),
                origin.join("/?page=app&view=profile")?,
            )]);
            rows.push(vec![home(lang)]);
            Ok(Screen::new(
                format!(
                    "{}\n{}: {}\n{}: {}",
                    lang.text("Profile", "الملف الشخصي"),
                    lang.text("Name", "الاسم"),
                    short(&profile.name, 120),
                    lang.text("Photo", "الصورة"),
                    if profile.image.is_some() {
                        lang.text("Set", "موجودة")
                    } else {
                        lang.text("Not set", "غير موجودة")
                    },
                ),
                rows,
            ))
        }
        "ac.profile.name" if tools.contains("update_profile") => {
            if let Some(name) = input.filter(|name| valid_name(name)) {
                return Ok(Screen::new(
                    format!(
                        "{}: {name}?",
                        lang.text("Change your name to", "تغيير الاسم إلى")
                    ),
                    vec![
                        vec![choice(
                            lang.text("Confirm", "تأكيد"),
                            Action::new("ac.profile.save")
                                .arg("name", name)
                                .arg("expected", profile.name),
                        )],
                        vec![back(lang, profile_action)],
                    ],
                ));
            }
            let text = if input.is_none() {
                lang.text(
                    "Send your new display name, or /cancel.",
                    "أرسل اسم العرض الجديد، أو /cancel للإلغاء.",
                )
            } else {
                lang.text(
                    "Enter a name of 1–120 characters.",
                    "أدخل اسماً من ١ إلى ١٢٠ حرفاً.",
                )
            };
            Ok(Screen::new(text, back_row()).prompt(Action::new("ac.profile.name")))
        }
        "ac.profile.save"
            if tools.contains("update_profile") && valid_name(action.value("name")) =>
        {
            if action.args.get("expected") != Some(&profile.name) {
                return Ok(Screen::new(
                    lang.text(
                        "Your name changed. Open your profile and try again.",
                        "تغير اسمك. افتح ملفك الشخصي وحاول مجدداً.",
                    ),
                    back_row(),
                ));
            }
            mcp.call(
                "update_profile",
                json!({ "input": { "name": action.value("name") } }),
            )
            .await?;
            Ok(Screen::new(
                lang.text("Name saved.", "تم حفظ الاسم."),
                back_row(),
            ))
        }
        _ => Ok(unavailable(lang)),
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= 120
}

fn roster(
    lang: Language,
    action: &Action,
    input: Option<&str>,
    notebook: &Notebook,
    tools: &HashSet<String>,
) -> Screen {
    if action.route == "nb.search" && input.is_none() {
        return Screen::new(
            lang.text("Send a student name to search.", "أرسل اسم الطالب للبحث."),
            vec![vec![back(lang, Action::new("nb.students")), home(lang)]],
        )
        .prompt(Action::new("nb.search"));
    }
    if action.route == "nb.student" {
        let Some(student) = notebook
            .students
            .iter()
            .find(|s| s.student_id == action.value("studentId"))
        else {
            return unavailable(lang);
        };
        let target = |route| Action::new(route).arg("studentId", &student.student_id);
        let mut rows = Vec::new();
        if tools.contains("update_student") && student.active {
            rows.push(vec![choice(
                lang.text("Edit student", "تعديل الطالب"),
                target("nb.edit"),
            )]);
        }
        if tools.contains("append_events") && student.active {
            rows.push(vec![choice(
                lang.text("Add note", "إضافة ملاحظة"),
                target("nb.note.tags"),
            )]);
        }
        rows.push(vec![choice(
            lang.text("History", "السجل"),
            target("nb.history"),
        )]);
        rows.push(vec![back(lang, Action::new("nb.students")), home(lang)]);
        return Screen::new(
            format!(
                "{}\n{}: {}\n{}: {}\n{}",
                short(&student.name, 200),
                lang.text("Level", "المستوى"),
                if student.group.is_empty() {
                    "—"
                } else {
                    &student.group
                },
                lang.text("Phone", "الهاتف"),
                student.whatsapp.as_deref().unwrap_or("—"),
                if student.active {
                    lang.text("Active", "نشط")
                } else {
                    lang.text("Inactive", "غير نشط")
                }
            ),
            rows,
        );
    }
    let query = short(input.unwrap_or_else(|| action.value("query")).trim(), 200);
    let students = rank_by_name(&notebook.students, &query, |student| &student.name);
    let index = page(action);
    let mut rows: Vec<_> = students
        .iter()
        .skip(index * PAGE)
        .take(PAGE)
        .map(|student| {
            vec![choice(
                &student.name,
                Action::new("nb.student").arg("studentId", &student.student_id),
            )]
        })
        .collect();
    rows.extend(nav(
        lang,
        Action::new("nb.students").arg("query", query),
        index,
        students.len(),
    ));
    let mut controls = vec![choice(lang.text("Search", "بحث"), Action::new("nb.search"))];
    if tools.contains("save_student") {
        controls.push(choice(
            lang.text("Add student", "إضافة طالب"),
            Action::new("nb.add"),
        ));
    }
    rows.push(controls);
    rows.push(vec![home(lang)]);
    Screen::new(
        format!("{} ({})", lang.text("Students", "الطلاب"), students.len()),
        rows,
    )
}

fn choose_course(lang: Language, action: &Action, notebook: &Notebook) -> Screen {
    let summarize = action.route == "nb.summary";
    let courses: Vec<_> = notebook
        .courses
        .iter()
        .filter(|course| summarize || course.open)
        .collect();
    let index = page(action);
    let mut rows = Vec::new();
    if summarize && index == 0 {
        rows.push(vec![choice(
            lang.text("Previous / no course", "سابق / دون دورة"),
            Action::new("nb.summary.course").arg("courseId", "previous"),
        )]);
    }
    rows.extend(courses.iter().skip(index * PAGE).take(PAGE).map(|course| {
        vec![choice(
            &course.name,
            Action::new(if summarize {
                "nb.summary.course"
            } else {
                "nb.roll"
            })
            .arg("courseId", &course.id)
            .arg("date", today().to_string()),
        )]
    }));
    rows.extend(nav(lang, action.clone(), index, courses.len()));
    rows.push(vec![home(lang)]);
    Screen::new(
        if summarize {
            lang.text("Choose a course for summary.", "اختر دورة للملخص.")
        } else {
            lang.text("Choose a course for attendance.", "اختر دورة للحضور.")
        },
        rows,
    )
}

type MarkKey<'a> = (&'a str, Option<&'a str>, NaiveDate);
type Mark<'a> = (&'a str, Option<Status>);

fn event_order(events: &[Event]) -> (Vec<&Event>, HashSet<&str>) {
    let mut unique: HashMap<&str, &Payload> = HashMap::new();
    let mut reversed = HashSet::new();
    let mut ordered = Vec::new();
    for event in events {
        if unique.contains_key(event.id.as_str()) {
            continue;
        }
        if let Payload::Reversal { target_id } = &event.payload {
            if unique
                .get(target_id.as_str())
                .is_some_and(|payload| !matches!(payload, Payload::Reversal { .. }))
            {
                reversed.insert(target_id.as_str());
            }
        }
        unique.insert(event.id.as_str(), &event.payload);
        ordered.push(event);
    }
    (ordered, reversed)
}

fn projected_marks(events: &[Event]) -> HashMap<MarkKey<'_>, Mark<'_>> {
    let (ordered, reversed) = event_order(events);
    ordered
        .into_iter()
        .filter_map(|event| {
            if reversed.contains(event.id.as_str()) {
                return None;
            }
            match &event.payload {
                Payload::Attendance {
                    student_id,
                    enrollment_id,
                    date,
                    status,
                } => Some((
                    (student_id.as_str(), enrollment_id.as_deref(), *date),
                    (event.id.as_str(), *status),
                )),
                _ => None,
            }
        })
        .collect()
}

fn summary(
    lang: Language,
    action: &Action,
    notebook: &Notebook,
    can_read_insights: bool,
) -> Screen {
    let course_id = action.value("courseId");
    let course = notebook
        .courses
        .iter()
        .find(|course| course.id == course_id);
    let previous = course_id == "previous";
    let heading = match course {
        Some(course) => &course.name,
        None if previous => lang.text("Previous / no course", "سابق / دون دورة"),
        None => return unavailable(lang),
    };
    let entries: Vec<_> = if previous {
        notebook
            .students
            .iter()
            .map(|student| (student, None))
            .collect()
    } else {
        notebook
            .enrollments
            .iter()
            .filter(|entry| entry.course_id == course_id)
            .filter_map(|entry| {
                notebook
                    .students
                    .iter()
                    .find(|s| s.student_id == entry.student_id)
                    .map(|student| (student, Some(entry.id.as_str())))
            })
            .collect()
    };
    let marks = projected_marks(&notebook.events);
    let index = page(action);
    let mut lines = vec![short(heading, 200)];
    for (student, enrollment) in entries.iter().skip(index * PAGE).take(PAGE) {
        let mut counts = [0; 3];
        for (&(student_id, enrollment_id, _), &(_, status)) in &marks {
            if student_id == student.student_id && enrollment_id == *enrollment {
                match status {
                    Some(Status::Present) => counts[0] += 1,
                    Some(Status::Absent) => counts[1] += 1,
                    Some(Status::Excused) => counts[2] += 1,
                    None => {}
                }
            }
        }
        lines.push(format!(
            "{}: {} {} · {} {} · {} {}",
            short(&student.name, 100),
            Status::Present.text(lang),
            counts[0],
            Status::Absent.text(lang),
            counts[1],
            Status::Excused.text(lang),
            counts[2]
        ));
    }
    if entries.is_empty() {
        lines.push(
            lang.text("No enrolled students.", "لا يوجد طلاب مسجلون.")
                .into(),
        );
    }
    let mut rows = nav(
        lang,
        Action::new("nb.summary.course").arg("courseId", course_id),
        index,
        entries.len(),
    );
    if can_read_insights {
        rows.push(vec![
            choice(
                lang.text("Follow-up", "المتابعة"),
                Action::new("nb.followup").arg("courseId", course_id),
            ),
            choice(
                lang.text("Attendance chart", "مخطط الحضور"),
                Action::new("nb.chart").arg("courseId", course_id),
            ),
        ]);
    }
    rows.push(vec![back(lang, Action::new("nb.summary")), home(lang)]);
    Screen::new(lines.join("\n"), rows)
}

async fn roll(
    mcp: &Mcp<'_>,
    tools: &HashSet<String>,
    lang: Language,
    action: &Action,
    input: Option<&str>,
    notebook: Notebook,
) -> Result<Screen> {
    let Some(course) = notebook
        .courses
        .iter()
        .find(|course| course.id == action.value("courseId") && course.open)
    else {
        return Ok(unavailable(lang));
    };
    let mut date = parse_date(action.value("date")).unwrap_or_else(today);
    let return_to_roll = || {
        Action::new("nb.roll")
            .arg("courseId", &course.id)
            .arg("date", date.to_string())
    };
    if action.route == "nb.date" {
        let parsed = input.and_then(parse_date);
        match parsed {
            Some(parsed) => date = parsed,
            None => {
                let text = if input.is_some() {
                    lang.text(
                        "Invalid date. Send YYYY-MM-DD.",
                        "تاريخ غير صالح. أرسل YYYY-MM-DD.",
                    )
                } else {
                    lang.text(
                        "Send date as YYYY-MM-DD.",
                        "أرسل التاريخ بالصيغة YYYY-MM-DD.",
                    )
                };
                return Ok(
                    Screen::new(text, vec![vec![back(lang, return_to_roll()), home(lang)]]).prompt(
                        Action::new("nb.date")
                            .arg("courseId", &course.id)
                            .arg("date", date.to_string()),
                    ),
                );
            }
        }
    }
    let mut saved = None;
    if action.route == "nb.mark" {
        if !tools.contains("sync_attendance") {
            return Ok(unavailable(lang));
        }
        let status = match action.value("status") {
            "present" => Status::Present,
            "absent" => Status::Absent,
            "excused" => Status::Excused,
            _ => return Ok(unavailable(lang)),
        };
        let Some(entry) = notebook.enrollments.iter().find(|entry| {
            entry.id == action.value("enrollmentId") && entry.course_id == course.id && entry.active
        }) else {
            return Ok(unavailable(lang));
        };
        let Some(student) = notebook
            .students
            .iter()
            .find(|student| student.student_id == entry.student_id && student.active)
        else {
            return Ok(unavailable(lang));
        };
        let id = action.value("eventId");
        if !id_shape(id)
            || chrono::DateTime::parse_from_rfc3339(action.value("time")).is_err()
            || parse_date(action.value("date")).is_none()
        {
            return Ok(unavailable(lang));
        }
        let expected = action
            .args
            .get("expected")
            .ok_or("missing attendance version")?;
        let response: SyncResponse = serde_json::from_value(mcp.call("sync_attendance", json!({
            "records": [{
                "event": { "id": id, "clientTime": action.value("time"), "payload": {
                    "kind": "attendance", "studentId": entry.student_id, "enrollmentId": entry.id,
                    "date": date.to_string(), "status": status.wire()
                }},
                "expectedEventId": if expected.is_empty() { None } else { Some(expected) }
            }]
        })).await?)?;
        if !response.outcomes.iter().any(|outcome| {
            outcome.id == id && matches!(outcome.kind.as_str(), "saved" | "unchanged")
        }) {
            return Ok(Screen::new(lang.text(
                "Attendance changed or could not be saved. Refresh and review before trying again.",
                "تغير الحضور أو تعذر حفظه. حدّث وراجع قبل المحاولة مجدداً."
            ), vec![vec![back(lang, Action::new("nb.roll").arg("courseId", &course.id).arg("date", date.to_string())), home(lang)]]));
        }
        saved = Some(format!(
            "{}: {} · {}\n",
            lang.text("Attendance saved", "تم حفظ الحضور"),
            short(&student.name, 100),
            status.text(lang)
        ));
    }
    let next;
    let notebook = if saved.is_some() {
        next = serde_json::from_value(mcp.call("get_notebook", json!({})).await?)?;
        &next
    } else {
        &notebook
    };
    let course_id = course.id.as_str();
    let mut screen = roll_screen(
        lang,
        action,
        date,
        course_id,
        notebook,
        tools.contains("sync_attendance"),
    )?;
    if let Some(saved) = saved {
        screen.text.insert_str(0, &saved);
    }
    Ok(screen)
}

fn roll_screen<'a>(
    lang: Language,
    action: &Action,
    date: NaiveDate,
    course_id: &str,
    notebook: &'a Notebook,
    writable: bool,
) -> Result<Screen> {
    let Some(course) = notebook
        .courses
        .iter()
        .find(|course| course.id == course_id && course.open)
    else {
        return Ok(unavailable(lang));
    };
    let entries: Vec<_> = notebook
        .enrollments
        .iter()
        .filter(|entry| entry.course_id == course.id && entry.active)
        .filter_map(|entry| {
            notebook
                .students
                .iter()
                .find(|student| student.active && student.student_id == entry.student_id)
                .map(|student| (entry, student))
        })
        .collect();
    let marks = projected_marks(&notebook.events);
    let mark_for = |entry: &'a Enrollment| {
        marks.get(&(entry.student_id.as_str(), Some(entry.id.as_str()), date))
    };
    let index = page(action);
    let base = |route| {
        Action::new(route)
            .arg("courseId", &course.id)
            .arg("date", date.to_string())
    };
    let mut rows = Vec::new();
    for (entry, student) in entries.iter().skip(PAGE * index).take(PAGE) {
        let current = mark_for(entry);
        let status = current
            .and_then(|(_, status)| *status)
            .map_or("—", |status| status.text(lang));
        rows.push(vec![choice(
            &format!("{} · {status}", student.name),
            Action::new("nb.student").arg("studentId", &student.student_id),
        )]);
        if writable {
            let mut row = Vec::new();
            for status in [Status::Present, Status::Absent, Status::Excused] {
                row.push(choice(
                    status.text(lang),
                    fresh(
                        base("nb.mark")
                            .arg("enrollmentId", &entry.id)
                            .arg("status", status.wire())
                            .arg("expected", current.map_or("", |(id, _)| *id))
                            .arg("page", index.to_string()),
                    )?,
                ));
            }
            rows.push(row);
        }
    }
    rows.extend(nav(lang, base("nb.roll"), index, entries.len()));
    rows.push(vec![choice(
        lang.text("Change date", "تغيير التاريخ"),
        base("nb.date"),
    )]);
    rows.push(vec![back(lang, Action::new("nb.attendance")), home(lang)]);
    let marked = entries
        .iter()
        .filter(|(entry, _)| mark_for(entry).is_some_and(|(_, mark)| mark.is_some()))
        .count();
    Ok(Screen::new(
        format!(
            "{} · {date}\n{} {marked}/{}{}",
            short(&course.name, 200),
            lang.text("Marked", "تم تسجيل"),
            entries.len(),
            if entries.is_empty() {
                lang.text(
                    "\nNo active enrolled students.",
                    "\nلا يوجد طلاب مسجلون نشطون.",
                )
            } else {
                ""
            }
        ),
        rows,
    ))
}

async fn note(
    mcp: &Mcp<'_>,
    tools: &HashSet<String>,
    lang: Language,
    action: &Action,
    input: Option<&str>,
    notebook: &Notebook,
) -> Result<Screen> {
    let Some(student) = notebook
        .students
        .iter()
        .find(|student| student.student_id == action.value("studentId") && student.active)
    else {
        return Ok(unavailable(lang));
    };
    if !tools.contains("append_events") {
        return Ok(unavailable(lang));
    }
    let target = |route| Action::new(route).arg("studentId", &student.student_id);
    let back_rows = || vec![vec![back(lang, target("nb.student")), home(lang)]];
    let valid_tags = |tags: &[String]| {
        !tags.is_empty()
            && tags.len() <= 20
            && tags
                .iter()
                .all(|tag| !tag.is_empty() && tag.encode_utf16().count() <= 60)
    };
    match (action.route.as_str(), input) {
        ("nb.note.tags", _) | (_, None) => Ok(Screen::new(
            lang.text(
                "Send tags separated by commas.",
                "أرسل الوسوم مفصولة بفواصل.",
            ),
            back_rows(),
        )
        .prompt(target("nb.note.text"))),
        ("nb.note.text", Some(text)) => {
            let mut seen = HashSet::new();
            let mut tags: Vec<_> = text
                .split([',', '،'])
                .map(|tag| {
                    tag.trim()
                        .strip_prefix('#')
                        .unwrap_or(tag.trim())
                        .to_lowercase()
                })
                .filter(|tag| !tag.is_empty() && seen.insert(tag.clone()))
                .collect();
            if tags.is_empty() {
                tags.push("attendance".into());
            }
            if !valid_tags(&tags) {
                return Ok(Screen::new(
                    lang.text(
                        "Use at most 20 tags, each up to 60 characters.",
                        "استخدم ٢٠ وسماً كحد أقصى، كل منها حتى ٦٠ حرفاً.",
                    ),
                    back_rows(),
                )
                .prompt(target("nb.note.text")));
            }
            let next = fresh(
                target("nb.note.save")
                    .arg("tags", serde_json::to_string(&tags)?)
                    .arg("date", today().to_string()),
            )?;
            Ok(Screen::new(
                lang.text("Send the note text.", "أرسل نص الملاحظة."),
                back_rows(),
            )
            .prompt(next))
        }
        ("nb.note.save", Some(text)) => {
            if text.is_empty() || text.encode_utf16().count() > 10_000 {
                return Ok(Screen::new(
                    lang.text(
                        "Enter note text of 1–10000 characters.",
                        "أدخل ملاحظة من ١ إلى ١٠٠٠٠ حرف.",
                    ),
                    back_rows(),
                )
                .prompt(action.clone()));
            }
            let tags: Vec<String> = serde_json::from_str(action.value("tags"))?;
            if !valid_tags(&tags)
                || parse_date(action.value("date")).is_none()
                || !valid_event(action)
            {
                return Ok(unavailable(lang));
            }
            mcp.call("append_events", json!({ "events": [{
                "id": action.value("eventId"), "clientTime": action.value("time"),
                "payload": { "kind": "note", "studentId": student.student_id, "date": action.value("date"), "tags": tags, "text": text },
            }] })).await?;
            Ok(Screen::new(
                lang.text("Note saved.", "تم حفظ الملاحظة."),
                back_rows(),
            ))
        }
        _ => Ok(unavailable(lang)),
    }
}

fn valid_event(action: &Action) -> bool {
    id_shape(action.value("eventId"))
        && chrono::DateTime::parse_from_rfc3339(action.value("time")).is_ok()
}

async fn course(
    mcp: &Mcp<'_>,
    tools: &HashSet<String>,
    lang: Language,
    action: &Action,
    input: Option<&str>,
    notebook: &Notebook,
) -> Result<Screen> {
    let index = page(action);
    if action.route == "co.search" && input.is_none() {
        return Ok(Screen::new(
            lang.text(
                "Send a name to search, or /cancel.",
                "أرسل اسماً للبحث، أو /cancel للإلغاء.",
            ),
            vec![vec![back(lang, Action::new("co.list")), home(lang)]],
        )
        .prompt(Action::new("co.search")));
    }
    if matches!(action.route.as_str(), "co.list" | "co.search") {
        let query = short(input.unwrap_or_else(|| action.value("q")).trim(), 200);
        let courses = rank_by_name(&notebook.courses, &query, |course| &course.name);
        let mut rows: Vec<_> = courses
            .iter()
            .skip(index * PAGE)
            .take(PAGE)
            .map(|course| {
                vec![choice(
                    &format!(
                        "{}{}",
                        course.name,
                        if course.open {
                            ""
                        } else {
                            lang.text(" (closed)", " (مغلقة)")
                        }
                    ),
                    Action::new("co.view").arg("id", &course.id),
                )]
            })
            .collect();
        rows.extend(nav(
            lang,
            Action::new("co.list").arg("q", query),
            index,
            courses.len(),
        ));
        rows.push(vec![choice(
            lang.text("Search", "بحث"),
            Action::new("co.search"),
        )]);
        if tools.contains("save_course") {
            rows.push(vec![choice(
                lang.text("Create course", "إنشاء دورة"),
                Action::new("co.name").arg("id", uuid()?),
            )]);
        }
        rows.push(vec![home(lang)]);
        return Ok(Screen::new(lang.text("Courses", "الدورات"), rows));
    }
    let Some(course) = notebook
        .courses
        .iter()
        .find(|course| course.id == action.value("id"))
    else {
        return Ok(unavailable(lang));
    };
    let route = |name| Action::new(name).arg("id", &course.id);
    let report_tool = "export_course_report";
    let reports = [
        ("full", lang.text("Full report", "التقرير الكامل")),
        ("grades", lang.text("Grades", "الدرجات")),
        ("students", lang.text("Students", "الطلاب")),
        ("attendance", lang.text("Daily attendance", "الحضور اليومي")),
        (
            "participants",
            lang.text("Exam participants", "المشاركون بالامتحان"),
        ),
    ];
    if matches!(action.route.as_str(), "co.reports" | "co.report") {
        if !tools.contains(report_tool) {
            return Ok(unavailable(lang));
        }
        if action.route == "co.reports" {
            let mut rows: Vec<_> = reports
                .iter()
                .map(|(key, label)| vec![choice(label, route("co.report").arg("report", *key))])
                .collect();
            rows.push(vec![back(lang, route("co.view")), home(lang)]);
            return Ok(Screen::new(
                lang.text("Choose an Excel report.", "اختر تقرير Excel."),
                rows,
            ));
        }
        let report = action.value("report");
        if !reports.iter().any(|(key, _)| *key == report) {
            return Ok(unavailable(lang));
        }
        let result = mcp.call(report_tool, json!({ "courseId": course.id, "report": report, "format": "xlsx", "language": lang.text("en", "ar") })).await?;
        let blob = result["blob"].as_str().ok_or("missing export content")?;
        use base64::{Engine, engine::general_purpose::STANDARD};
        let mut screen = Screen::new(
            lang.text("Your report is ready.", "التقرير جاهز."),
            vec![vec![back(lang, route("co.reports")), home(lang)]],
        );
        screen.document = Some((format!("{report}.xlsx"), STANDARD.decode(blob)?));
        return Ok(screen);
    }
    let mut rows = vec![vec![choice(
        lang.text("Students", "الطلاب"),
        route("co.members"),
    )]];
    if tools.contains("get_admin_board") {
        rows.push(vec![choice(
            lang.text("Readiness", "جاهزية النتائج"),
            route("co.readiness"),
        )]);
    }
    if tools.contains(report_tool) {
        rows.push(vec![choice(
            lang.text("Reports", "التقارير"),
            route("co.reports"),
        )]);
    }
    for (tool, label, destination) in [
        ("save_course", lang.text("Rename", "تعديل الاسم"), "co.name"),
        (
            "set_course_thresholds",
            lang.text("Thresholds", "حدود النجاح"),
            "co.thresholds",
        ),
        (
            "set_course_status",
            if course.open {
                lang.text("Close course", "إغلاق الدورة")
            } else {
                lang.text("Reopen course", "إعادة فتح الدورة")
            },
            "co.status",
        ),
    ] {
        if tools.contains(tool) {
            rows.push(vec![choice(label, route(destination))]);
        }
    }
    rows.push(vec![back(lang, Action::new("co.list")), home(lang)]);
    Ok(Screen::new(
        format!(
            "{}\n{}",
            short(&course.name, 200),
            if course.open {
                lang.text("Open", "مفتوحة")
            } else {
                lang.text("Closed", "مغلقة")
            }
        ),
        rows,
    ))
}

async fn history(
    mcp: &Mcp<'_>,
    tools: &HashSet<String>,
    lang: Language,
    action: &Action,
    input: Option<&str>,
    notebook: &Notebook,
) -> Result<Screen> {
    let Some(student) = notebook
        .students
        .iter()
        .find(|student| student.student_id == action.value("studentId"))
    else {
        return Ok(unavailable(lang));
    };
    let (ordered, reversed) = event_order(&notebook.events);
    let history: Vec<_> = ordered
        .into_iter()
        .rev()
        .filter(|event| match &event.payload {
            Payload::Attendance { student_id, .. } | Payload::Note { student_id, .. } => {
                student_id == &student.student_id
            }
            _ => false,
        })
        .collect();
    let target = |route| Action::new(route).arg("studentId", &student.student_id);
    let description = |event: &Event| match &event.payload {
        Payload::Attendance { date, status, .. } => format!(
            "{date} · {}",
            status.map_or(
                lang.text("Attendance cleared", "تم إلغاء علامة الحضور"),
                |status| lang.text(status.wire(), status.text(lang)),
            )
        ),
        Payload::Note { date, text, .. } => format!("{date} · {}", short(text, 70)),
        _ => String::new(),
    };
    let index = page(action);
    if action.route == "nb.history" {
        let mut rows: Vec<_> = history
            .iter()
            .skip(index * PAGE)
            .take(PAGE)
            .map(|event| {
                let marker = if reversed.contains(event.id.as_str()) {
                    "↩ "
                } else {
                    ""
                };
                vec![choice(
                    &format!("{marker}{}", description(event)),
                    target("nb.history.item").arg("targetId", &event.id),
                )]
            })
            .collect();
        rows.extend(nav(lang, target("nb.history"), index, history.len()));
        rows.push(vec![back(lang, target("nb.student")), home(lang)]);
        let empty = if history.is_empty() {
            lang.text("\nNo records.", "\nلا توجد سجلات.")
        } else {
            ""
        };
        return Ok(Screen::new(
            format!(
                "{} · {}{empty}",
                lang.text("History", "السجل"),
                short(&student.name, 200)
            ),
            rows,
        ));
    }
    let Some(event) = history
        .iter()
        .find(|event| event.id == action.value("targetId"))
    else {
        return Ok(unavailable(lang));
    };
    let item = |route| target(route).arg("targetId", &event.id);
    let corrected = reversed.contains(event.id.as_str());
    let can_correct = !corrected && tools.contains("append_events");
    if action.route == "nb.history.item" {
        let mut rows = Vec::new();
        let mut text = if let Payload::Note {
            date, tags, text, ..
        } = &event.payload
        {
            const CHUNK: usize = 2000;
            if index > 0 {
                rows.push(vec![choice(
                    lang.text("Previous", "السابق"),
                    item("nb.history.item").arg("page", (index - 1).to_string()),
                )]);
            }
            if (index + 1) * CHUNK < text.chars().count() {
                rows.push(vec![choice(
                    lang.text("Next", "التالي"),
                    item("nb.history.item").arg("page", (index + 1).to_string()),
                )]);
            }
            let body: String = text.chars().skip(index * CHUNK).take(CHUNK).collect();
            let tags = tags
                .iter()
                .map(|tag| format!("#{tag}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{date}\n{tags}\n{body}")
        } else {
            description(event)
        };
        if corrected {
            text.push_str(lang.text("\nCorrected", "\nتم التصحيح"));
        }
        if can_correct {
            rows.push(vec![choice(
                lang.text("Correct", "تصحيح"),
                item("nb.reverse.reason"),
            )]);
        }
        rows.push(vec![back(lang, target("nb.history")), home(lang)]);
        return Ok(Screen::new(text, rows));
    }
    if !can_correct {
        return Ok(unavailable(lang));
    }
    let reason_valid = |reason: &str| !reason.is_empty() && reason.encode_utf16().count() <= 1000;
    if action.route == "nb.reverse.reason" {
        let rows = || vec![vec![back(lang, item("nb.history.item")), home(lang)]];
        let Some(reason) = input.filter(|reason| reason_valid(reason)) else {
            let text = if input.is_none() {
                lang.text("Send the correction reason.", "أرسل سبب التصحيح.")
            } else {
                lang.text(
                    "Enter a reason of 1–1000 characters.",
                    "أدخل سبباً من ١ إلى ١٠٠٠ حرف.",
                )
            };
            return Ok(Screen::new(text, rows()).prompt(item("nb.reverse.reason")));
        };
        let mut rows = rows();
        rows.insert(
            0,
            vec![choice(
                lang.text("Confirm", "تأكيد"),
                fresh(item("nb.reverse.save").arg("reason", reason))?,
            )],
        );
        return Ok(Screen::new(
            format!(
                "{}\n{}\n{reason}",
                description(event),
                lang.text("Confirm correction?", "تأكيد التصحيح؟")
            ),
            rows,
        ));
    }
    let reason = action.value("reason");
    if !reason_valid(reason) || !valid_event(action) {
        return Ok(unavailable(lang));
    }
    mcp.call(
        "append_events",
        json!({ "events": [{
        "id": action.value("eventId"), "clientTime": action.value("time"),
        "payload": { "kind": "reversal", "targetId": event.id, "reason": reason },
    }] }),
    )
    .await?;
    Ok(Screen::new(
        lang.text("Correction saved.", "تم حفظ التصحيح."),
        vec![vec![back(lang, target("nb.history")), home(lang)]],
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Overview {
    date: NaiveDate,
    recent_dates: Vec<NaiveDate>,
    session_counts: SessionCounts,
    follow_up: Vec<FollowUp>,
}

#[derive(Deserialize)]
struct SessionCounts {
    marked: usize,
    unmarked: usize,
}

#[derive(Deserialize)]
struct FollowUp {
    name: String,
    absences: usize,
    marks: Vec<OverviewMark>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum OverviewMark {
    Present,
    Absent,
    Excused,
    Unmarked,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chart {
    summary_url: Url,
    chart: Vec<ChartRow>,
}

#[derive(Deserialize)]
struct ChartRow {
    label: String,
    rate: Option<f64>,
    evidence: String,
}

async fn insights(mcp: &Mcp<'_>, origin: &Url, lang: Language, action: &Action) -> Result<Screen> {
    let id = action.value("courseId");
    let course_id = if id == "previous" {
        "none"
    } else if id_shape(id) {
        id
    } else {
        return Ok(unavailable(lang));
    };
    let date = parse_date(action.value("date")).unwrap_or_else(today);
    let is_overview = action.route == "nb.followup";
    let view = if is_overview { "overview" } else { "chart" };
    let data = mcp.call("get_course_attendance_view", json!({
        "courseId": course_id, "date": date.to_string(), "view": view, "language": lang.text("en", "ar"),
    })).await?;
    let index = page(action);
    let (mut lines, len, url) = if is_overview {
        let data: Overview = serde_json::from_value(data)?;
        if data.recent_dates.len() > 6 || data.follow_up.iter().any(|row| row.marks.len() > 6) {
            return Err("invalid attendance overview".into());
        }
        let mut lines = vec![
            format!("{} · {}", lang.text("Follow-up", "المتابعة"), data.date),
            format!(
                "{} {} · {} {}",
                lang.text("Marked", "تم تسجيل"),
                data.session_counts.marked,
                lang.text("Unmarked", "لم يسجل"),
                data.session_counts.unmarked
            ),
            format!(
                "{}: {}",
                lang.text("Recent sessions", "الجلسات الأخيرة"),
                if data.recent_dates.is_empty() {
                    lang.text("No recorded sessions.", "لا توجد جلسات مسجلة.")
                        .into()
                } else {
                    data.recent_dates
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" · ")
                }
            ),
        ];
        if data.follow_up.is_empty() {
            lines.push(
                lang.text(
                    "No students need follow-up.",
                    "لا يوجد طلاب يحتاجون للمتابعة.",
                )
                .into(),
            );
        } else {
            lines.push(
                lang.text(
                    "✓ Present · ✗ Absent · ○ Excused · – Unmarked",
                    "✓ حاضر · ✗ غائب · ○ معذور · – لم يسجل",
                )
                .into(),
            );
        }
        for item in data.follow_up.iter().skip(index * PAGE).take(PAGE) {
            let marks = item
                .marks
                .iter()
                .map(|mark| match mark {
                    OverviewMark::Present => "✓",
                    OverviewMark::Absent => "✗",
                    OverviewMark::Excused => "○",
                    OverviewMark::Unmarked => "–",
                })
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(format!(
                "{} · {}: {}\n{marks}",
                short(&item.name, 100),
                lang.text("Absences", "الغياب"),
                item.absences
            ));
        }
        (lines, data.follow_up.len(), None)
    } else {
        let data: Chart = serde_json::from_value(data)?;
        if data.summary_url.origin() != origin.origin()
            || !data.summary_url.username().is_empty()
            || data.summary_url.password().is_some()
            || data
                .chart
                .iter()
                .any(|row| row.rate.is_some_and(|rate| !(0.0..=1.0).contains(&rate)))
        {
            return Err("invalid attendance chart".into());
        }
        let mut lines = vec![lang.text("Attendance chart", "مخطط الحضور").into()];
        for item in data.chart.iter().skip(index * PAGE).take(PAGE) {
            let filled = (item.rate.unwrap_or_default() * 10.0).round() as usize;
            lines.push(format!(
                "{} · {}\n{}{}",
                short(&item.label, 60),
                short(&item.evidence, 80),
                "█".repeat(filled),
                "░".repeat(10 - filled)
            ));
        }
        (lines, data.chart.len(), Some(data.summary_url))
    };
    let mut rows = nav(
        lang,
        Action::new(&action.route)
            .arg("courseId", id)
            .arg("date", date.to_string()),
        index,
        len,
    );
    if let Some(url) = url {
        if lines.len() == 1 {
            lines.push(
                lang.text("No students to chart.", "لا يوجد طلاب للمخطط.")
                    .into(),
            );
        }
        rows.push(vec![Button::Link(
            lang.text("Open chart and export", "فتح المخطط والتصدير")
                .into(),
            url,
        )]);
    }
    rows.push(vec![
        back(lang, Action::new("nb.summary.course").arg("courseId", id)),
        home(lang),
    ]);
    Ok(Screen::new(lines.join("\n"), rows))
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    if value.len() != 10 {
        return None;
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    (date.to_string() == value).then_some(date)
}

fn today() -> NaiveDate {
    // Damascus has used UTC+03 year-round since 2022. This is the default for new records.
    Utc::now()
        .with_timezone(&FixedOffset::east_opt(10_800).expect("UTC+03 offset"))
        .date_naive()
}

fn fresh(action: Action) -> Result<Action> {
    Ok(action.arg("eventId", uuid()?).arg(
        "time",
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    ))
}

fn uuid() -> Result<String> {
    let mut bytes = random::<16>()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn id_shape(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
