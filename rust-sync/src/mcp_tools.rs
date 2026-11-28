use serde_json::{Value, json};

use crate::ledger::{js_trim, valid_uuid};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Student,
    Teacher,
    Administrator,
}

pub(crate) struct Access {
    pub role: Role,
    pub read_scope: bool,
    pub write_scope: bool,
    pub can_read: bool,
    pub can_write: bool,
    pub ready: bool,
    pub invitation: bool,
    pub user_id: String,
    pub name: String,
}

pub(crate) struct Operation {
    pub path: &'static str,
    pub method: &'static str,
    pub body: Option<Value>,
    pub edit: Option<EditVersion>,
    pub response: ResponseMapping,
}

pub(crate) struct EditVersion {
    pub time: String,
    pub id: String,
}

#[derive(PartialEq, Eq)]
pub(crate) enum ResponseMapping {
    Direct,
    Courses,
    History(Value),
    SavedCount(usize),
    FindStudents(Value),
    StudentDetails(Value),
    AttendanceSummary,
    AttendanceView(Value),
    CourseWorkflow(Value),
    CourseExport(Value),
    Invitation,
    Handoff,
}

fn tools(access: &Access) -> Vec<(&'static str, &'static str)> {
    let mut result = Vec::new();
    if access.invitation && !access.ready {
        result.push(("complete_invitation_setup", "/mcp/handoff/setup"));
    }
    if access.invitation && access.ready && access.write_scope {
        result.push(("open_invitation_profile", "/mcp/handoff/profile"));
    }
    if admin_write(access) {
        result.extend([
            ("open_password_reset", "/mcp/handoff/reset"),
            ("verify_account_phone", "/api/admin/accounts/verify-phone"),
        ]);
    }
    if access.read_scope {
        result.extend([
            ("get_account", "/api/me"),
            ("get_profile", "/api/mcp/profile"),
        ]);
    }
    if access.write_scope {
        result.push(("update_profile", "/api/mcp/profile"));
        if access.invitation && access.ready {
            result.push(("update_invitation_profile", "/api/me/invitation-profile"));
        }
    }
    if !access.ready {
        return result;
    }
    if access.role == Role::Student {
        if access.read_scope {
            result.push(("get_my_attendance", "/api/student/attendance"));
        }
        return result;
    }
    if access.read_scope && access.can_read {
        result.extend([
            ("list_courses", "/api/notebook"),
            ("find_students", "/api/notebook"),
            ("get_student_details", "/api/notebook"),
            ("get_course_attendance_view", "/api/notebook"),
            ("get_notebook", "/api/notebook"),
            ("get_attendance_summary", "/api/notebook"),
        ]);
    }
    if access.write_scope && access.can_write {
        result.extend([
            ("update_student", "/api/student/update"),
            ("append_events", "/api/events"),
        ]);
        if access.read_scope && access.can_read {
            result.extend([
                ("sync_attendance", "/api/attendance/sync"),
                ("sync_student", "/api/student/sync"),
            ]);
        }
    }
    if access.role == Role::Administrator
        && access.can_read
        && access.can_write
        && access.read_scope
    {
        result.extend([
            ("get_course_workflow", "/mcp/course-workflow"),
            ("get_notebook_history", "/api/notebook"),
            ("get_admin_board", "/api/admin"),
            ("export_course_report", "/mcp/course-report"),
        ]);
    }
    if admin_write(access) {
        result.extend([
            ("invite_accounts", "/api/admin/accounts/invite"),
            ("grant_account", "/api/admin/accounts/grant"),
            ("disable_account", "/api/admin/accounts/disable"),
            (
                "reset_account_password",
                "/api/admin/accounts/reset-password",
            ),
            ("save_student", "/api/admin/students"),
            ("create_student_in_course", "/api/admin/students/enroll"),
            ("save_course", "/api/admin/courses"),
            ("set_course_status", "/api/admin/courses/status"),
            ("set_course_thresholds", "/api/admin/courses/settings"),
            ("save_enrollment", "/api/admin/enrollments/save"),
            ("save_grades", "/api/admin/enrollments/grades"),
            ("save_grade_decision", "/api/admin/enrollments/decision"),
            ("set_handbook_received", "/api/admin/enrollments/handbook"),
        ]);
    }
    result
}

pub(crate) fn admin_write(access: &Access) -> bool {
    access.ready
        && access.role == Role::Administrator
        && access.read_scope
        && access.write_scope
        && access.can_read
        && access.can_write
}

