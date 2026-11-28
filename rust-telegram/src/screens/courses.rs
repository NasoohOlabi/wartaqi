use serde_json::{Value, json};

use super::*;
use crate::worker::normalize_digits;

pub(super) fn is_route(route: &str) -> bool {
    matches!(
        route,
        "co.commit"
            | "co.name"
            | "co.status"
            | "co.thresholds"
            | "co.threshold"
            | "co.members"
            | "co.student"
            | "co.membership"
            | "co.teacher"
            | "co.teacher.pick"
            | "co.handbook"
            | "co.grades"
            | "co.grade"
            | "co.decision"
            | "co.reason"
            | "co.readiness"
    )
}

struct CourseScreen<'a> {
    lang: Language,
    action: &'a Action,
    tools: &'a HashSet<String>,
    id: &'a str,
    course: &'a Value,
    enrollment: &'a Value,
    person: &'a Value,
}

impl CourseScreen<'_> {
    fn t<'a>(&self, en: &'a str, ar: &'a str) -> &'a str {
        self.lang.text(en, ar)
    }
    fn tool(&self, name: &str) -> bool {
        self.tools.contains(name)
    }

    fn route(&self, route: &str) -> Action {
        let mut action = Action::new(route).arg("id", self.id);
        for key in ["student", "entry"] {
            if let Some(value) = self.action.args.get(key) {
                action = action.arg(key, value);
            }
        }
        action
    }

    fn navigation(&self, to: &str) -> Vec<Button> {
        vec![back(self.lang, self.route(to)), home(self.lang)]
    }

    fn prompt(&self, text: &str, to: &str) -> Screen {
        Screen::new(text, vec![self.navigation(to)]).prompt(self.action.clone())
    }

    fn propose(
        &self,
        tool: &str,
        payload: Value,
        prior: &str,
        target: &str,
        label: &str,
    ) -> Result<Screen> {
        let expected = if prior == "course" {
            self.course
        } else {
            self.enrollment
        };
        let mut text = string(self.course, "name").to_owned();
        let name = string(self.person, "name");
        if !name.is_empty() {
            text.push_str(" - ");
            text.push_str(name);
        }
        Ok(Screen::new(
            format!(
                "{text}\n{label}\n\n{}",
                self.t("Confirm this change?", "تأكيد هذا التغيير؟")
            ),
            vec![
                vec![choice(
                    self.t("Confirm", "تأكيد"),
                    self.route("co.commit")
                        .arg("tool", tool)
                        .arg("kind", prior)
                        .arg("payload", serde_json::to_string(&payload)?)
                        .arg("expected", serde_json::to_string(expected)?)
                        .arg("returnRoute", target),
                )],
                self.navigation(target),
            ],
        ))
    }

    async fn commit(&self, mcp: &Mcp<'_>) -> Result<Screen> {
        let tool = self.action.value("tool");
        let target = self.action.value("returnRoute");
        if !self.tool("get_admin_board")
            || !self.tool(tool)
            || !matches!(target, "co.list" | "co.view" | "co.student")
        {
            return Ok(unavailable(self.lang));
        }
        let kind = self.action.value("kind");
        let payload: Value = serde_json::from_str(self.action.value("payload"))?;
        let payload_id = string(&payload, "id");
        let current = match tool {
            "save_course" | "set_course_status" | "set_course_thresholds"
                if kind == "course" && payload_id == self.id =>
            {
                self.course
            }
            "save_enrollment"
                if matches!(kind, "entry" | "new")
                    && self.course.is_object()
                    && payload.get("level").is_none()
                    && string(&payload, "courseId") == self.id
                    && string(&payload, "studentId") == self.action.value("student")
                    && (kind == "new" || payload_id == string(self.enrollment, "id")) =>
            {
                self.enrollment
            }
            "save_grades" | "save_grade_decision" | "set_handbook_received"
                if kind == "entry"
                    && self.enrollment.is_object()
                    && payload_id == string(self.enrollment, "id") =>
            {
                self.enrollment
            }
            _ => return Ok(unavailable(self.lang)),
        };
        if !id_shape(payload_id) {
            return Ok(unavailable(self.lang));
        }
        let expected: Value = serde_json::from_str(self.action.value("expected"))?;
        if &expected != current {
            return Ok(Screen::new(
                self.t(
                    "The record changed. Reopen it and try again.",
                    "تغير السجل. افتحه مجدداً وحاول مرة أخرى.",
                ),
                vec![self.navigation(target)],
            ));
        }
        mcp.call(tool, json!({"input":payload})).await?;
        Ok(Screen::new(
            self.t("Saved successfully.", "تم الحفظ بنجاح."),
            vec![self.navigation(target)],
        ))
    }
}

