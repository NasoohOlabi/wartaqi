use crate::access::Grant;
use crate::ledger::{InvalidData, decode_event, valid_uuid};
use futures_util::TryStreamExt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use tokio_postgres::Transaction;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Exact surviving marks and note counts, without event identities or note contents.
/// Each marks row is [student index, enrollment index or null, marks].
/// Each mark is date index * 3 + status (present=0, absent=1, excused=2).
/// Notes rows are [student index, count]. Dictionaries and rows are sorted.
pub(crate) async fn summary(
    tx: &Transaction<'_>,
    grant: &Grant,
    course_id: Option<&str>,
    all_courses: bool,
) -> Result<Value, Error> {
    if grant.is_student() || !grant.can_read() || course_id.is_some_and(|id| !valid_uuid(id)) {
        return Err(InvalidData.into());
    }
    let scope = if grant.is_admin() {
        "true"
    } else {
        "s.active AND s.teacher_id = $1::text::uuid"
    };
    let sql = format!(
        r#"SELECT e.id::text AS id, e.actor::text AS actor,
        to_char(e.client_time AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS client_time,
        to_char(e.received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS received_at,
        wartaq_event_payload(e.payload, e.payload_packed, e.student_id, e.enrollment_id) AS payload
        FROM events e JOIN students s ON s.id=e.student_id
        LEFT JOIN course_enrollments ce ON ce.id=e.enrollment_id
        WHERE $1::text IS NOT NULL AND {scope} AND
          (e.payload->>'kind' IN ('reversal', 'note') OR
           (e.payload->>'kind' = 'attendance' AND
            ($3 OR ($2::text IS NULL AND e.enrollment_id IS NULL) OR ce.course_id=$2::text::uuid)))
        ORDER BY e.sequence DESC"#
    );
    let teacher_id = grant.id();
    let rows = tx
        .query_raw(
            &sql,
            [
                &teacher_id as &(dyn tokio_postgres::types::ToSql + Sync),
                &course_id,
                &all_courses,
            ],
        )
        .await?;
    futures_util::pin_mut!(rows);
    let mut fold = Fold::default();
    while let Some(row) = rows.try_next().await? {
        fold.push(decode_event(&row)?);
    }
    Ok(fold.finish())
}

#[derive(Default)]
struct Fold {
    reversed: HashSet<String>,
    seen: HashSet<(String, Option<String>, String)>,
    marks: BTreeMap<(String, Option<String>), BTreeMap<String, u8>>,
    notes: BTreeMap<String, u64>,
}

impl Fold {
    // Reverse database sequence: only a later reversal can cancel a target.
    fn push(&mut self, event: Value) {
        let payload = &event["payload"];
        if payload["kind"] == "reversal" {
            self.reversed
                .insert(payload["targetId"].as_str().unwrap().to_owned());
            return;
        }
        if self.reversed.contains(event["id"].as_str().unwrap()) {
            return;
        }
        match payload["kind"].as_str() {
            Some("note") => {
                let student = payload["studentId"].as_str().unwrap();
                if !student.is_empty() {
                    *self.notes.entry(student.to_owned()).or_default() += 1;
                }
            }
            Some("attendance") => {
                let student = payload["studentId"].as_str().unwrap().to_owned();
                let enrollment = payload
                    .get("enrollmentId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let date = payload["date"].as_str().unwrap().to_owned();
                if !self
                    .seen
                    .insert((student.clone(), enrollment.clone(), date.clone()))
                {
                    return;
                }
                let status = match payload["status"].as_str() {
                    Some("present" | "early" | "late") => 0,
                    Some("absent") => 1,
                    Some("excused") => 2,
                    _ => return,
                };
                self.marks
                    .entry((student, enrollment))
                    .or_default()
                    .insert(date, status);
            }
            _ => {}
        }
    }

    fn finish(self) -> Value {
        let dates: Vec<_> = self
            .marks
            .values()
            .flat_map(|marks| marks.keys().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let students: Vec<_> = self
            .marks
            .keys()
            .map(|(student, _)| student.clone())
            .chain(self.notes.keys().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let enrollments: Vec<_> = self
            .marks
            .keys()
            .filter_map(|(_, enrollment)| enrollment.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let date_indices: HashMap<_, _> = dates
            .iter()
            .enumerate()
            .map(|(i, value)| (value, i))
            .collect();
        let student_indices: HashMap<_, _> = students
            .iter()
            .enumerate()
            .map(|(i, value)| (value, i))
            .collect();
        let enrollment_indices: HashMap<_, _> = enrollments
            .iter()
            .enumerate()
            .map(|(i, value)| (value, i))
            .collect();
        let marks: Vec<_> = self
            .marks
            .into_iter()
            .map(|((student, enrollment), days)| {
                let marks: Vec<_> = days
                    .into_iter()
                    .map(|(date, status)| date_indices[&date] * 3 + usize::from(status))
                    .collect();
                json!([
                    student_indices[&student],
                    enrollment.as_ref().map(|id| enrollment_indices[id]),
                    marks
                ])
            })
            .collect();
        let notes: Vec<_> = self
            .notes
            .into_iter()
            .map(|(student, count)| json!([student_indices[&student], count]))
            .collect();
        json!({"dates": dates, "students": students, "enrollments": enrollments, "marks": marks, "notes": notes})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folds_reversals_clears_notes_and_legacy_statuses() {
        let mut fold = Fold::default();
        for event in [
            json!({"id":"r1","payload":{"kind":"reversal","targetId":"clear"}}),
            json!({"id":"r2","payload":{"kind":"reversal","targetId":"note2"}}),
            json!({"id":"clear","payload":{"kind":"attendance","studentId":"s","enrollmentId":"e","date":"2026-01-01","status":null}}),
            json!({"id":"late","payload":{"kind":"attendance","studentId":"s","enrollmentId":"e","date":"2026-01-01","status":"late"}}),
            json!({"id":"clear2","payload":{"kind":"attendance","studentId":"s","date":"2026-01-02","status":null}}),
            json!({"id":"old","payload":{"kind":"attendance","studentId":"s","date":"2026-01-02","status":"absent"}}),
            json!({"id":"note2","payload":{"kind":"note","studentId":"s"}}),
            json!({"id":"note1","payload":{"kind":"note","studentId":"s"}}),
        ] {
            fold.push(event);
        }
        assert_eq!(
            fold.finish(),
            json!({"dates":["2026-01-01"],"students":["s"],"enrollments":["e"],"marks":[[0,0,[0]]],"notes":[[0,1]]})
        );
    }
}