/// Wire metadata emitted by the legacy MCP SDK. Authorization and argument
/// validation remain in the native dispatcher.
static CATALOG: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("mcp_tool_catalog.json")).expect("valid MCP tool catalog")
});

pub(crate) fn discovery(access: &Access, year: i32) -> Vec<Value> {
    tools(access)
        .into_iter()
        .map(|(name, _)| {
            let mut tool = CATALOG[name].clone();
            update_birth_year(&mut tool["inputSchema"], year);
            tool
        })
        .collect()
}

fn update_birth_year(schema: &mut Value, year: i32) {
    match schema {
        Value::Array(items) => {
            for item in items {
                update_birth_year(item, year);
            }
        }
        Value::Object(properties) => {
            if properties
                .get("minimum")
                .is_some_and(|number| number == 1900)
            {
                properties.insert("maximum".into(), json!(year));
            }
            for value in properties.values_mut() {
                update_birth_year(value, year);
            }
        }
        _ => {}
    }
}

const FIELDS: [&str; 6] = [
    "firstName",
    "lastName",
    "group",
    "whatsapp",
    "location",
    "birthYear",
];

fn normalize_fields(input: &Value, update: bool, year: i32) -> Result<Value, ()> {
    let fields = input.as_object().ok_or(())?;
    let mut output = serde_json::Map::new();
    for field in FIELDS {
        let value = fields.get(field).ok_or(())?;
        if field == "birthYear" {
            if value.is_null() {
                output.insert(field.into(), Value::Null);
                continue;
            }
            let number = value
                .as_f64()
                .filter(|n| n.fract() == 0.0 && n.abs() <= 9007199254740991.0)
                .ok_or(())?;
            if update && !(1900.0..=f64::from(year)).contains(&number) {
                return Err(());
            }
            output.insert(field.into(), json!(number as i64));
            continue;
        }
        if value.is_null() && matches!(field, "whatsapp" | "location") {
            output.insert(field.into(), Value::Null);
            continue;
        }
        let text = value.as_str().ok_or(())?;
        let text = if field == "location" && !update {
            text
        } else {
            js_trim(text)
        };
        let length = text.encode_utf16().count();
        if update
            && (length
                > match field {
                    "group" => 100,
                    "whatsapp" => 40,
                    _ => 200,
                }
                || (field == "firstName" && length == 0))
        {
            return Err(());
        }
        output.insert(field.into(), json!(text));
    }
    Ok(Value::Object(output))
}

fn edit_version(value: Option<&Value>) -> Result<Option<EditVersion>, ()> {
    let Some(value) = value else { return Ok(None) };
    let edit = value.as_object().filter(|edit| edit.len() == 2).ok_or(())?;
    let time = edit
        .get("time")
        .and_then(Value::as_str)
        .filter(|time| crate::ledger::iso_datetime(time))
        .ok_or(())?;
    let id = edit
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_uuid(id))
        .ok_or(())?;
    Ok(Some(EditVersion {
        time: time.to_owned(),
        id: id.to_owned(),
    }))
}

