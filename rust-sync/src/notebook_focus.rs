use crate::access::Grant;
use crate::ledger::{InvalidData, date as valid_date, decode_event, valid_uuid};
use futures_util::TryStreamExt;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use tokio_postgres::Transaction;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// A partial projection for the selected course (or legacy unscoped attendance).
/// Never persist this as a complete notebook or use it for history and totals.
pub(crate) struct FocusEvents {
    pub(crate) events: Vec<Value>,
    /// Enrollment IDs with at least one effective non-null mark, including future dates.
    pub(crate) recorded_enrollment_ids: Vec<String>,
}

pub(crate) async fn events(
    tx: &Transaction<'_>,
    grant: &Grant,
    date: &str,
    course_id: Option<&str>,
) -> Result<FocusEvents, Error> {
    if grant.is_student()
        || !grant.can_read()
        || !valid_date(date)
        || course_id.is_some_and(|id| !valid_uuid(id))
    {
        return Err(InvalidData.into());
    }
    let scope = if grant.is_admin() {
        "true"
    } else {
        "s.active AND s.teacher_id = $1::text::uuid"
    };
    let sql = format!(
        r#"SELECT e.sequence, e.id::text AS id, e.actor::text AS actor,
        to_char(e.client_time AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS client_time,
        to_char(e.received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS received_at,
        wartaq_event_payload(e.payload, e.payload_packed, e.student_id, e.enrollment_id) AS payload
        FROM events e JOIN students s ON s.id=e.student_id
        LEFT JOIN course_enrollments ce ON ce.id=e.enrollment_id
        WHERE $1::text IS NOT NULL AND {scope} AND (e.payload->>'kind' = 'reversal' OR
          (e.payload->>'kind' = 'attendance' AND
            (($2::text IS NULL AND e.enrollment_id IS NULL) OR ce.course_id=$2::text::uuid)))
        ORDER BY e.sequence DESC"#
    );
    let teacher_id = grant.id();
    let rows = tx
        .query_raw(
            &sql,
            [
                &teacher_id as &(dyn tokio_postgres::types::ToSql + Sync),
                &course_id,
            ],
        )
        .await?;
    futures_util::pin_mut!(rows);
    let mut fold = Fold::new(date);
    while let Some(row) = rows.try_next().await? {
        let sequence: i64 = row.try_get("sequence")?;
        fold.push(sequence, decode_event(&row)?);
    }
    Ok(fold.finish())
}

struct Fold<'a> {
    date: &'a str,
    reversed: HashSet<String>,
    seen: HashSet<(String, String, String)>,
    recent: BTreeMap<String, Vec<(i64, Value)>>,
    selected_clears: Vec<(i64, Value)>,
    recorded: HashSet<String>,
}

impl<'a> Fold<'a> {
    fn new(date: &'a str) -> Self {
        Self {
            date,
            reversed: HashSet::new(),
            seen: HashSet::new(),
            recent: BTreeMap::new(),
            selected_clears: Vec::new(),
            recorded: HashSet::new(),
        }
    }

    // Input must be in descending database sequence order. A reversal only affects
    // an earlier event, so it cannot accidentally cancel a forward reference.
    fn push(&mut self, sequence: i64, event: Value) {
        let payload = &event["payload"];
        if payload["kind"] == "reversal" {
            self.reversed
                .insert(payload["targetId"].as_str().unwrap().to_owned());
            return;
        }
        if payload["kind"] != "attendance" || self.reversed.contains(event["id"].as_str().unwrap())
        {
            return;
        }
        let student = payload["studentId"].as_str().unwrap();
        let enrollment = payload
            .get("enrollmentId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let day = payload["date"].as_str().unwrap();
        if !self
            .seen
            .insert((student.to_owned(), enrollment.to_owned(), day.to_owned()))
        {
            return;
        }
        if !payload["status"].is_null() && !enrollment.is_empty() {
            self.recorded.insert(enrollment.to_owned());
        }
        if day == self.date && payload["status"].is_null() {
            self.selected_clears.push((sequence, event));
        } else if day <= self.date && !payload["status"].is_null() {
            self.recent
                .entry(day.to_owned())
                .or_default()
                .push((sequence, event));
            if self.recent.len() > 6 {
                self.recent.pop_first();
            }
        }
    }

    fn finish(self) -> FocusEvents {
        let mut events = self.selected_clears;
        events.extend(self.recent.into_values().flatten());
        events.sort_unstable_by_key(|(sequence, _)| *sequence);
        let mut recorded_enrollment_ids: Vec<_> = self.recorded.into_iter().collect();
        recorded_enrollment_ids.sort_unstable();
        FocusEvents {
            events: events.into_iter().map(|(_, event)| event).collect(),
            recorded_enrollment_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reverse_scan_keeps_revealed_marks_clears_and_all_date_readiness() {
        let mark = |id: &str, date: &str, status: Value| json!({"id":id,"payload":{"kind":"attendance","studentId":"student","enrollmentId":"enrollment","date":date,"status":status}});
        let reversal = |id: &str, target: &str| json!({"id":id,"payload":{"kind":"reversal","targetId":target}});
        let mut fold = Fold::new("2026-01-08");
        let input = vec![
            mark("future", "2026-02-01", json!("present")),
            reversal("undo", "clear"),
            mark("clear", "2026-01-08", Value::Null),
            mark("revealed", "2026-01-08", json!("late")),
            mark("7", "2026-01-07", json!("absent")),
            mark("6", "2026-01-06", json!("present")),
            mark("5", "2026-01-05", json!("present")),
            mark("4", "2026-01-04", json!("present")),
            mark("3", "2026-01-03", json!("present")),
            mark("2", "2026-01-02", json!("present")),
        ];
        for (i, event) in input.into_iter().enumerate() {
            fold.push(100 - i as i64, event);
        }
        let result = fold.finish();
        let ids: Vec<_> = result
            .events
            .iter()
            .map(|event| event["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["3", "4", "5", "6", "7", "revealed"]);
        assert_eq!(result.recorded_enrollment_ids, ["enrollment"]);
    }
}
