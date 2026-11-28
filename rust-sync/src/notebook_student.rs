use crate::access::Grant;
use crate::ledger::{InvalidData, VISIBLE_LEDGER_EVENT, decode_event, valid_uuid};
use futures_util::TryStreamExt;
use serde_json::Value;
use tokio_postgres::Transaction;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Complete visible ledger for one student; roster and assignments come from the snapshot.
pub(crate) async fn events(
    tx: &Transaction<'_>,
    grant: &Grant,
    student_id: &str,
) -> Result<Vec<Value>, Error> {
    if grant.is_student() || !grant.can_read() || !valid_uuid(student_id) {
        return Err(InvalidData.into());
    }
    let scope = if grant.is_admin() {
        "true"
    } else {
        "s.active AND s.teacher_id = $2::text::uuid"
    };
    let sql = format!(
        r#"SELECT e.id::text AS id, e.actor::text AS actor,
        to_char(e.client_time AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS client_time,
        to_char(e.received_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS received_at,
        wartaq_event_payload(e.payload, e.payload_packed, e.student_id, e.enrollment_id) AS payload
        FROM events e JOIN students s ON s.id=e.student_id
        WHERE e.student_id=$1::text::uuid AND $2::text IS NOT NULL AND {scope}
          AND {VISIBLE_LEDGER_EVENT}
        ORDER BY e.sequence"#
    );
    let statement = tx.prepare(&sql).await?;
    let teacher_id = grant.id();
    let rows = tx.query_raw(&statement, [student_id, teacher_id]).await?;
    futures_util::pin_mut!(rows);
    let mut events = Vec::new();
    while let Some(row) = rows.try_next().await? {
        events.push(decode_event(&row)?);
    }
    Ok(events)
}
