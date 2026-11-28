use std::collections::BTreeMap;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    auth::{Result, random},
    mcp::Tool,
};

use super::{Language, bounded, has_private_input, matches, schema};

pub(super) type Labels = BTreeMap<String, String>;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Name {
    GetAccount,
    GetProfile,
    GetMyAttendance,
    ListCourses,
    FindStudents,
    GetStudentDetails,
    GetCourseAttendanceView,
    GetCourseWorkflow,
    GetNotebookHistory,
    ExportCourseReport,
    UpdateProfile,
    UpdateStudent,
    SyncAttendance,
    SyncStudent,
    AppendEvents,
    SaveStudent,
    CreateStudentInCourse,
    SaveCourse,
    SetCourseStatus,
    SetCourseThresholds,
    SaveEnrollment,
    SaveGrades,
    SaveGradeDecision,
    SetHandbookReceived,
}

impl Name {
    pub(super) fn wire(self) -> &'static str {
        match self {
            Self::GetAccount => "get_account",
            Self::GetProfile => "get_profile",
            Self::GetMyAttendance => "get_my_attendance",
            Self::ListCourses => "list_courses",
            Self::FindStudents => "find_students",
            Self::GetStudentDetails => "get_student_details",
            Self::GetCourseAttendanceView => "get_course_attendance_view",
            Self::GetCourseWorkflow => "get_course_workflow",
            Self::GetNotebookHistory => "get_notebook_history",
            Self::ExportCourseReport => "export_course_report",
            Self::UpdateProfile => "update_profile",
            Self::UpdateStudent => "update_student",
            Self::SyncAttendance => "sync_attendance",
            Self::SyncStudent => "sync_student",
            Self::AppendEvents => "append_events",
            Self::SaveStudent => "save_student",
            Self::CreateStudentInCourse => "create_student_in_course",
            Self::SaveCourse => "save_course",
            Self::SetCourseStatus => "set_course_status",
            Self::SetCourseThresholds => "set_course_thresholds",
            Self::SaveEnrollment => "save_enrollment",
            Self::SaveGrades => "save_grades",
            Self::SaveGradeDecision => "save_grade_decision",
            Self::SetHandbookReceived => "set_handbook_received",
        }
    }

    pub(super) fn write(self) -> bool {
        match self {
            Self::GetAccount
            | Self::GetProfile
            | Self::GetMyAttendance
            | Self::ListCourses
            | Self::FindStudents
            | Self::GetStudentDetails
            | Self::GetCourseAttendanceView
            | Self::GetCourseWorkflow
            | Self::GetNotebookHistory
            | Self::ExportCourseReport => false,
            Self::UpdateProfile
            | Self::UpdateStudent
            | Self::SyncAttendance
            | Self::SyncStudent
            | Self::AppendEvents
            | Self::SaveStudent
            | Self::CreateStudentInCourse
            | Self::SaveCourse
            | Self::SetCourseStatus
            | Self::SetCourseThresholds
            | Self::SaveEnrollment
            | Self::SaveGrades
            | Self::SaveGradeDecision
            | Self::SetHandbookReceived => true,
        }
    }

    fn title(self, lang: Language) -> &'static str {
        match self {
            Self::UpdateProfile => lang.text("Update profile", "تعديل الملف الشخصي"),
            Self::UpdateStudent => lang.text("Update student", "تعديل الطالب"),
            Self::SyncAttendance => lang.text("Save attendance", "حفظ الحضور"),
            Self::SyncStudent => lang.text("Save student change", "حفظ تعديل الطالب"),
            Self::AppendEvents => lang.text("Save notebook entry", "حفظ سجل المتابعة"),
            Self::SaveStudent => lang.text("Save student", "حفظ الطالب"),
            Self::CreateStudentInCourse => {
                lang.text("Create student and enrollment", "إضافة طالب وتسجيله")
            }
            Self::SaveCourse => lang.text("Save course", "حفظ الدورة"),
            Self::SetCourseStatus => lang.text("Change course status", "تغيير حالة الدورة"),
            Self::SetCourseThresholds => lang.text("Change course thresholds", "تغيير حدود الدورة"),
            Self::SaveEnrollment => lang.text("Save enrollment", "حفظ التسجيل"),
            Self::SaveGrades => lang.text("Save grades", "حفظ الدرجات"),
            Self::SaveGradeDecision => lang.text("Save result decision", "حفظ قرار النتيجة"),
            Self::SetHandbookReceived => lang.text("Save handbook receipt", "حفظ استلام الدليل"),
            _ => "",
        }
    }
}

