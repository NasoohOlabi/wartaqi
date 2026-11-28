use std::collections::{HashMap, HashSet};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use crate::{
    ledger,
    mcp_notebook::{Projection, array, choice, integer, optional_uuid, rank_students, string},
    mcp_operations::text,
};

pub(crate) fn workflow_filter(args: &Value) -> Result<Value, ()> {
    let course = optional_uuid(args, "courseId")?.ok_or(())?;
    let view = choice(
        args,
        "view",
        "grades",
        &["membership", "grades", "readiness"],
    )?;
    let search = bounded_text(args, "search", 200)?.unwrap_or("");
    let offset = integer(args, "offset", 0, 0, 9_007_199_254_740_991)?;
    let limit = integer(args, "limit", 50, 1, 100)?;
    let missing = args
        .get("missingScores")
        .map(|value| value.as_bool().ok_or(()))
        .transpose()?
        .unwrap_or(false);
    Ok(
        json!({"courseId":course,"view":view,"search":search,"offset":offset,"limit":limit,"missingScores":missing}),
    )
}

fn bounded_text<'a>(args: &'a Value, key: &str, max: usize) -> Result<Option<&'a str>, ()> {
    args.get(key)
        .map(|value| {
            value
                .as_str()
                .filter(|text| text.encode_utf16().count() <= max)
                .ok_or(())
        })
        .transpose()
}

fn date_filter<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, ()> {
    args.get(key)
        .map(|value| value.as_str().filter(|date| ledger::date(date)).ok_or(()))
        .transpose()
}

const IDENTITY: &[&str] = &["firstName", "lastName", "level", "enrolled"];
const GRADE: &[&str] = &[
    "theory",
    "practical",
    "present",
    "absent",
    "excused",
    "total",
    "ratio",
    "result",
];
const STUDENT: &[&str] = &[
    "rosterLevel",
    "studentActive",
    "teacher",
    "handbook",
    "whatsapp",
    "location",
    "birthYear",
];

fn columns(report: &str, defaults: bool) -> Vec<&'static str> {
    let detail = match (report, defaults) {
        ("full", false) => [GRADE, STUDENT].concat(),
        ("full", true) | ("grades", false) => GRADE.to_vec(),
        ("grades", true) => return vec!["firstName", "lastName", "theory", "practical", "result"],
        ("students", false) => STUDENT.to_vec(),
        ("students", true) => Vec::new(),
        ("attendance", _) => vec!["date", "mark"],
        ("participants", _) => vec!["theory", "practical"],
        _ => unreachable!("validated course report"),
    };
    [IDENTITY, detail.as_slice()].concat()
}

