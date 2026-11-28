use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Value, json};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::ledger;

pub(crate) fn directory_filter(args: &Value) -> Result<Value, ()> {
    let query = args["query"]
        .as_str()
        .map(ledger::js_trim)
        .filter(|query| query.encode_utf16().count() <= 200)
        .ok_or(())?;
    let status = choice(args, "status", "all", &["all", "active", "inactive"])?;
    let course = optional_uuid(args, "courseId")?;
    let offset = integer(args, "offset", 0, 0, 9_007_199_254_740_991)?;
    let limit = integer(args, "limit", 20, 1, 50)?;
    Ok(json!({"query":query,"status":status,"courseId":course,"offset":offset,"limit":limit}))
}

pub(crate) fn details_filter(args: &Value) -> Result<Value, ()> {
    let student = optional_uuid(args, "studentId")?.ok_or(())?;
    let course = optional_uuid(args, "courseId")?;
    Ok(json!({"studentId":student,"courseId":course}))
}

pub(crate) fn attendance_filter(args: &Value) -> Result<Value, ()> {
    let course = args["courseId"]
        .as_str()
        .filter(|course| *course == "none" || ledger::valid_uuid(course))
        .ok_or(())?;
    let date = args["date"]
        .as_str()
        .filter(|date| ledger::date(date))
        .ok_or(())?;
    let view = choice(args, "view", "session", &["session", "overview", "chart"])?;
    let language = choice(args, "language", "en", &["en", "ar"])?;
    Ok(json!({"courseId":course,"date":date,"view":view,"language":language}))
}

pub(crate) fn choice<'a>(
    args: &'a Value,
    key: &str,
    default: &'a str,
    choices: &[&str],
) -> Result<&'a str, ()> {
    match args.get(key) {
        Some(value) => value
            .as_str()
            .filter(|value| choices.contains(value))
            .ok_or(()),
        None => Ok(default),
    }
}

pub(crate) fn optional_uuid<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, ()> {
    args.get(key)
        .map(|value| {
            value
                .as_str()
                .filter(|value| ledger::valid_uuid(value))
                .ok_or(())
        })
        .transpose()
}

pub(crate) fn integer(
    args: &Value,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ()> {
    match args.get(key) {
        Some(number) => number
            .as_f64()
            .filter(|number| number.fract() == 0.0 && (min as f64..=max as f64).contains(number))
            .map(|number| number as u64)
            .ok_or(()),
        None => Ok(default),
    }
}

pub(crate) fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], &'static str> {
    value[key]
        .as_array()
        .map(Vec::as_slice)
        .ok_or("Invalid response")
}

pub(crate) fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn normalize_name(name: &str) -> String {
    let decomposed: String = name
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c == '\u{0671}' { '\u{0627}' } else { c })
        .collect();
    decomposed.split_whitespace().collect::<Vec<_>>().join(" ")
}

// Fuse's default Bitap scoring, with ignoreLocation=true and the single name key.
// Match on UTF-16 units to preserve the JavaScript matcher and its 32-bit chunks.
fn name_score(name: &str, term: &[u16], chunks: &[&[u16]]) -> Option<f64> {
    let name = normalize_name(name);
    let text: Vec<_> = name.encode_utf16().collect();
    let threshold = if term.len() < 3 { 0.0 } else { 0.34 };
    let score = if text == term {
        f64::EPSILON
    } else {
        let scores: Vec<_> = chunks
            .iter()
            .map(|chunk| bitap(&text, chunk, threshold))
            .collect();
        if scores.iter().all(Option::is_none) {
            return None;
        }
        scores.iter().map(|score| score.unwrap_or(1.0)).sum::<f64>() / scores.len() as f64
    };
    let norm = (1000.0 / (name.split_whitespace().count() as f64).sqrt()).round() / 1000.0;
    Some(score.powf(norm))
}

fn bitap(text: &[u16], pattern: &[u16], threshold: f64) -> Option<f64> {
    if text.windows(pattern.len()).any(|word| word == pattern) {
        return Some(0.001);
    }
    let mut masks = HashMap::<u16, u32>::new();
    for (index, &c) in pattern.iter().enumerate() {
        *masks.entry(c).or_default() |= 1 << (pattern.len() - index - 1);
    }
    let high_bit = 1 << (pattern.len() - 1);
    let end = text.len() + pattern.len();
    let mut previous = vec![0_u32; end + 2];
    let mut current = previous.clone();
    for errors in 0..=(pattern.len() as f64 * threshold) as u32 {
        current[end + 1] = (1 << errors) - 1;
        for index in (1..=end).rev() {
            let mask = text
                .get(index - 1)
                .and_then(|c| masks.get(c))
                .copied()
                .unwrap_or(0);
            current[index] = ((current[index + 1] << 1) | 1) & mask;
            if errors > 0 {
                current[index] |=
                    ((previous[index + 1] | previous[index]) << 1) | 1 | previous[index + 1];
            }
            if current[index] & high_bit != 0 {
                return Some((f64::from(errors) / pattern.len() as f64).max(0.001));
            }
        }
        std::mem::swap(&mut previous, &mut current);
    }
    None
}