fn field(key: &str, lang: Language) -> &str {
    match key {
        "studentId" => lang.text("Student", "الطالب"),
        "courseId" => lang.text("Course", "الدورة"),
        "enrollmentId" => lang.text("Enrollment", "التسجيل"),
        "teacherId" => lang.text("Teacher", "المعلم"),
        "targetId" => lang.text("Entry to correct", "السجل المراد تصحيحه"),
        "id" => lang.text("Record", "السجل"),
        "name" => lang.text("Name", "الاسم"),
        "firstName" => lang.text("First name", "الاسم الأول"),
        "lastName" => lang.text("Last name", "الكنية"),
        "studentName" => lang.text("Student", "الطالب"),
        "courseName" => lang.text("Course", "الدورة"),
        "teacherName" => lang.text("Teacher", "المعلم"),
        "level" => lang.text("Level", "المستوى"),
        "group" => lang.text("Level", "المستوى"),
        "active" => lang.text("Active", "نشط"),
        "open" => lang.text("Open", "مفتوحة"),
        "date" => lang.text("Date", "التاريخ"),
        "status" => lang.text("Status", "الحالة"),
        "present" => lang.text("Present", "حاضر"),
        "absent" => lang.text("Absent", "غائب"),
        "excused" => lang.text("Excused", "معذور"),
        "unmarked" => lang.text("Unmarked", "غير مسجل"),
        "marked" => lang.text("Marked", "مسجل"),
        "total" => lang.text("Total", "المجموع"),
        "theory" => lang.text("Theory", "النظري"),
        "practical" => lang.text("Practical", "العملي"),
        "result" => lang.text("Result", "النتيجة"),
        "calculatedResult" => lang.text("Calculated result", "النتيجة المحسوبة"),
        "effectiveResult" => lang.text("Final result", "النتيجة النهائية"),
        "attendanceMinimum" => lang.text("Minimum attendance", "الحد الأدنى للحضور"),
        "practicalMinimum" => lang.text("Minimum practical", "الحد الأدنى للعملي"),
        "theoryMinimum" => lang.text("Minimum theory", "الحد الأدنى للنظري"),
        "handbookReceived" => lang.text("Handbook received", "استلم الدليل"),
        "text" => lang.text("Note", "الملاحظة"),
        "note" => lang.text("Reason", "السبب"),
        "notes" => lang.text("Notes", "الملاحظات"),
        "reason" => lang.text("Reason", "السبب"),
        "tags" => lang.text("Tags", "الوسوم"),
        "kind" => lang.text("Type", "النوع"),
        "attendance" => lang.text("Attendance", "الحضور"),
        "reversal" => lang.text("Correction", "تصحيح"),
        "whatsapp" => lang.text("WhatsApp", "واتساب"),
        "phone" => lang.text("Phone", "الهاتف"),
        "address" => lang.text("Address", "العنوان"),
        _ => key,
    }
}

const MODEL_FIELDS: &[&str] = &[
    "id",
    "studentId",
    "courseId",
    "enrollmentId",
    "eventId",
    "targetId",
    "teacherId",
    "name",
    "studentName",
    "courseName",
    "teacherName",
    "firstName",
    "lastName",
    "level",
    "active",
    "open",
    "status",
    "date",
    "kind",
    "role",
    "canRead",
    "canWrite",
    "view",
    "total",
    "offset",
    "limit",
    "present",
    "absent",
    "excused",
    "unmarked",
    "marked",
    "rate",
    "result",
    "calculatedResult",
    "effectiveResult",
    "theory",
    "practical",
    "handbookReceived",
    "missingTheory",
    "missingPractical",
    "missingAttendance",
    "lowTheory",
    "lowPractical",
    "course",
    "account",
    "profile",
    "student",
    "students",
    "courses",
    "teachers",
    "enrollment",
    "notes",
    "session",
    "sessionCounts",
    "attendance",
    "rows",
    "records",
    "event",
    "payload",
    "target",
    "undone",
    "readiness",
    "gradesClose",
    "missingScores",
    "followUp",
    "totals",
    "breakdown",
    "recentDates",
    "marks",
    "absences",
    "chart",
    "label",
    "evidence",
    "columns",
];

pub(super) struct Allowed {
    pub(super) name: Name,
    pub(super) description: String,
    pub(super) schema: Value,
}