pub(super) async fn render(
    mcp: &Mcp<'_>,
    tools: &HashSet<String>,
    lang: Language,
    action: &Action,
    input: Option<&str>,
    snapshot: &Value,
    notebook: &Notebook,
) -> Result<Screen> {
    let id = action.value("id");
    if !id_shape(id) {
        return Ok(unavailable(lang));
    }
    let course = array(snapshot, "courses")?
        .iter()
        .find(|course| course["id"] == id)
        .unwrap_or(&Value::Null);
    let student = action.value("student");
    let enrollment = array(snapshot, "enrollments")?
        .iter()
        .find(|entry| entry["courseId"] == id && entry["studentId"] == student)
        .unwrap_or(&Value::Null);
    let board = if tools.contains("get_admin_board")
        && matches!(
            action.route.as_str(),
            "co.members" | "co.student" | "co.membership" | "co.teacher" | "co.teacher.pick"
        ) {
        Some(mcp.call("get_admin_board", json!({})).await?)
    } else {
        None
    };
    let roster = array(board.as_ref().unwrap_or(snapshot), "students")?;
    let person = roster
        .iter()
        .find(|person| person["studentId"] == student)
        .unwrap_or(&Value::Null);
    let state = CourseScreen {
        lang,
        action,
        tools,
        id,
        course,
        enrollment,
        person,
    };
    if action.route == "co.commit" {
        return state.commit(mcp).await;
    }
    if action.route == "co.name" && state.tool("save_course") {
        let destination = if course.is_null() {
            "co.list"
        } else {
            "co.view"
        };
        let Some(name) = input else {
            return Ok(state.prompt(
                state.t(
                    "Send the course name, or /cancel.",
                    "أرسل اسم الدورة، أو /cancel للإلغاء.",
                ),
                destination,
            ));
        };
        if name.is_empty() || name.encode_utf16().count() > 200 {
            return Ok(state.prompt(
                state.t(
                    "Enter a name of 1–200 characters.",
                    "أدخل اسماً من ١ إلى ٢٠٠ حرف.",
                ),
                destination,
            ));
        }
        let label = format!("{}: {name}", state.t("Course name", "اسم الدورة"));
        return state.propose(
            "save_course",
            json!({"id":id,"name":name}),
            "course",
            destination,
            &label,
        );
    }
    if course.is_null() {
        return Ok(unavailable(lang));
    }
    if action.route == "co.members" {
        return members(&state, roster, array(snapshot, "enrollments")?);
    }
    if action.route == "co.readiness" && state.tool("get_admin_board") {
        return Ok(readiness(&state, notebook, array(snapshot, "enrollments")?));
    }
    if matches!(
        action.route.as_str(),
        "co.status" | "co.thresholds" | "co.threshold"
    ) {
        return settings(&state, input);
    }
    if (enrollment.is_object()
        && !action.value("entry").is_empty()
        && action.value("entry") != enrollment["id"])
        || (state.person.is_null() && !enrollment.is_object())
    {
        return Ok(unavailable(lang));
    }
    if matches!(
        action.route.as_str(),
        "co.grades" | "co.grade" | "co.decision" | "co.reason"
    ) {
        return grades(&state, input);
    }
    membership(&state, board.as_ref())
}

fn array<'a>(object: &'a Value, name: &str) -> Result<&'a Vec<Value>> {
    object[name]
        .as_array()
        .ok_or_else(|| "invalid notebook response".into())
}