pub(crate) fn find_students(snapshot: &Value, filter: &Value) -> Result<Value, &'static str> {
    let course = filter["courseId"].as_str();
    if let Some(course) = course
        && !array(snapshot, "courses")?
            .iter()
            .any(|item| item["id"] == course)
    {
        return Err("Course not found");
    }
    let membership: HashMap<_, _> = array(snapshot, "enrollments")?
        .iter()
        .filter(|item| {
            course.is_some_and(|course| item["courseId"] == course) && item["active"] == true
        })
        .map(|item| (string(item, "studentId"), string(item, "level")))
        .collect();
    let eligible = array(snapshot, "students")?.iter().filter(|student| {
        let id = string(student, "studentId");
        let active = student["active"] == true;
        let status = string(filter, "status");
        (status == "all" || active == (status == "active"))
            && (course.is_none() || (active && membership.contains_key(id)))
    });
    let matches = rank_students(eligible, string(filter, "query"));
    let offset = filter["offset"].as_u64().ok_or("Invalid response")? as usize;
    let limit = filter["limit"].as_u64().ok_or("Invalid response")? as usize;
    let total = matches.len();
    let students: Vec<_> = matches
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|student| {
            let id = string(student, "studentId");
            let level = membership
                .get(id)
                .copied()
                .unwrap_or_else(|| string(student, "group"));
            json!({"studentId":id,"name":student["name"],"active":student["active"],"level":level})
        })
        .collect();
    Ok(json!({"total":total,"offset":offset,"students":students}))
}

pub(crate) fn rank_students<'a>(
    students: impl Iterator<Item = &'a Value>,
    query: &str,
) -> Vec<&'a Value> {
    let term: Vec<_> = normalize_name(query).encode_utf16().collect();
    if term.is_empty() {
        return students.collect();
    }
    let (full, remainder) = term.split_at(term.len() / 32 * 32);
    let mut chunks: Vec<_> = full.chunks(32).collect();
    if !remainder.is_empty() {
        chunks.push(&term[term.len().saturating_sub(32)..]);
    }
    let mut matches: Vec<_> = students
        .filter_map(|student| {
            name_score(string(student, "name"), &term, &chunks).map(|score| (score, student))
        })
        .collect();
    matches.sort_by(|left, right| left.0.total_cmp(&right.0));
    matches.into_iter().map(|(_, student)| student).collect()
}

struct Mark<'a> {
    event: &'a Value,
    value: Option<Value>,
}

type MarkKey<'a> = (&'a str, &'a str, &'a str);

pub(crate) struct Projection<'a> {
    marks: Vec<Mark<'a>>,
    versions: HashMap<MarkKey<'a>, usize>,
    notes: Vec<&'a Value>,
}

impl<'a> Projection<'a> {
    pub(crate) fn new(events: &'a [Value]) -> Self {
        let mut seen = HashMap::<&str, &str>::new();
        let mut reversed = HashSet::new();
        let mut ordered = Vec::new();
        for event in events {
            let id = string(event, "id");
            if seen.contains_key(id) {
                continue;
            }
            if event["payload"]["kind"] == "reversal"
                && let Some(kind) = seen.get(string(&event["payload"], "targetId"))
                && *kind != "reversal"
            {
                reversed.insert(string(&event["payload"], "targetId"));
            }
            seen.insert(id, string(&event["payload"], "kind"));
            ordered.push(event);
        }
        let mut marks = Vec::new();
        let mut versions = HashMap::new();
        let mut notes = Vec::new();
        for event in ordered
            .into_iter()
            .filter(|event| !reversed.contains(string(event, "id")))
        {
            let payload = &event["payload"];
            match string(payload, "kind") {
                "attendance" => {
                    let key = (
                        string(payload, "studentId"),
                        string(payload, "enrollmentId"),
                        string(payload, "date"),
                    );
                    let value = payload["status"].as_str().map(|status| {
                        let mut value = payload.clone();
                        if matches!(status, "early" | "late") {
                            value["status"] = json!("present");
                        }
                        value
                    });
                    let marked = Mark { event, value };
                    let position = *versions.entry(key).or_insert(marks.len());
                    if position == marks.len() {
                        marks.push(marked);
                    } else {
                        marks[position] = marked;
                    }
                }
                "note" => notes.push(event),
                _ => {}
            }
        }
        Self {
            marks,
            versions,
            notes,
        }
    }