impl Allowed {
    pub(super) fn from_tool(tool: Tool) -> Option<Self> {
        let name = serde_json::from_value(Value::String(tool.name)).ok()?;
        if tool.input_schema["type"] != "object" || !schema::supported(&tool.input_schema, 0) {
            return None;
        }
        Some(Self {
            name,
            description: bounded(&tool.description, 500),
            schema: tool.input_schema,
        })
    }

    pub(super) fn student() -> Self {
        Self {
            name: Name::GetMyAttendance,
            description: "View your own attendance".into(),
            schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        }
    }

    pub(super) fn validate(&self, args: &Value) -> Result<()> {
        if !args.is_object()
            || args.to_string().len() > 16_000
            || !schema::accepts(&self.schema, args)
        {
            return Err("invalid agent arguments".into());
        }
        if self.name.write() && (args.get("edit").is_some() || !safe_write(args, 0)) {
            return Err("protected agent fields".into());
        }
        let allowed = match self.name {
            Name::SyncAttendance => args["records"].as_array().is_some_and(|items| {
                items.iter().all(|record| {
                    record.get("editTime").is_none()
                        && record.get("expectedRevision").is_none()
                        && record["event"].get("editTime").is_none()
                })
            }),
            Name::SyncStudent => {
                args["operation"].get("editTime").is_none()
                    && args["operation"]["event"].get("editTime").is_none()
            }
            Name::SaveEnrollment => args["input"].get("level").is_none(),
            Name::CreateStudentInCourse => args["input"]["enrollment"].get("level").is_none(),
            Name::UpdateStudent => args
                .get("group")
                .is_none_or(|group| args["expected"].get("group") == Some(group)),
            Name::AppendEvents => args["events"].as_array().is_some_and(|events| {
                !events.is_empty()
                    && events.iter().all(|event| {
                        matches!(event["payload"]["kind"].as_str(), Some("note" | "reversal"))
                            && event.get("editTime").is_none()
                    })
            }),
            _ => true,
        };
        if !allowed {
            return Err("agent operation denied".into());
        }
        Ok(())
    }

    pub(super) fn model_definition(&self) -> Value {
        let mut parameters = self.schema.clone();
        match self.name {
            Name::AppendEvents => {
                if let Some(kinds) = parameters
                    .pointer_mut("/properties/events/items/properties/payload/oneOf")
                    .and_then(Value::as_array_mut)
                {
                    kinds.retain(|schema| {
                        matches!(
                            schema["properties"]["kind"]["const"].as_str(),
                            Some("note" | "reversal")
                        )
                    });
                }
                if let Some(event) = parameters.pointer_mut("/properties/events/items") {
                    hide_event_version(event);
                }
            }
            Name::SyncAttendance => {
                if let Some(record) = parameters
                    .pointer_mut("/properties/records/items/properties")
                    .and_then(Value::as_object_mut)
                {
                    record.remove("expectedRevision");
                    record.remove("editTime");
                    if let Some(event) = record.get_mut("event") {
                        hide_event_version(event);
                    }
                }
            }
            Name::SyncStudent => {
                if let Some(operations) = parameters
                    .pointer_mut("/properties/operation/oneOf")
                    .and_then(Value::as_array_mut)
                {
                    for operation in operations {
                        if operation["properties"]["kind"]["const"] == "note" {
                            if let Some(event) = operation.pointer_mut("/properties/event") {
                                hide_event_version(event);
                            }
                        } else {
                            hide_event_version(operation);
                        }
                    }
                }
            }
            _ if self.name.write() => {
                if let Some(properties) = parameters
                    .get_mut("properties")
                    .and_then(Value::as_object_mut)
                {
                    properties.remove("edit");
                }
            }
            _ => {}
        }
        json!({"type":"function", "function":{"name":self.name, "description":self.description, "parameters":parameters}})
    }
}

fn hide_event_version(schema: &mut Value) {
    if let Some(fields) = schema["properties"].as_object_mut() {
        for key in ["id", "clientTime", "editTime"] {
            fields.remove(key);
        }
    }
    if let Some(fields) = schema["required"].as_array_mut() {
        fields.retain(|field| !matches!(field.as_str(), Some("id" | "clientTime" | "editTime")));
    }
}