pub(crate) fn operation(
    access: &Access,
    name: &str,
    arguments: &Value,
    year: i32,
) -> Result<Operation, ()> {
    let (_, path) = tools(access)
        .into_iter()
        .find(|(tool, _)| *tool == name)
        .ok_or(())?;
    if !arguments.is_object() {
        return Err(());
    }
    if path.starts_with("/mcp/handoff/") {
        if name == "open_password_reset"
            && arguments.get("authUserId").is_some_and(|value| {
                !value
                    .as_str()
                    .is_some_and(|id| (1..=256).contains(&id.encode_utf16().count()))
            })
        {
            return Err(());
        }
        return Ok(Operation {
            path,
            method: "GET",
            body: Some(arguments.clone()),
            edit: None,
            response: ResponseMapping::Handoff,
        });
    }
    if path.starts_with("/api/admin/") || path == "/api/me/invitation-profile" {
        let input = arguments
            .get("input")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or(())?;
        return Ok(Operation {
            path,
            method: "POST",
            body: Some(input),
            edit: if matches!(name, "invite_accounts" | "verify_account_phone") {
                None
            } else {
                edit_version(arguments.get("edit"))?
            },
            response: if name == "invite_accounts" {
                ResponseMapping::Invitation
            } else {
                ResponseMapping::Direct
            },
        });
    }
    if matches!(name, "append_events" | "sync_attendance") {
        return attendance_operation(access, name, arguments, path);
    }
    if name == "update_profile" {
        let input = arguments
            .get("input")
            .filter(|input| crate::auth_profile::profile_valid(input))
            .ok_or(())?;
        return Ok(Operation {
            path,
            method: "POST",
            body: Some(input.clone()),
            edit: edit_version(arguments.get("edit"))?,
            response: ResponseMapping::Direct,
        });
    }
    if name == "sync_student" {
        let input = arguments
            .get("operation")
            .and_then(Value::as_object)
            .ok_or(())?;
        let operation = match input.get("kind").and_then(Value::as_str) {
            Some("note") if input.len() == 2 => {
                let event = event(input.get("event").ok_or(())?, &access.name)?;
                if event["payload"]["kind"] != "note"
                    || !event["payload"]["studentId"]
                        .as_str()
                        .is_some_and(valid_uuid)
                {
                    return Err(());
                }
                json!({"kind":"note", "event":event})
            }
            Some("profile-v1") => Value::Object(input.clone()),
            _ => return Err(()),
        };
        return Ok(Operation {
            path,
            method: "POST",
            body: Some(json!({"accountId":access.user_id, "operation":operation})),
            edit: None,
            response: ResponseMapping::Direct,
        });
    }
    if name != "update_student" {
        return Ok(Operation {
            path,
            method: "GET",
            body: None,
            edit: None,
            response: match name {
                "list_courses" => ResponseMapping::Courses,
                "get_notebook_history" => ResponseMapping::History(history_filter(arguments)?),
                "find_students" => {
                    ResponseMapping::FindStudents(crate::mcp_notebook::directory_filter(arguments)?)
                }
                "get_student_details" => {
                    ResponseMapping::StudentDetails(crate::mcp_notebook::details_filter(arguments)?)
                }
                "get_attendance_summary" => ResponseMapping::AttendanceSummary,
                "get_course_attendance_view" => ResponseMapping::AttendanceView(
                    crate::mcp_notebook::attendance_filter(arguments)?,
                ),
                "get_course_workflow" => {
                    ResponseMapping::CourseWorkflow(crate::mcp_courses::workflow_filter(arguments)?)
                }
                "export_course_report" => {
                    ResponseMapping::CourseExport(crate::mcp_courses::export_filter(arguments)?)
                }
                _ => ResponseMapping::Direct,
            },
        });
    }
    let input = arguments.as_object().ok_or(())?;
    if input.len() != 8 + usize::from(input.contains_key("edit"))
        || input.keys().any(|key| {
            !FIELDS.contains(&key.as_str())
                && !matches!(key.as_str(), "studentId" | "expected" | "edit")
        })
    {
        return Err(());
    }
    let id = input
        .get("studentId")
        .and_then(Value::as_str)
        .filter(|id| valid_uuid(id))
        .ok_or(())?;
    let expected = input.get("expected").and_then(Value::as_object).ok_or(())?;
    if expected.len() != FIELDS.len() || expected.keys().any(|key| !FIELDS.contains(&key.as_str()))
    {
        return Err(());
    }
    let expected = normalize_fields(&Value::Object(expected.clone()), false, year)?;
    let mut body = normalize_fields(arguments, true, year)?;
    let first = body["firstName"].as_str().ok_or(())?;
    let last = body["lastName"].as_str().ok_or(())?;
    if first.encode_utf16().count()
        + if last.is_empty() {
            0
        } else {
            1 + last.encode_utf16().count()
        }
        > 200
    {
        return Err(());
    }
    body["studentId"] = json!(id);
    body["expected"] = expected;
    Ok(Operation {
        path,
        method: "POST",
        body: Some(body),
        edit: edit_version(input.get("edit"))?,
        response: ResponseMapping::Direct,
    })
}

fn event(input: &Value, name: &str) -> Result<Value, ()> {
    let source = input.as_object().ok_or(())?;
    let mut event = serde_json::Map::new();
    for key in ["id", "clientTime", "payload"] {
        event.insert(key.into(), source.get(key).ok_or(())?.clone());
    }
    if let Some(time) = source.get("editTime") {
        event.insert("editTime".into(), time.clone());
    }
    event.insert("actor".into(), json!(name));
    event.insert("receivedAt".into(), json!(""));
    crate::ledger::decode_input(&Value::Object(event)).map_err(|_| ())
}