fn string<'a>(object: &'a Value, name: &str) -> &'a str {
    object[name].as_str().unwrap_or_default()
}

fn display(value: &Value) -> String {
    if value.is_null() {
        "—".into()
    } else if let Some(text) = value.as_str() {
        if text.is_empty() {
            "—".into()
        } else {
            short(text, 500)
        }
    } else {
        value.to_string()
    }
}

fn members(state: &CourseScreen<'_>, roster: &[Value], enrollments: &[Value]) -> Result<Screen> {
    let index = page(state.action);
    let query = state.action.value("q");
    let people = roster.iter().filter_map(|person| {
        let name = string(person, "name");
        let id = string(person, "studentId");
        let enrollment = enrollments
            .iter()
            .find(|entry| entry["courseId"] == state.id && entry["studentId"] == id);
        if state.tool("get_admin_board") || enrollment.is_some() {
            Some((
                name,
                id,
                enrollment.is_some_and(|entry| entry["active"] == true),
            ))
        } else {
            None
        }
    });
    let people = rank_by_name(people, query, |(name, _, _)| name);
    let mut rows: Vec<_> = people
        .iter()
        .skip(index * PAGE)
        .take(PAGE)
        .map(|(name, student, active)| {
            vec![choice(
                &format!("{}{}", name, if *active { " ✓" } else { "" }),
                Action::new("co.student")
                    .arg("id", state.id)
                    .arg("student", *student),
            )]
        })
        .collect();
    rows.extend(nav(
        state.lang,
        state.route("co.members").arg("q", query),
        index,
        people.len(),
    ));
    rows.push(vec![choice(
        state.t("Search students", "بحث عن طالب"),
        state.route("co.search").arg("target", "co.members"),
    )]);
    if state.tool("create_student_in_course") {
        rows.push(vec![choice(
            state.t("Add new student", "إضافة طالب جديد"),
            Action::new("nb.add").arg("courseId", state.id),
        )]);
    }
    rows.push(state.navigation("co.view"));
    Ok(Screen::new(
        format!(
            "{} — {}",
            string(state.course, "name"),
            state.t("Students", "الطلاب")
        ),
        rows,
    ))
}

fn settings(state: &CourseScreen<'_>, input: Option<&str>) -> Result<Screen> {
    let course = state.course;
    if state.action.route == "co.status" && state.tool("set_course_status") {
        let open = course["open"] == true;
        let label = if open {
            state.t(
                "Closing may advance eligible students’ roster levels.",
                "قد يؤدي الإغلاق إلى ترقية مستويات الطلاب المستحقين.",
            )
        } else {
            state.t("Reopen this course?", "إعادة فتح هذه الدورة؟")
        };
        return state.propose(
            "set_course_status",
            json!({"id":state.id,"open":!open}),
            "course",
            "co.view",
            label,
        );
    }
    if !state.tool("set_course_thresholds") {
        return Ok(unavailable(state.lang));
    }
    let fields = [
        ("attendanceMinimum", state.t("Attendance", "الحضور")),
        ("practicalMinimum", state.t("Practical", "العملي")),
        ("theoryMinimum", state.t("Theory", "النظري")),
    ];
    if state.action.route == "co.thresholds" {
        let mut rows: Vec<_> = fields
            .iter()
            .map(|(field, label)| {
                vec![choice(
                    label,
                    state.route("co.threshold").arg("field", *field),
                )]
            })
            .collect();
        rows.push(state.navigation("co.view"));
        return Ok(Screen::new(
            format!(
                "{}: {} / {} / {}",
                state.t("Thresholds", "حدود النجاح"),
                display(&course["attendanceMinimum"]),
                display(&course["practicalMinimum"]),
                display(&course["theoryMinimum"])
            ),
            rows,
        ));
    }
    if state.action.route != "co.threshold" {
        return Ok(unavailable(state.lang));
    }
    let field = state.action.value("field");
    let Some((_, label)) = fields.iter().find(|(key, _)| *key == field) else {
        return Ok(unavailable(state.lang));
    };
    let Some(value) = input.and_then(percentage) else {
        return Ok(state.prompt(
            state.t("Enter a number from 0 to 100.", "أدخل رقماً من ٠ إلى ١٠٠."),
            "co.thresholds",
        ));
    };
    let mut payload = json!({"id":state.id});
    for (key, _) in fields {
        payload[key] = course[key].clone();
    }
    payload[field] = json!(value);
    state.propose(
        "set_course_thresholds",
        payload,
        "course",
        "co.view",
        &format!("{label}: {value}"),
    )
}