    pub(crate) fn attendance(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.marks
            .iter()
            .filter_map(|mark| mark.value.as_ref().map(|value| (mark.event, value)))
    }

    fn version(&self, key: MarkKey<'_>) -> Option<&Mark<'a>> {
        self.versions
            .get(&key)
            .and_then(|position| self.marks.get(*position))
    }
}

pub(crate) fn attendance_summary(snapshot: &Value) -> Result<Value, &'static str> {
    let projection = Projection::new(array(snapshot, "events")?);
    let attendance: Vec<_> = projection
        .attendance()
        .map(|(event, value)| json!({"event":event,"value":value}))
        .collect();
    let notes: Vec<_> = projection
        .notes
        .iter()
        .map(|event| json!({"event":event,"value":event["payload"]}))
        .collect();
    Ok(
        json!({"students":snapshot["students"],"courses":snapshot["courses"],"enrollments":snapshot["enrollments"],"attendance":attendance,"notes":notes}),
    )
}

pub(crate) fn student_details(snapshot: &Value, filter: &Value) -> Result<Value, &'static str> {
    let id = string(filter, "studentId");
    let selected_course = filter["courseId"].as_str();
    let courses = array(snapshot, "courses")?;
    let mut student = array(snapshot, "students")?
        .iter()
        .find(|student| student["studentId"] == id)
        .cloned()
        .ok_or("Student or course not found")?;
    let enrollments: Vec<_> = array(snapshot, "enrollments")?
        .iter()
        .filter(|item| item["studentId"] == id)
        .collect();
    let selected = if let Some(course_id) = selected_course {
        if !courses.iter().any(|course| course["id"] == course_id) {
            return Err("Student or course not found");
        }
        Some(
            enrollments
                .iter()
                .copied()
                .find(|item| item["courseId"] == course_id)
                .ok_or("Student or course not found")?,
        )
    } else {
        None
    };
    let group = student
        .as_object_mut()
        .ok_or("Invalid response")?
        .remove("group")
        .unwrap_or_else(|| json!(""));
    let level = selected.map_or(group, |item| item["level"].clone());
    let courses: Vec<_> = enrollments.into_iter().map(|item| {
        let course_id = string(item,"courseId");
        let name = courses.iter().find(|course| course["id"] == course_id).map_or("", |course| string(course, "name"));
        let mut course = json!({"enrollmentId":item["id"],"courseId":course_id,"name":name,"active":item["active"]});
        if selected_course.is_none_or(|id| id == course_id) { course["level"] = item["level"].clone(); }
        course
    }).collect();
    let projection = Projection::new(array(snapshot, "events")?);
    let with_id = |event: &Value, payload: &Value| {
        let mut payload = payload.clone();
        payload["eventId"] = event["id"].clone();
        payload
    };
    let attendance: Vec<_> = projection
        .attendance()
        .filter(|(_, value)| {
            value["studentId"] == id
                && selected.is_none_or(|item| value["enrollmentId"] == item["id"])
        })
        .map(|(event, value)| with_id(event, value))
        .collect();
    let notes: Vec<_> = projection
        .notes
        .iter()
        .filter(|event| event["payload"]["studentId"] == id)
        .map(|event| with_id(event, &event["payload"]))
        .collect();
    Ok(
        json!({"student":student,"level":level,"courses":courses,"attendance":attendance,"notes":notes}),
    )
}

#[derive(Default)]
struct Counts {
    present: usize,
    absent: usize,
    excused: usize,
    notes: usize,
}

impl Counts {
    fn record(&mut self, status: &str) {
        match status {
            "present" => self.present += 1,
            "absent" => self.absent += 1,
            "excused" => self.excused += 1,
            _ => {}
        }
    }

    fn marked(&self) -> usize {
        self.present + self.absent + self.excused
    }

    fn rate(&self) -> Option<f64> {
        let marked = self.marked();
        (marked > 0).then(|| self.present as f64 / marked as f64)
    }
}

fn compare_names(left: &Value, right: &Value) -> std::cmp::Ordering {
    let a = string(left, "name");
    let b = string(right, "name");
    normalize_name(a)
        .cmp(&normalize_name(b))
        .then_with(|| a.cmp(b))
}