fn safe_write(value: &Value, depth: usize) -> bool {
    depth <= 12
        && match value {
            Value::Array(items) => {
                items.len() <= 20 && items.iter().all(|item| safe_write(item, depth + 1))
            }
            Value::Object(items) => items.iter().all(|(key, item)| {
                !matches(
                    r"(?i)password|secret|token|credential|authorization|phoneconfirmed",
                    key,
                ) && safe_write(item, depth + 1)
            }),
            Value::String(value) => value.chars().count() <= 2000,
            _ => true,
        }
}

pub(super) fn uuid() -> Result<String> {
    let mut bytes = random::<16>()?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

pub(super) fn prepare(name: Name, mut args: Value) -> Result<Value> {
    fn stamp(event: &mut Value) -> Result<()> {
        let event = event.as_object_mut().ok_or("missing agent event")?;
        event.insert("id".into(), json!(uuid()?));
        event.insert(
            "clientTime".into(),
            json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
        );
        event.remove("editTime");
        Ok(())
    }
    match name {
        Name::AppendEvents => {
            for event in args["events"]
                .as_array_mut()
                .ok_or("missing agent events")?
            {
                stamp(event)?;
            }
        }
        Name::SyncAttendance => {
            for record in args["records"]
                .as_array_mut()
                .ok_or("missing attendance records")?
            {
                let record = record.as_object_mut().ok_or("invalid attendance record")?;
                record.remove("editTime");
                record.remove("expectedRevision");
                stamp(record.get_mut("event").ok_or("missing attendance event")?)?;
            }
        }
        Name::SyncStudent => match args["operation"]["kind"].as_str() {
            Some("note") => stamp(&mut args["operation"]["event"])?,
            Some("contact" | "profile-v1") => stamp(&mut args["operation"])?,
            _ => return Err("invalid student operation".into()),
        },
        _ => {
            if let Some(object) = args.as_object_mut() {
                object.remove("edit");
            }
        }
    }
    Ok(args)
}

pub(super) fn prepare_proposal(
    question: &str,
    name: Name,
    args: Value,
    labels: &mut Labels,
) -> Result<Value> {
    fn display_name(input: &Value) -> Result<String> {
        let display = input["name"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                [input["firstName"].as_str(), input["lastName"].as_str()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ")
            });
        let display = display.trim();
        if display.is_empty()
            || display.encode_utf16().count() > 200
            || display.contains(char::is_control)
        {
            return Err("missing name for new agent record".into());
        }
        Ok(display.into())
    }

    fn replace_id(
        input: &mut Value,
        field: &str,
        label: String,
        labels: &mut Labels,
    ) -> Result<()> {
        let id = uuid()?;
        input
            .as_object_mut()
            .ok_or("missing agent record")?
            .insert(field.into(), json!(id));
        labels.insert(id, label);
        Ok(())
    }

    let mut args = prepare(name, args)?;
    let create = matches(
        r"(?i)\b(add|create|new)\b|أضف|اضف|أضِف|انشئ|أنشئ|جديد",
        question,
    );
    match name {
        Name::SaveCourse | Name::SaveStudent if create => {
            let field = if name == Name::SaveCourse {
                "id"
            } else {
                "studentId"
            };
            let input = args.get_mut("input").ok_or("missing agent record")?;
            if !input[field]
                .as_str()
                .is_some_and(|id| labels.contains_key(id))
            {
                let label = display_name(input)?;
                replace_id(input, field, label, labels)?;
            }
        }
        Name::CreateStudentInCourse if create => {
            let input = args.get_mut("input").ok_or("missing agent records")?;
            let course = input["enrollment"]["courseId"]
                .as_str()
                .and_then(|id| labels.get(id))
                .cloned()
                .ok_or("unresolved course")?;
            let student = input.get_mut("student").ok_or("missing agent record")?;
            let label = display_name(student)?;
            replace_id(student, "studentId", label.clone(), labels)?;
            replace_id(
                input
                    .get_mut("enrollment")
                    .ok_or("missing agent enrollment")?,
                "id",
                bounded(&format!("{label} · {course}"), 200),
                labels,
            )?;
        }
        Name::SaveEnrollment if matches(r"(?i)\benroll\b|سجّل|سجل|ضم", question) => {
            let input = args.get_mut("input").ok_or("missing agent enrollment")?;
            if input["active"] == true
                && !input["id"]
                    .as_str()
                    .is_some_and(|id| labels.contains_key(id))
            {
                let course = input["courseId"]
                    .as_str()
                    .and_then(|id| labels.get(id))
                    .ok_or("unresolved course")?;
                let student = input["studentId"]
                    .as_str()
                    .and_then(|id| labels.get(id))
                    .ok_or("unresolved student")?;
                let label = bounded(&format!("{student} · {course}"), 200);
                replace_id(input, "id", label, labels)?;
            }
        }
        Name::SaveGradeDecision => {
            let input = args["input"]
                .as_object_mut()
                .ok_or("missing agent result decision")?;
            input.insert("requestId".into(), json!(uuid()?));
        }
        _ => {}
    }
    Ok(args)
}

pub(super) fn safe_result(raw: &Value) -> Option<Value> {
    fn visit(value: &Value, depth: usize, key: &str) -> Value {
        if depth > 8 {
            return Value::Null;
        }
        match value {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .take(30)
                    .map(|item| {
                        if item.is_object()
                            || item.is_array()
                            || matches!(key, "marks" | "recentDates" | "columns")
                        {
                            visit(item, depth + 1, key)
                        } else {
                            Value::Null
                        }
                    })
                    .collect(),
            ),
            Value::Object(items) => Value::Object(
                items
                    .iter()
                    .filter(|(key, _)| MODEL_FIELDS.contains(&key.as_str()))
                    .map(|(key, item)| (key.clone(), visit(item, depth + 1, key)))
                    .collect(),
            ),
            Value::String(value) => {
                if schema::uuid(value) {
                    return if key == "id" || key.ends_with("Id") {
                        json!(value)
                    } else {
                        Value::Null
                    };
                }
                if !(has_private_input(value) || matches(r"(?i)https?://|www\.", value)) {
                    return json!(bounded(value, 200));
                }
                Value::Null
            }
            _ => value.clone(),
        }
    }
    (raw.is_object() || raw.is_array()).then(|| visit(raw, 0, ""))
}