fn attendance_operation(
    access: &Access,
    name: &str,
    arguments: &Value,
    path: &'static str,
) -> Result<Operation, ()> {
    let key = if name == "append_events" {
        "events"
    } else {
        "records"
    };
    let records = arguments.get(key).and_then(Value::as_array).ok_or(())?;
    if !(1..=500).contains(&records.len()) {
        return Err(());
    }
    let mut parsed = Vec::with_capacity(records.len());
    for record in records {
        if name == "append_events" {
            parsed.push(event(record, &access.name)?);
            continue;
        }
        let input = record.as_object().ok_or(())?;
        let event = event(input.get("event").ok_or(())?, &access.name)?;
        let payload = &event["payload"];
        if payload["kind"] != "attendance"
            || payload.get("enrollmentId").is_none()
            || !matches!(
                payload["status"].as_str(),
                Some("present" | "absent" | "excused")
            )
        {
            return Err(());
        }
        let expected = input.get("expectedEventId").ok_or(())?;
        if !expected.is_null() && !expected.as_str().is_some_and(valid_uuid) {
            return Err(());
        }
        let mut result = json!({"event": event, "expectedEventId": expected});
        if let Some(revision) = input.get("expectedRevision") {
            if revision != "unknown" {
                return Err(());
            }
            result["expectedRevision"] = revision.clone();
        }
        if let Some(time) = input.get("editTime") {
            if !time.as_str().is_some_and(crate::ledger::iso_datetime) {
                return Err(());
            }
            result["editTime"] = time.clone();
        }
        parsed.push(result);
    }
    let body = if name == "append_events" {
        json!({"events": parsed})
    } else {
        json!({"accountId": access.user_id, "records": parsed})
    };
    let response = if name == "append_events" && !access.read_scope {
        ResponseMapping::SavedCount(records.len())
    } else {
        ResponseMapping::Direct
    };
    Ok(Operation {
        path,
        method: "POST",
        body: Some(body),
        edit: None,
        response,
    })
}

pub(crate) fn map_success(
    mapping: &ResponseMapping,
    value: Value,
    origin: &str,
) -> Result<Value, &'static str> {
    match mapping {
        ResponseMapping::Direct => Ok(value),
        ResponseMapping::SavedCount(count) => Ok(json!({"saved": count})),
        ResponseMapping::History(filter) => history(&value, filter),
        ResponseMapping::FindStudents(filter) => crate::mcp_notebook::find_students(&value, filter),
        ResponseMapping::StudentDetails(filter) => {
            crate::mcp_notebook::student_details(&value, filter)
        }
        ResponseMapping::AttendanceSummary => crate::mcp_notebook::attendance_summary(&value),
        ResponseMapping::AttendanceView(filter) => {
            crate::mcp_notebook::attendance_view(&value, filter, origin)
        }
        ResponseMapping::CourseWorkflow(filter) => crate::mcp_courses::workflow(&value, filter),
        ResponseMapping::CourseExport(_)
        | ResponseMapping::Invitation
        | ResponseMapping::Handoff => Ok(value),
        ResponseMapping::Courses => {
            let courses = value
                .get("courses")
                .and_then(Value::as_array)
                .ok_or("Invalid response")?;
            let mut result = Vec::with_capacity(courses.len());
            for course in courses {
                let item = course.as_object().ok_or("Invalid response")?;
                result.push(json!({"id": item.get("id").ok_or("Invalid response")?, "name": item.get("name").ok_or("Invalid response")?, "open": item.get("open").ok_or("Invalid response")?}));
            }
            Ok(json!(result))
        }
    }
}

fn history_filter(arguments: &Value) -> Result<Value, ()> {
    let input = arguments.as_object().ok_or(())?;
    let course = input.get("courseId").and_then(Value::as_str).ok_or(())?;
    if course != "none" && !valid_uuid(course) {
        return Err(());
    }
    let mut filter = json!({"courseId":course});
    for key in ["studentId", "kind", "from", "to"] {
        if let Some(value) = input.get(key) {
            let text = value.as_str().ok_or(())?;
            let valid = match key {
                "studentId" => valid_uuid(text),
                "kind" => matches!(text, "student" | "attendance" | "note" | "reversal"),
                _ => crate::ledger::iso_datetime(&format!("{text}T00:00:00Z")),
            };
            if !valid {
                return Err(());
            }
            filter[key] = value.clone();
        }
    }
    for (key, default, max) in [("offset", 0, 9007199254740991u64), ("limit", 50, 100)] {
        let number = if let Some(value) = input.get(key) {
            value
                .as_f64()
                .filter(|v| {
                    v.fract() == 0.0
                        && *v >= if key == "limit" { 1.0 } else { 0.0 }
                        && *v <= max as f64
                })
                .ok_or(())? as u64
        } else {
            default
        };
        filter[key] = json!(number);
    }
    Ok(filter)
}