pub(crate) fn attendance_view(
    snapshot: &Value,
    filter: &Value,
    origin: &str,
) -> Result<Value, &'static str> {
    let course_id = string(filter, "courseId");
    let date = string(filter, "date");
    let view = string(filter, "view");
    let ar = filter["language"] == "ar";
    let course = array(snapshot, "courses")?
        .iter()
        .find(|course| course["id"] == course_id);
    if course_id != "none" && course.is_none() {
        return Err("Course not found");
    }
    let mut summary_url = url::Url::parse(origin).map_err(|_| "Invalid origin")?;
    summary_url.query_pairs_mut().extend_pairs([
        ("page", "app"),
        ("tab", "summary"),
        ("course", course_id),
    ]);
    let enrollments: HashMap<_, _> = array(snapshot, "enrollments")?
        .iter()
        .filter(|item| item["courseId"] == course_id)
        .map(|item| (string(item, "studentId"), item))
        .collect();
    let ids: HashSet<_> = enrollments
        .values()
        .map(|item| string(item, "id"))
        .collect();
    let mut students: Vec<_> = array(snapshot, "students")?
        .iter()
        .filter(|item| {
            if course_id == "none" {
                item["active"] == true
            } else {
                enrollments.contains_key(string(item, "studentId"))
            }
        })
        .collect();
    students.sort_by(|a, b| {
        compare_names(a, b).then_with(|| string(a, "studentId").cmp(string(b, "studentId")))
    });
    let projection = Projection::new(array(snapshot, "events")?);
    let mut records = HashMap::new();
    let mut dates = BTreeMap::<_, Counts>::new();
    let mut totals: HashMap<_, Counts> = students
        .iter()
        .map(|student| (string(student, "studentId"), Counts::default()))
        .collect();
    for (_, value) in projection.attendance().filter(|(_, value)| {
        let enrollment_id = string(value, "enrollmentId");
        if course_id == "none" {
            enrollment_id.is_empty()
        } else {
            ids.contains(enrollment_id)
        }
    }) {
        let id = string(value, "studentId");
        let day = string(value, "date");
        let status = string(value, "status");
        records.insert((id, day), status);
        if let Some(count) = totals.get_mut(id) {
            count.record(status);
            dates.entry(day).or_default().record(status);
        }
    }
    if view == "chart" {
        let mut sorted: Vec<_> = students
            .iter()
            .map(|item| &totals[string(item, "studentId")])
            .collect();
        sorted.sort_by(|a, b| {
            b.rate()
                .unwrap_or(-1.0)
                .total_cmp(&a.rate().unwrap_or(-1.0))
        });
        let chart: Vec<_> = sorted
            .iter()
            .enumerate()
            .map(|(index, count)| {
                let label = format!(
                    "{} {}",
                    if ar { "طالب" } else { "Student" },
                    format_count(index + 1, ar)
                );
                let rate = count.rate();
                let evidence = rate.map_or_else(
                    || if ar { "لا سجلات" } else { "No records" }.to_owned(),
                    |rate| {
                        format!(
                            "{} ({}/{})",
                            format_percent(rate, ar),
                            format_count(count.present, ar),
                            format_count(count.marked(), ar)
                        )
                    },
                );
                json!({"label":label,"rate":rate,"evidence":evidence})
            })
            .collect();
        return Ok(json!({"summaryUrl":summary_url.as_str(),"chart":chart}));
    }
    for event in &projection.notes {
        if let Some(count) = totals.get_mut(string(&event["payload"], "studentId")) {
            count.notes += 1;
        }
    }
    let active: Vec<_> = students
        .iter()
        .copied()
        .filter(|item| {
            item["active"] == true
                && enrollments
                    .get(string(item, "studentId"))
                    .is_none_or(|item| item["active"] == true)
        })
        .collect();
    let session: Vec<_> = active.iter().map(|student| {
        let id = string(student, "studentId");
        let enrollment = enrollments.get(id).copied().unwrap_or(&Value::Null);
        let record = projection.version((id, string(enrollment, "id"), date));
        let status = record.and_then(|record| record.value.as_ref()).map_or("unmarked", |value| string(value, "status"));
        json!({"studentId":id,"name":student["name"],"enrollmentId":enrollment["id"],"level":enrollment.get("level").unwrap_or(&student["group"]),"teacherId":enrollment["teacherId"],"teacherName":enrollment["teacherName"],"status":status,"eventId":record.map(|record| &record.event["id"])})
    }).collect();
    let marked = session
        .iter()
        .filter(|row| row["status"] != "unmarked")
        .count();
    let mut result = json!({
        "course":course.map(|course| json!({"id":course["id"],"name":course["name"],"open":course["open"]})),
        "date":date,"sessionCounts":{"marked":marked,"unmarked":session.len()-marked},"session":session
    });
    if view == "session" {
        return Ok(result);
    }
    let recent_dates: Vec<_> = {
        let ordered: std::collections::BTreeSet<_> = records
            .keys()
            .map(|(_, day)| *day)
            .filter(|day| *day <= date)
            .collect();
        let mut recent: Vec<_> = ordered.into_iter().rev().take(6).collect();
        recent.reverse();
        recent
    };
    let mut follow_up: Vec<_> = active.iter().filter_map(|student| {
        let id = string(student, "studentId");
        let marks: Vec<_> = recent_dates.iter().map(|day| records.get(&(id, day)).copied().unwrap_or("unmarked")).collect();
        let absences = marks.iter().filter(|mark| **mark == "absent").count();
        (absences > 0).then(|| (absences, json!({"studentId":id,"name":student["name"],"marks":marks,"absences":absences})))
    }).collect();
    follow_up.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| compare_names(&a.1, &b.1)));
    let mut breakdown: Vec<_> = dates.into_iter().collect();
    breakdown.sort_by(|a, b| b.1.absent.cmp(&a.1.absent).then_with(|| b.0.cmp(a.0)));
    result["summaryUrl"] = json!(summary_url.as_str());
    result["recentDates"] = json!(recent_dates);
    result["followUp"] = json!(
        follow_up
            .into_iter()
            .map(|(_, row)| row)
            .collect::<Vec<_>>()
    );
    result["breakdown"] = json!(breakdown.into_iter().map(|(date, count)| json!({"date":date,"present":count.present,"absent":count.absent,"excused":count.excused})).collect::<Vec<_>>());
    result["totals"] = json!(students.into_iter().map(|student| {
        let id = string(student, "studentId");
        let enrollment = enrollments.get(id).copied().unwrap_or(&Value::Null);
        let count = &totals[id];
        json!({"studentId":id,"name":student["name"],"enrollmentId":enrollment["id"],"active":student["active"],"membershipActive":enrollment["active"],"present":count.present,"absent":count.absent,"excused":count.excused,"notes":count.notes,"rate":count.rate()})
    }).collect::<Vec<_>>());
    Ok(result)
}