pub(super) fn collect_labels(
    value: &Value,
    labels: &mut Labels,
    depth: usize,
    parent_course: &str,
) {
    if depth > 8 || labels.len() >= 100 {
        return;
    }
    if let Some(items) = value.as_array() {
        for item in items {
            collect_labels(item, labels, depth + 1, parent_course);
        }
    }
    if let Some(items) = value.as_object() {
        let course = value["courseName"]
            .as_str()
            .or_else(|| value["course"]["name"].as_str())
            .unwrap_or(parent_course);
        let full_name = [value["firstName"].as_str(), value["lastName"].as_str()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let name = [
            value["name"].as_str(),
            value["studentName"].as_str(),
            value["student"]["name"].as_str(),
            value["courseName"].as_str(),
            Some(&full_name),
        ]
        .into_iter()
        .flatten()
        .find(|name| !name.trim().is_empty() && !name.contains(char::is_control));
        if let Some(name) = name {
            for key in ["id", "studentId", "enrollmentId", "courseId", "eventId"] {
                if labels.len() >= 100 {
                    break;
                }
                if key == "courseId" && course.is_empty()
                    || key == "eventId" && !value["kind"].is_string()
                    || matches!(key, "studentId" | "enrollmentId") && name == course
                {
                    continue;
                }
                if let Some(id) = value[key].as_str().filter(|id| schema::uuid(id)) {
                    labels.entry(id.into()).or_insert_with(|| match key {
                        "courseId" => bounded(course, 200),
                        "enrollmentId" if !course.is_empty() => {
                            bounded(&format!("{name} · {course}"), 200)
                        }
                        _ => bounded(name, 200),
                    });
                }
            }
            if labels.len() < 100
                && let Some(id) = value["enrollment"]["id"]
                    .as_str()
                    .filter(|id| schema::uuid(id))
            {
                labels.entry(id.into()).or_insert_with(|| {
                    if course.is_empty() {
                        name.into()
                    } else {
                        bounded(&format!("{name} · {course}"), 200)
                    }
                });
            }
        }
        if labels.len() < 100
            && let (Some(id), Some(name)) =
                (value["teacherId"].as_str(), value["teacherName"].as_str())
            && schema::uuid(id)
            && !name.is_empty()
            && !name.contains(char::is_control)
        {
            labels
                .entry(id.into())
                .or_insert_with(|| bounded(name, 200));
        }
        let payload = value.get("payload").unwrap_or(value);
        if labels.len() < 100
            && value["undone"] != true
            && let (Some(id), Some(kind), Some(date)) = (
                value["id"].as_str().or_else(|| value["eventId"].as_str()),
                payload["kind"].as_str(),
                payload["date"].as_str(),
            )
            && schema::uuid(id)
            && schema::date(date)
            && matches!(kind, "attendance" | "note")
        {
            let student = payload["studentId"].as_str().and_then(|id| labels.get(id));
            let label = bounded(
                &[Some(kind), student.map(String::as_str), Some(date)]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · "),
                200,
            );
            labels.entry(id.into()).or_insert(label);
        }
        for (key, item) in items {
            if key != "target" && (key != "event" || value["undone"] != true) {
                collect_labels(item, labels, depth + 1, course);
            }
        }
    }
}

fn display(value: &Value, lang: Language) -> String {
    match value {
        Value::Null => lang.text("Blank", "فارغ").into(),
        Value::Bool(true) => lang.text("Yes", "نعم").into(),
        Value::Bool(false) => lang.text("No", "لا").into(),
        Value::String(value) => field(value, lang).replace(['\r', '\n', '\t'], " "),
        _ => value.to_string(),
    }
}

pub(super) fn render_result(value: &Value, lang: Language) -> String {
    fn visit(value: &Value, key: &str, depth: usize, lines: &mut Vec<String>, lang: Language) {
        if depth > 8 || lines.len() >= 60 {
            return;
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    visit(item, key, depth + 1, lines, lang);
                }
            }
            Value::Object(items) => {
                for (key, item) in items {
                    if key != "id" && !key.ends_with("Id") {
                        visit(item, key, depth + 1, lines, lang);
                    }
                }
            }
            Value::Null => {}
            _ if !key.is_empty() => {
                lines.push(format!("{}: {}", field(key, lang), display(value, lang)))
            }
            _ => {}
        }
    }
    let mut lines = Vec::new();
    visit(value, "", 0, &mut lines, lang);
    if lines.is_empty() {
        return lang
            .text("No results to display.", "لا توجد نتائج للعرض.")
            .into();
    }
    bounded(&lines.join("\n"), 3500)
}