fn membership(state: &CourseScreen<'_>, board: Option<&Value>) -> Result<Screen> {
    let enrollment = state.enrollment;
    let active = enrollment["active"] == true;
    let has_enrollment = enrollment.is_object();
    let person = state.person;
    if state.action.route == "co.student" {
        if person.is_null() || (!state.tool("get_admin_board") && !has_enrollment) {
            return Ok(unavailable(state.lang));
        }
        let route = |route| state.route(route).arg("entry", string(enrollment, "id"));
        let mut rows = Vec::new();
        if state.tool("save_enrollment") {
            let label = if active {
                state.t("Withdraw", "إلغاء التسجيل")
            } else {
                state.t("Enroll", "تسجيل")
            };
            rows.push(vec![choice(label, route("co.membership"))]);
            if has_enrollment {
                rows.push(vec![choice(
                    state.t("Teacher", "المعلم"),
                    route("co.teacher"),
                )]);
            }
        }
        if has_enrollment {
            if state.tool("set_handbook_received") {
                let action = if enrollment["handbookReceived"] == true {
                    state.t("Mark handbook not received", "إلغاء استلام الدليل")
                } else {
                    state.t("Mark handbook received", "تسجيل استلام الدليل")
                };
                rows.push(vec![choice(action, route("co.handbook"))]);
            }
            if state.tool("save_grades") {
                rows.push(vec![choice(
                    state.t("Scores and notes", "الدرجات والملاحظات"),
                    route("co.grades"),
                )]);
            }
            if state.tool("save_grade_decision") {
                rows.push(vec![choice(
                    state.t("Result decision", "قرار النتيجة"),
                    route("co.decision"),
                )]);
            }
        }
        rows.push(state.navigation("co.members"));
        let status = if active {
            state.t("Active", "نشط")
        } else {
            state.t("Not active", "غير نشط")
        };
        let mut text = format!(
            "{}\n{}: {}\n{}: {status}",
            short(string(person, "name"), 200),
            state.t("Course level", "مستوى الدورة"),
            display(&enrollment["level"]),
            state.t("Membership", "التسجيل")
        );
        if has_enrollment && state.tool("get_admin_board") {
            text.push_str(&format!(
                "\n{}: {} / {}\n{}: {}\n{}: {}",
                state.t("Theory / practical", "النظري / العملي"),
                display(&enrollment["theory"]),
                display(&enrollment["practical"]),
                state.t("Notes", "الملاحظات"),
                display(&enrollment["notes"]),
                state.t("Handbook", "الدليل"),
                if enrollment["handbookReceived"] == true {
                    "✓"
                } else {
                    "—"
                }
            ));
        }
        return Ok(Screen::new(text, rows));
    }
    if state.action.route == "co.handbook" && has_enrollment && state.tool("set_handbook_received")
    {
        return state.propose(
            "set_handbook_received",
            json!({"id":enrollment["id"],"handbookReceived":enrollment["handbookReceived"]!=true}),
            "entry",
            "co.student",
            state.t("Change handbook receipt?", "تغيير حالة استلام الدليل؟"),
        );
    }
    if !state.tool("save_enrollment") {
        return Ok(unavailable(state.lang));
    }
    let student = state.action.value("student");
    if state.action.route == "co.membership" && person.is_object() {
        let entry_id = if has_enrollment {
            string(enrollment, "id").into()
        } else {
            uuid()?
        };
        let label = if active {
            state.t(
                "Withdraw membership? History and grades remain.",
                "إلغاء التسجيل؟ سيبقى التاريخ والدرجات.",
            )
        } else {
            state.t("Enroll this student?", "تسجيل هذا الطالب؟")
        };
        return state.propose(
            "save_enrollment",
            json!({"id":entry_id,"courseId":state.id,"studentId":student,
            "teacherId":enrollment["teacherId"],"active":!active}),
            if has_enrollment { "entry" } else { "new" },
            "co.student",
            label,
        );
    }
    let Some(board) = board else {
        return Ok(unavailable(state.lang));
    };
    if !has_enrollment {
        return Ok(unavailable(state.lang));
    }
    let teachers: Vec<_> = array(board, "teachers")?
        .iter()
        .filter(|item| item["active"] == true)
        .collect();
    if state.action.route == "co.teacher" {
        let index = page(state.action);
        let mut rows: Vec<_> = teachers
            .iter()
            .skip(index * PAGE)
            .take(PAGE)
            .map(|teacher| {
                vec![choice(
                    string(teacher, "name"),
                    state
                        .route("co.teacher.pick")
                        .arg("teacher", string(teacher, "teacherId")),
                )]
            })
            .collect();
        rows.extend(nav(
            state.lang,
            state.route("co.teacher"),
            index,
            teachers.len(),
        ));
        rows.push(vec![choice(
            state.t("No teacher", "بلا معلم"),
            state.route("co.teacher.pick").arg("teacher", ""),
        )]);
        rows.push(state.navigation("co.student"));
        return Ok(Screen::new(
            state.t("Choose the course teacher.", "اختر معلم الدورة."),
            rows,
        ));
    }
    let teacher = state.action.value("teacher");
    if state.action.route != "co.teacher.pick"
        || (!teacher.is_empty() && !teachers.iter().any(|item| item["teacherId"] == teacher))
    {
        return Ok(unavailable(state.lang));
    }
    state.propose(
        "save_enrollment",
        json!({"id":enrollment["id"],"courseId":state.id,"studentId":student,"active":active,
        "teacherId": if teacher.is_empty() { None } else { Some(teacher) }}),
        "entry",
        "co.student",
        state.t("Change course teacher?", "تغيير معلم الدورة؟"),
    )
}

