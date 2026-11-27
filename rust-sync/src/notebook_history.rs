use crate::access::Grant;
use crate::ledger::{InvalidData, decode_event, valid_uuid};
use serde_json::Value;
use tokio_postgres::Transaction;

type Error = Box<dyn std::error::Error + Send + Sync>;
const PAGE_SIZE: usize = 100;

pub(crate) struct HistoryPage {
    pub(crate) events: Vec<Value>,
    pub(crate) reversed_ids: Vec<String>,
    pub(crate) next_before: Option<String>,
}

pub(crate) async fn history(
    tx: &Transaction<'_>,
    grant: &Grant,
    course_id: Option<&str>,
    student_id: Option<&str>,
    before: Option<i64>,
) -> Result<HistoryPage, Error> {
    if grant.is_student()
        || !grant.can_read()
        || course_id.is_some_and(|id| !valid_uuid(id))
        || student_id.is_some_and(|id| !valid_uuid(id))
        || before.is_some_and(|value| value <= 0)
    {
        return Err(InvalidData.into());
    }
    let scope = if grant.is_admin() {
        "true"
    } else {
        "s.active AND s.teacher_id = $1::text::uuid"
    };
    // Scope ordinary records once, then attach reversals to their earlier target.
    let sql = format!(
        r#"
        WITH eligible AS MATERIALIZED (
          SELECT e.id::text AS id, e.sequence FROM events e
          JOIN students s ON s.id::text = CASE WHEN e.payload_packed THEN e.student_id::text ELSE e.payload->>'studentId' END
          LEFT JOIN course_enrollments ce ON ce.id=e.enrollment_id
          WHERE $1::text IS NOT NULL AND {scope} AND ($2::text IS NULL OR s.id=$2::text::uuid)
            AND (e.payload->>'kind'='note' OR (e.payload->>'kind'='attendance' AND
                (($4::text IS NULL AND e.enrollment_id IS NULL) OR ce.course_id=$4::text::uuid)))
        ), reversals AS MATERIALIZED (
          SELECT e.id::text AS id, e.sequence, e.payload->>'targetId' AS target_id
          FROM events e WHERE e.payload->>'kind'='reversal'
        ), page AS MATERIALIZED (
          SELECT id, sequence FROM (
            SELECT id, sequence FROM eligible
            UNION ALL
            SELECT r.id, r.sequence FROM reversals r JOIN eligible e ON e.id=r.target_id AND e.sequence<r.sequence
          ) visible WHERE $3::bigint IS NULL OR sequence<$3 ORDER BY sequence DESC LIMIT 101
        )
        SELECT e.sequence,e.id::text AS id,e.actor::text AS actor,
          to_char(e.client_time AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS client_time,
          to_char(e.received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS received_at,
          wartaq_event_payload(e.payload,e.payload_packed,e.student_id,e.enrollment_id) AS payload,
          EXISTS (SELECT 1 FROM reversals r WHERE r.target_id=p.id AND r.sequence>p.sequence)
            AND e.payload->>'kind' IN ('attendance','note') AS reversed
        FROM page p JOIN events e ON e.id=p.id::uuid ORDER BY e.sequence DESC"#
    );
    let teacher = grant.id();
    let rows = tx
        .query(&sql, &[&teacher, &student_id, &before, &course_id])
        .await?;
    let has_more = rows.len() > PAGE_SIZE;
    let mut events = Vec::new();
    let mut reversed_ids = Vec::new();
    let mut oldest = None;
    for row in rows.into_iter().take(PAGE_SIZE) {
        let sequence: i64 = row.try_get("sequence")?;
        let id: String = row.try_get("id")?;
        if row.try_get::<_, bool>("reversed")? {
            reversed_ids.push(id.clone());
        }
        oldest = Some(sequence.to_string());
        events.push(decode_event(&row)?);
    }
    events.reverse();
    Ok(HistoryPage {
        events,
        reversed_ids,
        next_before: if has_more { oldest } else { None },
    })
}