pub(super) fn preview(name: Name, args: &Value, lang: Language, labels: &Labels) -> Result<String> {
    fn visit(
        value: &Value,
        key: &str,
        parent: &str,
        labels: &Labels,
        lines: &mut Vec<String>,
        lang: Language,
    ) -> Result<()> {
        if matches!(
            key,
            "expected"
                | "expectedEventId"
                | "expectedRevision"
                | "clientTime"
                | "requestId"
                | "edit"
        ) {
            return Ok(());
        }
        match value {
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    if items.len() > 1 {
                        lines.push(format!("{} {}", lang.text("Item", "عنصر"), index + 1));
                    }
                    visit(item, key, parent, labels, lines, lang)?;
                }
            }
            Value::Object(items) => {
                for (field, item) in items {
                    visit(item, field, key, labels, lines, lang)?;
                }
            }
            Value::String(id) if key == "studentId" && parent == "payload" && id.is_empty() => {
                lines.push(format!(
                    "{}: {}",
                    field(key, lang),
                    lang.text("General notebook", "دفتر المتابعة العام")
                ));
            }
            Value::String(id)
                if matches!(
                    key,
                    "studentId" | "courseId" | "enrollmentId" | "teacherId" | "targetId"
                ) || key == "id" && parent == "input" =>
            {
                let label = labels.get(id).ok_or("unresolved agent target")?;
                lines.push(format!("{}: {label}", field(key, lang)));
            }
            _ if key != "id" => {
                lines.push(format!("{}: {}", field(key, lang), display(value, lang)))
            }
            _ => {}
        }
        Ok(())
    }
    let title = name.title(lang);
    let mut lines = Vec::new();
    visit(args, "", "", labels, &mut lines, lang)?;
    if title.is_empty() || lines.is_empty() {
        return Err("agent write has no preview".into());
    }
    let mut text = format!(
        "{}: {title}\n{}",
        lang.text("Confirm", "تأكيد"),
        lines.join("\n")
    );
    if name == Name::SetCourseStatus && args["input"]["open"] != true && args["open"] != true {
        text.push_str(lang.text(
            "\nClosing the course may advance eligible students’ levels.",
            "\nقد يؤدي إغلاق الدورة إلى ترقية مستويات الطلاب المؤهلين.",
        ));
    }
    if text.encode_utf16().count() > 3500 {
        return Err("agent preview too large".into());
    }
    Ok(text)
}