pub(crate) fn export_filter(args: &Value) -> Result<Value, ()> {
    let course = optional_uuid(args, "courseId")?.ok_or(())?;
    let report = choice(
        args,
        "report",
        "full",
        &["full", "grades", "students", "attendance", "participants"],
    )?;
    let format = choice(args, "format", "rows", &["rows", "xlsx"])?;
    let language = choice(args, "language", "en", &["en", "ar"])?;
    let enrollment = choice(args, "enrollment", "all", &["all", "active", "withdrawn"])?;
    let student = optional_uuid(args, "studentId")?;
    let level = bounded_text(args, "level", 200)?;
    let coverage = choice(
        args,
        "coverage",
        "all",
        &["all", "any", "both", "theory", "practical", "none"],
    )?;
    let result = choice(args, "result", "all", &["all", "pass", "fail", "pending"])?;
    let from = date_filter(args, "from")?;
    let to = date_filter(args, "to")?;
    let mark = choice(
        args,
        "mark",
        "all",
        &["all", "present", "absent", "excused"],
    )?;
    if (report != "attendance" && (from.is_some() || to.is_some() || mark != "all"))
        || from.zip(to).is_some_and(|(from, to)| from > to)
    {
        return Err(());
    }
    let cols = if let Some(list) = args.get("columns") {
        let allowed = columns(report, false);
        let list = list
            .as_array()
            .filter(|list| !list.is_empty() && list.len() <= allowed.len())
            .ok_or(())?;
        let mut used = HashSet::new();
        list.iter()
            .map(|column| {
                column
                    .as_str()
                    .filter(|name| allowed.contains(name) && used.insert(*name))
                    .ok_or(())
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        columns(report, true)
    };
    Ok(
        json!({"courseId":course,"report":report,"format":format,"language":language,"columns":cols,"enrollment":enrollment,"studentId":student,"level":level,"coverage":coverage,"result":result,"from":from,"to":to,"mark":mark}),
    )
}

struct Dataset<'a> {
    course: &'a Value,
    entries: Vec<&'a Value>,
    admin: &'a Value,
    projection: Projection<'a>,
}

impl<'a> Dataset<'a> {
    fn new(data: &'a Value, course_id: &str) -> Result<Self, &'static str> {
        let notebook = &data["notebook"];
        let course = array(notebook, "courses")?
            .iter()
            .find(|course| course["id"] == course_id)
            .ok_or("Course not found")?;
        let entries = array(notebook, "enrollments")?
            .iter()
            .filter(|entry| entry["courseId"] == course_id)
            .collect();
        let projection = Projection::new(array(notebook, "events")?);
        Ok(Self {
            course,
            entries,
            admin: &data["admin"],
            projection,
        })
    }

    fn marks_by_enrollment(&self) -> HashMap<&str, Vec<&Value>> {
        let mut marks = HashMap::<_, Vec<_>>::new();
        for (_, value) in self.projection.attendance() {
            if let Some(id) = value["enrollmentId"].as_str() {
                marks.entry(id).or_default().push(value);
            }
        }
        marks
    }
}

struct Attendance {
    present: usize,
    absent: usize,
    excused: usize,
    total: usize,
    ratio: Option<f64>,
}

impl Attendance {
    fn from_marks(marks: &[&Value]) -> Self {
        let present = marks
            .iter()
            .filter(|value| value["status"] == "present")
            .count();
        let absent = marks
            .iter()
            .filter(|value| value["status"] == "absent")
            .count();
        let excused = marks
            .iter()
            .filter(|value| value["status"] == "excused")
            .count();
        let total = marks.len();
        let ratio = (total > 0).then(|| present as f64 / total as f64 * 100.0);
        Self {
            present,
            absent,
            excused,
            total,
            ratio,
        }
    }

    fn json(&self) -> Value {
        json!({"present":self.present,"absent":self.absent,"excused":self.excused,"total":self.total,"ratio":self.ratio})
    }
}

fn calculated_result(course: &Value, entry: &Value, ratio: Option<f64>) -> &'static str {
    match (entry["practical"].as_f64(), entry["theory"].as_f64(), ratio) {
        (Some(practical), Some(theory), Some(ratio)) => {
            let passes = practical >= course["practicalMinimum"].as_f64().unwrap_or(80.0)
                && theory >= course["theoryMinimum"].as_f64().unwrap_or(80.0)
                && ratio >= course["attendanceMinimum"].as_f64().unwrap_or(70.0);
            if passes { "pass" } else { "fail" }
        }
        _ => "pending",
    }
}

fn effective_result<'a>(entry: &'a Value, calculated: &'a str) -> &'a str {
    entry["gradeDecisions"]
        .as_array()
        .and_then(|decisions| decisions.last())
        .and_then(|decision| decision["result"].as_str())
        .filter(|result| *result != "calculated")
        .unwrap_or(calculated)
}

fn absent_scores(entry: &Value) -> bool {
    entry["active"] == true && (entry["practical"].is_null() || entry["theory"].is_null())
}

fn readiness(course: &Value, entry: &Value, marks: &[&Value]) -> [bool; 5] {
    let theory = entry["theory"].as_f64();
    let practical = entry["practical"].as_f64();
    [
        theory.is_none(),
        practical.is_none(),
        marks.is_empty(),
        theory.is_some_and(|grade| grade < course["theoryMinimum"].as_f64().unwrap_or(80.0)),
        practical.is_some_and(|grade| grade < course["practicalMinimum"].as_f64().unwrap_or(80.0)),
    ]
}

pub(crate) fn workflow(data: &Value, filter: &Value) -> Result<Value, &'static str> {
    let dataset = Dataset::new(data, string(filter, "courseId"))?;
    let entries: HashMap<_, _> = dataset
        .entries
        .iter()
        .map(|entry| (string(entry, "studentId"), *entry))
        .collect();
    let by_enrollment = dataset.marks_by_enrollment();
    let view = string(filter, "view");
    let course = dataset.course;
    let students = array(dataset.admin, "students")?;
    let accounts: HashMap<_, _> = array(dataset.admin, "accounts")?
        .iter()
        .map(|account| (string(account, "authUserId"), account))
        .collect();
    let active =
        |student: &Value, entry: &Value| student["active"] == true && entry["active"] == true;
    let matching = rank_students(
        students.iter().filter(|student| {
            if view == "membership" {
                return true;
            }
            entries
                .get(string(student, "studentId"))
                .is_some_and(|entry| {
                    if view == "readiness" {
                        active(student, entry)
                    } else {
                        filter["missingScores"] != true || absent_scores(entry)
                    }
                })
        }),
        string(filter, "search"),
    );
    let marks = |entry: &Value| {
        by_enrollment
            .get(string(entry, "id"))
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let offset = filter["offset"].as_u64().ok_or("Invalid response")? as usize;
    let limit = filter["limit"].as_u64().ok_or("Invalid response")? as usize;
    let total = matching.len();
    let mut rows = Vec::new();
    for student in matching.into_iter().skip(offset).take(limit) {
        let id = string(student, "studentId");
        let identity = json!({"studentId":id,"name":student["name"],"firstName":student["firstName"],"lastName":student["lastName"],"active":student["active"],"rosterLevel":student["group"]});
        let entry = entries.get(id).copied();
        let row = if view == "membership" {
            let enrollment = entry.map(|entry| json!({"id":entry["id"],"courseId":entry["courseId"],"studentId":entry["studentId"],"level":entry["level"],"active":entry["active"],"teacherId":entry["teacherId"],"handbookReceived":entry["handbookReceived"]}));
            json!({"student":identity,"enrollment":enrollment,"defaultLevel":entry.map_or(&student["group"], |entry| &entry["level"])})
        } else {
            let entry = entry.ok_or("Enrollment missing from course view")?;
            if view == "readiness" {
                let [theory, practical, attendance, low_theory, low_practical] =
                    readiness(course, entry, marks(entry));
                json!({"student":identity,"enrollmentId":entry["id"],"missingTheory":theory,"missingPractical":practical,"missingAttendance":attendance,"lowTheory":low_theory,"lowPractical":low_practical})
            } else {
                let totals = Attendance::from_marks(marks(entry));
                let calculated = calculated_result(course, entry, totals.ratio);
                let effective = effective_result(entry, calculated);
                let mut enrollment = entry.clone();
                if let Some(decisions) = enrollment["gradeDecisions"].as_array_mut() {
                    for decision in decisions {
                        let author = accounts.get(string(decision, "actor")).map(|account| {
                            let name = account["profileName"]
                                .as_str()
                                .unwrap_or_else(|| string(account, "name"));
                            let name = ledger::js_trim(name);
                            if name.is_empty() {
                                string(account, "verifiedPhone")
                            } else {
                                name
                            }
                        });
                        decision["authorName"] = json!(author);
                    }
                }
                json!({"student":identity,"enrollment":enrollment,"attendance":totals.json(),"calculatedResult":calculated,"effectiveResult":effective})
            }
        };
        rows.push(row);
    }
    let mut result = json!({"course":course,"view":view,"total":total,"offset":offset,"limit":limit,"rows":rows});
    match view {
        "membership" => result["teachers"] = dataset.admin["teachers"].clone(),
        "readiness" => {
            let mut total = 0;
            let mut counts = [0; 5];
            for student in students {
                if let Some(entry) = entries.get(string(student, "studentId"))
                    && active(student, entry)
                {
                    total += 1;
                    for (count, missing) in
                        counts
                            .iter_mut()
                            .zip(readiness(course, entry, marks(entry)))
                    {
                        *count += usize::from(missing);
                    }
                }
            }
            let [theory, practical, attendance, low_theory, low_practical] = counts;
            result["readiness"] = json!({"total":total,"missingTheory":theory,"missingPractical":practical,"missingAttendance":attendance,"lowTheory":low_theory,"lowPractical":low_practical});
        }
        _ => {
            let missing = dataset
                .entries
                .iter()
                .filter(|entry| absent_scores(entry))
                .count();
            result["gradesClose"] = json!({"eligibleWithSavedData":course["open"] == true && missing == 0,"missingScores":missing});
        }
    }
    Ok(result)
}

fn label(key: &str, ar: bool) -> &'static str {
    let pair = match key {
        "firstName" => ["First name", "الاسم الأول"],
        "lastName" => ["Last name", "اسم العائلة"],
        "level" => ["Course level", "مستوى الدورة"],
        "enrolled" => ["Enrolled", "مسجل"],
        "theory" => ["Theory", "نظري"],
        "practical" => ["Practical", "عملي"],
        "present" => ["Present", "حاضر"],
        "absent" => ["Absent", "غائب"],
        "excused" => ["Excused", "معذور"],
        "total" => ["Total recorded", "المجموع"],
        "ratio" => ["Attendance %", "الحضور %"],
        "result" => ["Result", "النتيجة"],
        "date" => ["Date", "التاريخ"],
        "mark" => ["Attendance", "الحضور"],
        "rosterLevel" => ["Roster level", "مستوى السجل"],
        "studentActive" => ["Student active", "الطالب نشط"],
        "whatsapp" => ["WhatsApp", "واتساب"],
        "location" => ["Location", "الموقع"],
        "birthYear" => ["Birth year", "سنة الميلاد"],
        "teacher" => ["Teacher", "المعلم"],
        "handbook" => ["Handbook received", "استلم الدليل"],
        "pass" => ["Pass", "ناجح"],
        "fail" => ["Fail", "راسب"],
        "pending" => ["Pending", "قيد الانتظار"],
        "yes" => ["Yes", "نعم"],
        "no" => ["No", "لا"],
        "open" => ["Open", "مفتوحة"],
        "closed" => ["Closed", "مغلقة"],
        _ => ["", ""],
    };
    pair[usize::from(ar)]
}