fn history(snapshot: &Value, filter: &Value) -> Result<Value, &'static str> {
    use std::collections::{HashMap, HashSet};
    let course = filter["courseId"].as_str().ok_or("Invalid response")?;
    let courses = snapshot["courses"].as_array().ok_or("Invalid response")?;
    if course != "none" && !courses.iter().any(|item| item["id"] == course) {
        return Err("Course not found");
    }
    let enrollments = snapshot["enrollments"]
        .as_array()
        .ok_or("Invalid response")?;
    let ids: HashSet<&str> = enrollments
        .iter()
        .filter(|item| item["courseId"] == course)
        .filter_map(|item| item["id"].as_str())
        .collect();
    let events = snapshot["events"].as_array().ok_or("Invalid response")?;
    let mut by_id = HashMap::new();
    let mut unique = Vec::new();
    let mut reversed = HashSet::new();
    for event in events {
        let id = event["id"].as_str().ok_or("Invalid response")?;
        if by_id.contains_key(id) {
            continue;
        }
        if event["payload"]["kind"] == "reversal"
            && let Some(target_id) = event["payload"]["targetId"].as_str()
            && by_id
                .get(target_id)
                .is_some_and(|target: &&Value| target["payload"]["kind"] != "reversal")
        {
            reversed.insert(target_id);
        }
        unique.push(event);
        by_id.insert(id, event);
    }
    let mut rows = Vec::new();
    for event in unique.into_iter().rev() {
        let payload = &event["payload"];
        let target = if payload["kind"] == "reversal" {
            payload["targetId"]
                .as_str()
                .and_then(|id| by_id.get(id).copied())
        } else {
            Some(event)
        };
        let target_payload = target.map(|item| &item["payload"]);
        if let Some(p) = target_payload
            && p["kind"] == "attendance"
        {
            let enrollment = p.get("enrollmentId").and_then(Value::as_str);
            if (course == "none" && enrollment.is_some())
                || (course != "none" && !enrollment.is_some_and(|id| ids.contains(id)))
            {
                continue;
            }
        }
        if let Some(student) = filter.get("studentId")
            && !target_payload
                .is_some_and(|p| p["kind"] != "reversal" && &p["studentId"] == student)
        {
            continue;
        }
        if filter
            .get("kind")
            .is_some_and(|kind| &payload["kind"] != kind)
        {
            continue;
        }
        let date = target_payload
            .and_then(|p| p.get("date"))
            .and_then(Value::as_str);
        if filter
            .get("from")
            .and_then(Value::as_str)
            .is_some_and(|from| date.is_none_or(|day| day < from))
            || filter
                .get("to")
                .and_then(Value::as_str)
                .is_some_and(|to| date.is_none_or(|day| day > to))
        {
            continue;
        }
        rows.push(json!({"event":event,"undone":event["id"].as_str().is_some_and(|id| reversed.contains(id)),"target":if payload["kind"] == "reversal" {target} else {None}}));
    }
    let offset = filter["offset"].as_u64().ok_or("Invalid response")? as usize;
    let limit = filter["limit"].as_u64().ok_or("Invalid response")? as usize;
    Ok(
        json!({"total":rows.len(),"offset":offset,"records":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>()}),
    )
}

