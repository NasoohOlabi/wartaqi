use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

use super::{
    Allowed, Draft, Labels, Language, Name, Reply, Turn, WRITE_INTENT, bounded, explicit_date,
    matches, normalize_digits, schema, tools, unavailable, unsupported,
};
use crate::auth::Result;

#[derive(Deserialize)]
struct Course {
    id: String,
    name: String,
    open: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Present,
    Absent,
    Excused,
    Unmarked,
}

impl Status {
    fn text(self, lang: Language) -> &'static str {
        match self {
            Self::Present => lang.text("Present", "حاضر"),
            Self::Absent => lang.text("Absent", "غائب"),
            Self::Excused => lang.text("Excused", "معذور"),
            Self::Unmarked => lang.text("Unmarked", "لم يُسجل"),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    name: String,
    status: Status,
    student_id: Option<String>,
    enrollment_id: Option<String>,
    event_id: Option<String>,
}

#[derive(Deserialize)]
struct Session {
    course: Option<Course>,
    date: String,
    session: Vec<Row>,
}

impl Session {
    fn parse(raw: &Value) -> Result<Self> {
        let session: Self = serde_json::from_value(raw.clone())?;
        if !schema::date(&session.date)
            || session.session.len() > 10_000
            || session.session.iter().any(|row| !valid_name(&row.name))
            || session
                .course
                .as_ref()
                .is_some_and(|course| !schema::uuid(&course.id) || !valid_name(&course.name))
        {
            return Err("invalid attendance source".into());
        }
        Ok(session)
    }
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 200 && !name.contains(char::is_control)
}

fn normalized(value: &str) -> String {
    value
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn reply(lang: Language, en: &str, ar: &str) -> Reply {
    Reply::Answer(lang.text(en, ar).into())
}

fn tool(tools: &[Allowed], name: Name) -> Option<&Allowed> {
    tools.iter().find(|tool| tool.name == name)
}

async fn courses(catalog: &[Allowed], turn: &mut Turn<'_>) -> Result<Vec<Course>> {
    let list = tool(catalog, Name::ListCourses).ok_or("list courses unavailable")?;
    let courses: Vec<Course> = serde_json::from_value(turn.read(list, json!({})).await?)?;
    if courses.len() > 10_000
        || courses
            .iter()
            .any(|course| !schema::uuid(&course.id) || !valid_name(&course.name))
    {
        return Err("invalid course list".into());
    }
    Ok(courses)
}

fn one_course(courses: &[Course], name: &str) -> Option<usize> {
    let name = normalized(name);
    let mut matches = courses
        .iter()
        .enumerate()
        .filter(|(_, course)| normalized(&course.name) == name);
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

fn args(course: &Course, date: &str, lang: Language) -> Value {
    json!({"courseId": course.id, "date": date, "view": "session", "language": lang.text("en", "ar")})
}

fn check_session(session: &Session, selected: &Course, date: &str) -> Result<()> {
    if session.date != date
        || !session.course.as_ref().is_some_and(|course| {
            course.id == selected.id && course.name == selected.name && course.open == selected.open
        })
    {
        return Err("attendance source changed".into());
    }
    Ok(())
}

pub(super) async fn plan(
    question: &str,
    lang: Language,
    catalog: &[Allowed],
    turn: &mut Turn<'_>,
) -> Option<Reply> {
    let en = schema::regex(
        r"(?i)^mark\s+(.+?)\s+(present|absent|excused)\s+in\s+(.+?)(?:\s+on\s+([0-9]{4}-[0-9]{2}-[0-9]{2}))?[.!]?$",
    )?;
    let ar = schema::regex(
        r"^(?:سجل|سجّل|حط)\s+(.+?)\s+(حاضر|حاضرة|غائب|غائبة|غايب|معذور)\s+في\s+(?:دورة\s+)?(.+?)(?:\s+(?:في|بتاريخ|يوم)\s+([0-9]{4}-[0-9]{2}-[0-9]{2}))?[.!؟]?$",
    )?;
    let question = normalize_digits(question);
    let request = en.captures(&question).or_else(|| ar.captures(&question))?;
    let student = normalized(&request[1]);
    let status = match request[2].to_lowercase().as_str() {
        "present" | "حاضر" | "حاضرة" => "present",
        "absent" | "غائب" | "غائبة" | "غايب" => "absent",
        _ => "excused",
    };
    let Some(date) = request
        .get(4)
        .map(|date| date.as_str())
        .filter(|date| schema::date(date))
    else {
        return Some(reply(
            lang,
            "No change saved. Give the date as YYYY-MM-DD.",
            "لم يُحفظ أي تغيير. أدخل التاريخ بصيغة YYYY-MM-DD.",
        ));
    };
    let course_name = request[3].trim();
    let (Some(view), Some(sync), Some(_)) = (
        tool(catalog, Name::GetCourseAttendanceView),
        tool(catalog, Name::SyncAttendance),
        tool(catalog, Name::ListCourses),
    ) else {
        return Some(reply(
            lang,
            "No change saved. Attendance access is unavailable; use /menu.",
            "لم يُحفظ أي تغيير. صلاحية الحضور غير متاحة؛ استخدم /menu.",
        ));
    };
    let result = async {
        let courses = courses(catalog, turn).await?;
        let Some(index) = one_course(&courses, course_name) else {
            return Ok(reply(lang, "No change saved. The course is missing or ambiguous; provide its exact name.", "لم يُحفظ أي تغيير. الدورة غير موجودة أو اسمها غير محدد؛ أدخل اسمها الكامل."));
        };
        let course = &courses[index];
        if !course.open {
            return Ok(reply(lang, "No change saved. This course is closed.", "لم يُحفظ أي تغيير. هذه الدورة مغلقة."));
        }
        let raw = turn.read(view, args(course, date, lang)).await?;
        let session = Session::parse(&raw)?;
        check_session(&session, course, date)?;
        let mut hits = session.session.iter().filter(|row| normalized(&row.name) == student);
        let Some(row) = hits.next().filter(|_| hits.next().is_none()) else {
            return Ok(reply(lang, "No change saved. The student is missing or ambiguous in this course; provide the exact name.", "لم يُحفظ أي تغيير. الطالب غير موجود أو اسمه غير محدد في الدورة؛ أدخل الاسم الكامل."));
        };
        let draft = tools::prepare(Name::SyncAttendance, json!({"records":[{
            "event":{"payload":{"kind":"attendance","studentId":row.student_id,"enrollmentId":row.enrollment_id,"date":date,"status":status}},
            "expectedEventId":row.event_id
        }]}))?;
        sync.validate(&draft)?;
        if !matches_source(&draft, &raw) {
            return Err("attendance source did not match".into());
        }
        let mut labels = Labels::new();
        if let Some(result) = tools::safe_result(&raw) {
            tools::collect_labels(&result, &mut labels, 0, "");
        }
        // A direct request can target a student outside the first 30 model-visible rows.
        if let (Some(student), Some(enrollment)) = (&row.student_id, &row.enrollment_id) {
            labels.insert(student.clone(), row.name.clone());
            labels.insert(enrollment.clone(), bounded(&format!("{} · {}", row.name, course.name), 200));
        }
        let text = tools::preview(Name::SyncAttendance, &draft, lang, &labels)?;
        Result::Ok(Reply::Approval { text, draft: Draft { name: Name::SyncAttendance, args: draft } })
    }.await;
    Some(result.unwrap_or_else(|_| {
        reply(
            lang,
            "No change saved. Could not verify the current attendance; try again or use /menu.",
            "لم يُحفظ أي تغيير. تعذر التحقق من الحضور؛ حاول مجدداً أو استخدم /menu.",
        )
    }))
}

pub(super) fn matches_source(args: &Value, raw: &Value) -> bool {
    let Some(records) = args["records"].as_array() else {
        return false;
    };
    let Ok(session) = Session::parse(raw) else {
        return false;
    };
    let Some(rows) = raw["session"].as_array() else {
        return false;
    };
    if records.is_empty() || !session.course.is_some_and(|course| course.open) {
        return false;
    }
    let mut targets = HashSet::new();
    for record in records {
        let payload = &record["event"]["payload"];
        let Some(enrollment) = payload["enrollmentId"]
            .as_str()
            .filter(|id| schema::uuid(id))
        else {
            return false;
        };
        let Some(student) = payload["studentId"].as_str().filter(|id| schema::uuid(id)) else {
            return false;
        };
        let Some(expected) = record.get("expectedEventId") else {
            return false;
        };
        if payload["date"] != session.date
            || !targets.insert(enrollment)
            || !(expected.is_null() || expected.as_str().is_some_and(schema::uuid))
        {
            return false;
        }
        let mut exact = rows
            .iter()
            .filter(|row| row["studentId"] == student && row["enrollmentId"] == enrollment);
        let Some(row) = exact.next() else {
            return false;
        };
        if exact.next().is_some()
            || row.get("eventId") != Some(expected)
            || expected.is_null() && row["status"] != "unmarked"
        {
            return false;
        }
    }
    true
}

pub(super) fn verify_read(args: &Value, raw: &Value) -> Result<()> {
    if args.get("view").is_none_or(|view| view == "session") {
        let session = Session::parse(raw)?;
        if args["date"] != session.date
            || (if let Some(course) = session.course {
                args["courseId"] != course.id
            } else {
                args["courseId"] != "none"
            })
        {
            return Err("attendance source differs from request".into());
        }
    }
    Ok(())
}

pub(super) fn render(raw: &Value, lang: Language) -> Option<String> {
    let data = Session::parse(raw).ok()?;
    let mut lines = vec![match data.course {
        Some(course) => format!("{} — {}", course.name, data.date),
        None => data.date,
    }];
    if data.session.is_empty() {
        lines.push(
            lang.text("No students in this session.", "لا يوجد طلاب.")
                .into(),
        );
    }
    for row in data.session.iter().take(30) {
        lines.push(format!("{}: {}", row.name, row.status.text(lang)));
    }
    if data.session.len() > 30 {
        lines.push(
            lang.text("Open /menu to see the rest.", "افتح /menu لعرض البقية.")
                .into(),
        );
    }
    Some(bounded(&lines.join("\n"), 3500))
}

#[derive(Deserialize)]
struct OwnAttendance {
    attendance: Vec<OwnRow>,
}

#[derive(Deserialize)]
struct OwnRow {
    date: String,
    course: Option<String>,
    status: Status,
}

fn own_counts(data: &OwnAttendance) -> Result<[u32; 3]> {
    let mut counts = [0u32; 3];
    for row in &data.attendance {
        if !schema::date(&row.date) {
            return Err("invalid attendance date".into());
        }
        counts[match row.status {
            Status::Present => 0,
            Status::Absent => 1,
            Status::Excused => 2,
            Status::Unmarked => return Err("invalid own attendance mark".into()),
        }] += 1;
    }
    Ok(counts)
}

pub(super) fn model_own_result(raw: &Value) -> Result<Value> {
    let data: OwnAttendance = serde_json::from_value(raw.clone())?;
    let [present, absent, excused] = own_counts(&data)?;
    let mut safe = tools::safe_result(raw).ok_or("invalid own attendance")?;
    safe["totals"] = json!({"present":present,"absent":absent,"excused":excused});
    Ok(safe)
}

fn own_result(raw: Value, lang: Language) -> Result<Reply> {
    let mut data: OwnAttendance = serde_json::from_value(raw)?;
    let counts = own_counts(&data)?;
    data.attendance
        .sort_by(|left, right| right.date.cmp(&left.date));
    let statuses = [Status::Present, Status::Absent, Status::Excused];
    let totals = statuses
        .into_iter()
        .zip(counts)
        .map(|(status, count)| format!("{}: {count}", status.text(lang)))
        .collect::<Vec<_>>()
        .join(" | ");
    let mut lines = vec![
        lang.text("Your attendance summary", "ملخص حضورك").into(),
        totals,
    ];
    for row in data.attendance.into_iter().take(5) {
        let course = row
            .course
            .filter(|name| valid_name(name))
            .map(|name| format!(" — {name}"))
            .unwrap_or_default();
        lines.push(format!("{}{course}: {}", row.date, row.status.text(lang)));
    }
    Ok(Reply::Answer(lines.join("\n")))
}

pub(super) async fn fallback(
    question: &str,
    lang: Language,
    catalog: &[Allowed],
    turn: &mut Turn<'_>,
) -> Reply {
    let attendance = matches(
        r"(?i)\b(attendance|absent|absence|present|excused)\b|حضور|غائب|غياب|حاضر|معذور",
        question,
    );
    if matches(WRITE_INTENT, question) {
        return reply(
            lang,
            "No change was saved. Clarify the record and change, or use /menu.",
            "لم يُحفظ أي تغيير. وضّح السجل والتغيير المطلوب أو استخدم /menu.",
        );
    }
    let result = async {
        if let Some(own) = tool(catalog, Name::GetMyAttendance)
            && matches(r"(?i)\b(my (?:attendance|absences?)|was i|am i|have i)\b|حضوري|غيابي|كنت حاضر|كنت غايب", question)
        {
            if explicit_date(question).is_some() || matches(r"(?i)\b(today|yesterday|tomorrow|week|month|course|class|on|for)\b|اليوم|أمس|امس|أسبوع|شهر|دورة", question) {
                return Ok(reply(lang, "Open /menu, then My attendance, to check the full record for that course or date.", "افتح /menu ثم حضوري لعرض السجل حسب الدورة أو التاريخ."));
            }
            return own_result(turn.read(own, json!({})).await?, lang);
        }
        if let Some(view) = tool(catalog, Name::GetCourseAttendanceView)
            && let Some(text) = schema::regex(r"(?i)\battendance for (?:course )?(.+?) on ([0-9]{4}-[0-9]{2}-[0-9]{2})\b")
            && let Some(captures) = text.captures(question)
        {
            let selected = courses(catalog, turn).await?;
            let Some(course) = one_course(&selected, &captures[1]).map(|index| &selected[index]) else {
                return Ok(reply(lang, "Could not find one matching course. Open /menu and select the course.", "لم أجد دورة مطابقة واحدة. افتح /menu واختر الدورة."));
            };
            let args = args(course, &captures[2], lang);
            let raw = turn.read(view, args.clone()).await?;
            verify_read(&args, &raw)?;
            return Ok(Reply::Answer(render(&raw, lang).ok_or("invalid attendance rows")?));
        }
        if !attendance && tool(catalog, Name::ListCourses).is_some()
            && matches(r"(?i)\b(courses?|classes?)\b|دور[ةات]|صفوف", question)
            && matches(r"(?i)\b(list|show|my|what|which)\b|اعرض|قائمة|دوراتي|ما هي", question)
        {
            let results = courses(catalog, turn).await?;
            if results.is_empty() {
                return Ok(reply(lang, "No courses available.", "لا توجد دورات متاحة."));
            }
            let mut lines = results.iter().take(30).map(|course| format!("{} — {}", course.name,
                if course.open { lang.text("Open", "مفتوحة") } else { lang.text("Closed", "مغلقة") })).collect::<Vec<_>>();
            if results.len() > 30 {
                lines.push(lang.text("Open /menu to see the rest.", "افتح /menu لعرض البقية.").into());
            }
            return Ok(Reply::Answer(lines.join("\n")));
        }
        if let Some(find) = tool(catalog, Name::FindStudents)
            && let Some(regex) = schema::regex(r"(?i)^(?:find|search for|look up)\s+(?:student\s+)?(.+?)[?.!]*$")
            && let Some(captures) = regex.captures(question)
        {
            let raw = turn.read(find, json!({"query":captures[1].trim(),"status":"all","offset":0,"limit":20})).await?;
            let safe = tools::safe_result(&raw).ok_or("invalid student result")?;
            if safe["students"].as_array().is_some_and(Vec::is_empty) {
                return Ok(reply(lang, "No students found. Try another name or open /menu.", "لم أجد طلاباً بهذا الاسم. جرّب اسماً آخر أو افتح /menu."));
            }
            return Ok(Reply::Answer(tools::render_result(&safe, lang)));
        }
        Result::Ok(Reply::Answer(unsupported(lang).into()))
    }.await;
    result.unwrap_or_else(|_| Reply::Answer(unavailable(lang).into()))
}