pub(crate) fn export(data: &Value, filter: &Value) -> Result<Value, &'static str> {
    let course_id = string(filter, "courseId");
    let dataset = Dataset::new(data, course_id)?;
    let course = dataset.course;
    let report = string(filter, "report");
    let ar = filter["language"] == "ar";
    let columns = array(filter, "columns")?;
    let by_enrollment = dataset.marks_by_enrollment();
    let students: HashMap<_, _> = array(dataset.admin, "students")?
        .iter()
        .map(|student| (string(student, "studentId"), student))
        .collect();
    let on = |value: bool| label(if value { "yes" } else { "no" }, ar);
    let mut rows = Vec::new();
    for entry in &dataset.entries {
        let id = string(entry, "studentId");
        let active = entry["active"] == true;
        let theory = !entry["theory"].is_null();
        let practical = !entry["practical"].is_null();
        let matches_coverage = match string(filter, "coverage") {
            "any" => theory || practical,
            "both" => theory && practical,
            "theory" => theory,
            "practical" => practical,
            "none" => !theory && !practical,
            _ => true,
        };
        if (filter["enrollment"] == "active" && !active)
            || (filter["enrollment"] == "withdrawn" && active)
            || (!filter["studentId"].is_null() && filter["studentId"] != id)
            || (!filter["level"].is_null() && filter["level"] != entry["level"])
            || !matches_coverage
            || (report == "participants" && !theory && !practical)
        {
            continue;
        }
        let student = students.get(id).copied().unwrap_or(&Value::Null);
        let marks = by_enrollment
            .get(string(entry, "id"))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let attendance = Attendance::from_marks(marks);
        let result = effective_result(entry, calculated_result(course, entry, attendance.ratio));
        if filter["result"] != "all" && filter["result"] != result {
            continue;
        }
        let row = |mark: &Value| {
            columns
                .iter()
                .map(|value| match value.as_str().unwrap_or_default() {
                    "firstName" => json!(
                        student["firstName"]
                            .as_str()
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| string(student, "name"))
                    ),
                    "lastName" => json!(string(student, "lastName")),
                    "level" => entry["level"].clone(),
                    "enrolled" => json!(on(active)),
                    "theory" => entry["theory"].clone(),
                    "practical" => entry["practical"].clone(),
                    "present" => json!(attendance.present),
                    "absent" => json!(attendance.absent),
                    "excused" => json!(attendance.excused),
                    "total" => json!(attendance.total),
                    "ratio" => json!(attendance.ratio),
                    "result" => json!(label(result, ar)),
                    "date" => json!(string(mark, "date")),
                    "mark" => json!(label(string(mark, "status"), ar)),
                    "rosterLevel" => json!(string(student, "group")),
                    "studentActive" => json!(if student.is_null() {
                        ""
                    } else {
                        on(student["active"] == true)
                    }),
                    "whatsapp" => json!(string(student, "whatsapp")),
                    "location" => json!(string(student, "location")),
                    "birthYear" => student["birthYear"].clone(),
                    "teacher" => json!(string(entry, "teacherName")),
                    "handbook" => json!(on(entry["handbookReceived"] == true)),
                    _ => Value::Null,
                })
                .collect::<Vec<_>>()
        };
        if report == "attendance" {
            for mark in marks.iter().copied().filter(|mark| {
                mark["studentId"] == id
                    && filter["from"]
                        .as_str()
                        .is_none_or(|from| string(mark, "date") >= from)
                    && filter["to"]
                        .as_str()
                        .is_none_or(|to| string(mark, "date") <= to)
                    && (filter["mark"] == "all" || filter["mark"] == mark["status"])
            }) {
                rows.push(row(mark));
            }
        } else {
            rows.push(row(&Value::Null));
        }
    }
    if filter["format"] == "rows" {
        return Ok(text(
            &json!({"course":course,"columns":columns,"rows":rows}).to_string(),
            false,
        ));
    }
    let mut sheet = Vec::new();
    if report == "full" {
        let labels = if ar {
            [
                "الدورة",
                "الحالة",
                "الحد الأدنى للحضور %",
                "درجة النجاح النظري",
                "درجة النجاح العملي",
            ]
        } else {
            [
                "Course",
                "Status",
                "Minimum attendance %",
                "Theory passing grade",
                "Practical passing grade",
            ]
        };
        sheet.push(labels.into_iter().map(Value::from).collect());
        sheet.push(vec![
            course["name"].clone(),
            json!(label(
                if course["open"] == true {
                    "open"
                } else {
                    "closed"
                },
                ar
            )),
            course["attendanceMinimum"].clone(),
            course["theoryMinimum"].clone(),
            course["practicalMinimum"].clone(),
        ]);
        sheet.push(vec![]);
    }
    sheet.push(
        columns
            .iter()
            .map(|column| json!(label(column.as_str().unwrap_or_default(), ar)))
            .collect(),
    );
    sheet.append(&mut rows);
    let blob = STANDARD.encode(crate::mcp_workbook::xlsx(&sheet, columns.len().max(5))?);
    Ok(
        json!({"content":[{"type":"resource","resource":{"uri":format!("wartaq://courses/{course_id}/{report}.xlsx"),"mimeType":"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet","blob":blob}}]}),
    )
}