pub(crate) fn map_failure(mapping: &ResponseMapping, message: &str) -> String {
    match mapping {
        ResponseMapping::Courses => "Unable to load courses",
        ResponseMapping::FindStudents(_) => "Unable to load students",
        ResponseMapping::StudentDetails(_) => "Unable to load student",
        ResponseMapping::Invitation => "Invitation could not be confirmed. Check accounts before retrying; reset a password if necessary.",
        _ => message,
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(role: Role, read_scope: bool, write_scope: bool) -> Access {
        Access {
            role,
            read_scope,
            write_scope,
            can_read: true,
            can_write: true,
            ready: true,
            invitation: false,
            user_id: "user-1".into(),
            name: "Teacher".into(),
        }
    }

    #[test]
    fn discovery_enforces_roles_scopes_and_setup() {
        let student = access(Role::Student, true, true);
        let names: Vec<_> = discovery(&student, 2026)
            .into_iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            [
                "get_account",
                "get_profile",
                "update_profile",
                "get_my_attendance"
            ]
        );
        assert!(operation(&student, "get_notebook", &json!({}), 2026).is_err());
        let mut teacher = access(Role::Teacher, true, false);
        assert!(operation(&teacher, "update_student", &json!({}), 2026).is_err());
        assert_eq!(
            operation(&teacher, "get_notebook", &json!({}), 2026)
                .unwrap()
                .path,
            "/api/notebook"
        );
        teacher.ready = false;
        assert!(operation(&teacher, "get_notebook", &json!({}), 2026).is_err());
        assert_eq!(discovery(&teacher, 2026).len(), 2);
    }

    #[test]
    fn attendance_scopes_and_request_transform() {
        let mut teacher = access(Role::Teacher, true, true);
        let event = json!({"id":"e4856c4d-a0ad-4f0c-af15-f8f5e24dddac", "clientTime":"2026-11-27T10:00:00.000Z", "payload":{"kind":"attendance", "studentId":"e4856c4d-a0ad-4f0c-af15-f8f5e24dddac", "enrollmentId":"e4856c4d-a0ad-4f0c-af15-f8f5e24dddac", "date":"2026-11-27", "status":"present", "ignored":true}, "ignored":true});
        let input = json!({"records":[{"event":event,"expectedEventId":null,"ignored":true}]});
        let request = operation(&teacher, "sync_attendance", &input, 2026).unwrap();
        let body = request.body.unwrap();
        assert_eq!(body["accountId"], "user-1");
        assert_eq!(body["records"][0]["event"]["actor"], "Teacher");
        assert!(body["records"][0]["event"].get("ignored").is_none());
        teacher.read_scope = false;
        assert!(operation(&teacher, "sync_attendance", &input, 2026).is_err());
        let append =
            operation(&teacher, "append_events", &json!({"events":[event]}), 2026).unwrap();
        assert!(append.response == ResponseMapping::SavedCount(1));
        assert_eq!(
            map_success(
                &append.response,
                json!({"private":true}),
                "https://wartaq.example"
            )
            .unwrap(),
            json!({"saved":1})
        );
    }

    #[test]
    fn history_resolves_reversal_and_course_scope() {
        let admin = access(Role::Administrator, true, true);
        let course_id = "e4856c4d-a0ad-4f0c-af15-f8f5e24dddac";
        let request = operation(
            &admin,
            "get_notebook_history",
            &json!({"courseId":course_id}),
            2026,
        )
        .unwrap();
        let snapshot = json!({"courses":[{"id":course_id}], "enrollments":[{"id":"entry", "courseId":course_id}], "events":[
            {"id":"event-a","payload":{"kind":"attendance","studentId":"student","enrollmentId":"entry","date":"2026-11-27"}},
            {"id":"event-b","payload":{"kind":"reversal","targetId":"event-a"}}
        ]});
        let result = map_success(&request.response, snapshot, "https://wartaq.example").unwrap();
        assert_eq!(result["total"], 2);
        assert_eq!(result["records"][0]["target"]["id"], "event-a");
        assert_eq!(result["records"][1]["undone"], true);
        assert_eq!(
            map_failure(&ResponseMapping::Courses, "ignored"),
            "Unable to load courses"
        );
        assert!(
            operation(
                &access(Role::Teacher, true, true),
                "get_notebook_history",
                &json!({"courseId":course_id}),
                2026
            )
            .is_err()
        );
    }

    #[test]
    fn student_update_validates_and_normalizes() {
        let teacher = access(Role::Teacher, false, true);
        let mut input = json!({"studentId":"e4856c4d-a0ad-4f0c-af15-f8f5e24dddac","firstName":"  Ada  ","lastName":"  Lovelace ","group":" A ","whatsapp": null,"location":" Here ","birthYear":2000,"expected":{"firstName":" Ada ","lastName":"Lovelace","group":"A","whatsapp":null,"location":" Untrimmed ","birthYear":2000}});
        let operation = operation(&teacher, "update_student", &input, 2026).unwrap();
        assert_eq!(operation.method, "POST");
        assert_eq!(operation.body.unwrap()["location"], "Here");
        input["extra"] = json!(true);
        assert!(super::operation(&teacher, "update_student", &input, 2026).is_err());
    }
}