fn format_count(count: usize, ar: bool) -> String {
    let digits = count.to_string();
    let mut text = String::new();
    for (index, digit) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(if ar { '٬' } else { ',' });
        }
        text.push(if ar {
            char::from_u32('٠' as u32 + u32::from(digit - b'0')).unwrap()
        } else {
            char::from(digit)
        });
    }
    text
}

fn format_percent(rate: f64, ar: bool) -> String {
    let tenths = (rate * 1000.0).round() as usize;
    let mut text = format_count(tenths / 10, ar);
    if tenths % 10 != 0 {
        text.push(if ar { '٫' } else { '.' });
        text.push_str(&format_count(tenths % 10, ar));
    }
    text.push_str(if ar { "٪\u{061c}" } else { "%" });
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn course_views_keep_blank_level_and_cleared_revision() {
        let snapshot = json!({"courses":[{"id":"course","name":"Course","open":true}],"students":[{"studentId":"student","name":"إبراهيم","active":true,"group":"Roster"}],"enrollments":[{"id":"entry","courseId":"course","studentId":"student","level":"","active":true}],"events":[
            {"id":"original","payload":{"kind":"attendance","date":"2026-11-27","studentId":"student","enrollmentId":"entry","status":"late"}},
            {"id":"cleared","payload":{"kind":"attendance","date":"2026-11-27","studentId":"student","enrollmentId":"entry","status":null}}
        ]});
        let filter =
            json!({"courseId":"course","date":"2026-11-27","view":"overview","language":"en"});
        let result = attendance_view(&snapshot, &filter, "https://wartaq.example").unwrap();
        assert_eq!(result["session"][0]["status"], "unmarked");
        assert_eq!(result["session"][0]["eventId"], "cleared");
        assert_eq!(result["session"][0]["level"], "");
        assert_eq!(result["totals"][0]["present"], 0);
        let query =
            json!({"courseId":"course","query":"ابرا","status":"all","offset":0,"limit":20});
        let students = find_students(&snapshot, &query).unwrap();
        assert_eq!(students["total"], 1);
        assert_eq!(students["students"][0]["level"], "");
    }
}