fn percentage(text: &str) -> Option<f64> {
    let text = normalize_digits(text);
    if text.is_empty()
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    text.parse::<f64>()
        .ok()
        .filter(|value| (0.0..=100.0).contains(value))
}

fn grades(state: &CourseScreen<'_>, input: Option<&str>) -> Result<Screen> {
    let enrollment = state.enrollment;
    if enrollment.is_null() {
        return Ok(unavailable(state.lang));
    }
    let fields = [
        ("theory", state.t("Theory", "النظري")),
        ("practical", state.t("Practical", "العملي")),
        ("notes", state.t("Notes", "الملاحظات")),
    ];
    match state.action.route.as_str() {
        "co.grades" if state.tool("save_grades") => {
            let mut rows = Vec::new();
            let mut lines = vec![state.t("Scores and notes", "الدرجات والملاحظات").into()];
            for (field, label) in fields {
                rows.push(vec![choice(
                    label,
                    state.route("co.grade").arg("field", field),
                )]);
                lines.push(format!("{label}: {}", display(&enrollment[field])));
            }
            rows.push(state.navigation("co.student"));
            Ok(Screen::new(lines.join("\n"), rows))
        }
        "co.grade" if state.tool("save_grades") => {
            let field = state.action.value("field");
            let Some((_, label)) = fields.iter().find(|(key, _)| *key == field) else {
                return Ok(unavailable(state.lang));
            };
            let message = if field == "notes" {
                state.t("Send notes, or - to clear.", "أرسل الملاحظات أو - للمسح.")
            } else {
                state.t(
                    "Send a score from 0 to 100, or - to clear.",
                    "أرسل درجة من ٠ إلى ١٠٠ أو - للمسح.",
                )
            };
            let value = match input.filter(|text| !text.is_empty()) {
                Some("-") if field == "notes" => Some(json!("")),
                Some("-") => Some(Value::Null),
                Some(value) if field == "notes" && value.encode_utf16().count() <= 2000 => {
                    Some(json!(value))
                }
                Some(value) if field != "notes" => percentage(value).map(|value| json!(value)),
                _ => None,
            };
            let Some(value) = value else {
                return Ok(state.prompt(message, "co.grades"));
            };
            let mut payload = json!({"id":enrollment["id"],"theory":enrollment["theory"],"practical":enrollment["practical"],
                "notes":enrollment["notes"].as_str().unwrap_or_default()});
            let description = format!("{label}: {}", display(&value));
            payload[field] = value;
            state.propose("save_grades", payload, "entry", "co.student", &description)
        }
        "co.decision" if state.tool("save_grade_decision") => Ok(Screen::new(
            state.t(
                "Choose a result, then give a reason.",
                "اختر النتيجة ثم اذكر السبب.",
            ),
            vec![
                vec![
                    choice(
                        state.t("Pass", "ناجح"),
                        state.route("co.reason").arg("result", "pass"),
                    ),
                    choice(
                        state.t("Fail", "راسب"),
                        state.route("co.reason").arg("result", "fail"),
                    ),
                ],
                vec![choice(
                    state.t("Calculated result", "النتيجة المحسوبة"),
                    state.route("co.reason").arg("result", "calculated"),
                )],
                state.navigation("co.student"),
            ],
        )),
        "co.reason" if state.tool("save_grade_decision") => {
            let result = state.action.value("result");
            let label = match result {
                "pass" => state.t("Pass", "ناجح"),
                "fail" => state.t("Fail", "راسب"),
                "calculated" => state.t("Calculated result", "النتيجة المحسوبة"),
                _ => return Ok(unavailable(state.lang)),
            };
            let Some(note) =
                input.filter(|note| !note.is_empty() && note.encode_utf16().count() <= 2000)
            else {
                return Ok(state.prompt(
                    state.t(
                        "Send a reason of 1–2000 characters.",
                        "أرسل سبباً من ١ إلى ٢٠٠٠ حرف.",
                    ),
                    "co.decision",
                ));
            };
            state.propose(
                "save_grade_decision",
                json!({"id":enrollment["id"],"result":result,"note":note,"requestId":uuid()?}),
                "entry",
                "co.student",
                &format!("{label}: {note}"),
            )
        }
        _ => Ok(unavailable(state.lang)),
    }
}

