use std::time::Duration;

use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use deadpool_postgres::Pool;
use serde_json::json;

pub(crate) async fn health(pool: &Pool) -> Response {
    async fn check(pool: &Pool) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let db = pool.get().await?;
        db.query("SELECT course_enrollments.teacher_id, course_enrollments.active, course_enrollments.handbook_received, course_enrollments.notes, course_enrollments.grade_decisions, app_accounts.invitation_request_id, app_accounts.invitation_username, app_accounts.verified_phone, app_accounts.invitation_phone, app_accounts.invitation_birth_date, app_accounts.invitation_requires_setup, students.whatsapp, students.location, students.birth_year, courses.open, courses.attendance_minimum, courses.practical_minimum, courses.theory_minimum, telegram_conversation_turns.turn FROM app_accounts, teachers, students, courses, course_enrollments, events, telegram_conversation_turns, neon_auth.\"user\", neon_auth.account, neon_auth.session, neon_auth.verification LIMIT 0", &[]).await?;
        db.query("SELECT 1 FROM neon_auth.\"oauthClient\", neon_auth.\"oauthClientResource\", neon_auth.\"oauthResource\", neon_auth.\"oauthAccessToken\", neon_auth.\"oauthRefreshToken\", neon_auth.\"oauthConsent\", neon_auth.\"oauthClientAssertion\" LIMIT 0", &[]).await?;
        let assignment = db.query_opt("SELECT is_nullable FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'students' AND column_name = 'teacher_id'", &[]).await?;
        if assignment.is_none_or(|row| row.get::<_, String>(0) != "YES") {
            return Ok(false);
        }
        if db.query_opt("SELECT 1 FROM pg_index i JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0] WHERE i.indexrelid = to_regclass('public.app_accounts_verified_phone_idx') AND i.indrelid = 'public.app_accounts'::regclass AND a.attname = 'verified_phone' AND i.indisunique AND i.indisvalid AND i.indisready AND i.indnkeyatts = 1 AND i.indpred IS NULL", &[]).await?.is_none() { return Ok(false); }
        let sync: i32 = db.query_one("SELECT count(*)::int FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid WHERE c.relnamespace = 'public'::regnamespace AND t.tgname = 'notify_workspace_changed' AND t.tgenabled IN ('O','A') AND c.relname IN ('app_accounts','teachers','students','courses','course_enrollments','events') AND to_regclass('public.grade_sync_receipts') IS NOT NULL", &[]).await?.get(0);
        let versions: i32 = db.query_one("SELECT count(*)::int FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid WHERE c.relnamespace = 'public'::regnamespace AND t.tgname = 'wartaq_edit_version' AND t.tgenabled IN ('O','A') AND c.relname IN ('students','courses','course_enrollments')", &[]).await?.get(0);
        db.query(
            "SELECT edit_time, wartaq_event_payload(payload, payload_packed, student_id, enrollment_id) FROM public.events LIMIT 0",
            &[],
        )
            .await?;
        Ok(sync == 6 && versions == 3)
    }
    let ok = matches!(check(pool).await, Ok(true));
    (
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"ok": ok})),
    )
        .into_response()
}

pub(crate) async fn maintain(pool: Pool) {
    let mut timer = tokio::time::interval(Duration::from_secs(3600));
    loop {
        timer.tick().await;
        let result = async {
            let db = pool.get().await?;
            db.execute(
                "DELETE FROM telegram_conversation_turns WHERE received_at < now() - interval '3 days'",
                &[],
            )
            .await?;
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        }
        .await;
        if result.is_err() {
            eprintln!("Telegram conversation cleanup failed");
        }
    }
}