fn readiness(state: &CourseScreen<'_>, notebook: &Notebook, enrollments: &[Value]) -> Screen {
    let records = projected_marks(&notebook.events);
    let attended: HashSet<_> = records
        .iter()
        .filter_map(|((_, id, _), (_, mark))| if mark.is_some() { *id } else { None })
        .collect();
    let mut total = 0;
    let mut missing = [0; 3];
    let mut low = [0; 2];
    for enrollment in enrollments {
        if enrollment["courseId"] != state.id
            || enrollment["active"] != true
            || !notebook
                .students
                .iter()
                .any(|student| student.active && enrollment["studentId"] == student.student_id)
        {
            continue;
        }
        total += 1;
        for (index, field) in ["theory", "practical"].into_iter().enumerate() {
            match enrollment[field].as_f64() {
                None => missing[index] += 1,
                Some(score) => {
                    if score
                        < state.course[format!("{field}Minimum")]
                            .as_f64()
                            .unwrap_or_default()
                    {
                        low[index] += 1;
                    }
                }
            }
        }
        if !attended.contains(string(enrollment, "id")) {
            missing[2] += 1;
        }
    }
    Screen::new(
        format!(
            "{} — {}\n{}: {total}\n{}: {} / {} / {}\n{}: {} / {}",
            state.t("Result readiness", "جاهزية النتائج"),
            short(string(state.course, "name"), 200),
            state.t("Active students", "الطلاب النشطون"),
            state.t(
                "Missing theory / practical / attendance",
                "النظري / العملي / الحضور الناقص"
            ),
            missing[0],
            missing[1],
            missing[2],
            state.t("Below theory / practical minimum", "دون حد النظري / العملي"),
            low[0],
            low[1]
        ),
        vec![state.navigation("co.view")],
    )
}
