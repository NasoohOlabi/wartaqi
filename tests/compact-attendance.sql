\set ON_ERROR_STOP on
\ir ../neon/001-notebook.sql
\ir ../neon/003-courses.sql
\ir ../neon/021-edit-versions.sql
INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ('00000000-0000-4000-8000-000000000001', 'test', 'Test', 'read_write');
INSERT INTO students (id, name, teacher_id) VALUES ('00000000-0000-4000-8000-000000000002', 'Test', '00000000-0000-4000-8000-000000000001');
INSERT INTO courses (id, name) VALUES ('00000000-0000-4000-8000-000000000003', 'Test');
INSERT INTO course_enrollments (id, course_id, student_id) VALUES ('00000000-0000-4000-8000-000000000004', '00000000-0000-4000-8000-000000000003', '00000000-0000-4000-8000-000000000002');
INSERT INTO events (id, teacher_id, student_id, actor, client_time, enrollment_id, payload)
SELECT ('10000000-0000-4000-8000-' || lpad(n::text, 12, '0'))::uuid, '00000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-000000000002', 'Test', '2026-01-01',
  CASE WHEN n = 3 OR n = 6 THEN NULL ELSE '00000000-0000-4000-8000-000000000004'::uuid END,
  CASE n
    WHEN 1 THEN '{"kind":"attendance","studentId":"00000000-0000-4000-8000-000000000002","enrollmentId":"00000000-0000-4000-8000-000000000004","date":"2026-01-01","status":"early"}'::jsonb
    WHEN 2 THEN '{"kind":"attendance","studentId":"00000000-0000-4000-8000-000000000002","enrollmentId":"00000000-0000-4000-8000-000000000004","date":"2026-01-02","status":null,"extra":true}'::jsonb
    WHEN 3 THEN '{"kind":"attendance","studentId":"00000000-0000-4000-8000-000000000002","date":"2026-01-03","status":"late"}'::jsonb
    WHEN 4 THEN '{"kind":"attendance","studentId":"00000000-0000-4000-8000-000000000002","enrollmentId":"00000000-0000-4000-8000-000000000004","date":"2026-02-30","status":"present"}'::jsonb
    WHEN 5 THEN '{"kind":"attendance","studentId":"00000000-0000-4000-8000-000000000002","enrollmentId":"00000000-0000-4000-8000-000000000004","date":"2026-01-05","status":"absent"}'::jsonb
    ELSE '{"kind":"reversal","targetId":"10000000-0000-4000-8000-000000000001","reason":"test"}'::jsonb
  END FROM generate_series(1, 6) AS n;
CREATE TEMP TABLE original AS SELECT id, sequence, payload FROM events;
BEGIN;
\ir ../neon/023-compact-attendance.sql
COMMIT;
INSERT INTO events (id, teacher_id, student_id, actor, client_time, enrollment_id, payload)
SELECT '10000000-0000-4000-8000-000000000008', teacher_id, student_id, actor, client_time, enrollment_id, payload
FROM events WHERE id = '10000000-0000-4000-8000-000000000001';
DO $$ BEGIN
  IF EXISTS (SELECT FROM events WHERE payload_packed) OR
    EXISTS (SELECT FROM events e JOIN original o USING (id) WHERE e.sequence <> o.sequence OR e.payload <> o.payload) OR
    EXISTS (SELECT FROM events WHERE wartaq_event_payload(payload, payload_packed, student_id, enrollment_id) <> payload) THEN
    RAISE EXCEPTION 'reader setup changed the legacy layout';
  END IF;
END $$;
BEGIN;
\ir ../neon/025-enable-attendance-packing.sql
COMMIT;
BEGIN;
\ir ../neon/001-notebook.sql
\ir ../neon/003-courses.sql
\ir ../neon/021-edit-versions.sql
\ir ../neon/023-compact-attendance.sql
\ir ../neon/025-enable-attendance-packing.sql
COMMIT;
INSERT INTO events (id, teacher_id, student_id, actor, client_time, enrollment_id, payload)
SELECT '10000000-0000-4000-8000-000000000007', teacher_id, student_id, actor, client_time, enrollment_id, original.payload
FROM events JOIN original USING (id) WHERE id = '10000000-0000-4000-8000-000000000001';
DO $$ BEGIN
  IF EXISTS (SELECT FROM events e JOIN original o USING (id) WHERE e.sequence <> o.sequence OR wartaq_event_payload(e.payload, e.payload_packed, e.student_id, e.enrollment_id) <> o.payload) THEN RAISE EXCEPTION 'roundtrip changed'; END IF;
  IF (SELECT count(*) FROM events WHERE payload_packed) <> 4 THEN RAISE EXCEPTION 'wrong packed count'; END IF;
  IF NOT EXISTS (SELECT FROM events e JOIN original o USING (id) WHERE e.id = '10000000-0000-4000-8000-000000000001' AND wartaq_event_payload(e.payload, e.payload_packed, e.student_id, e.enrollment_id) = o.payload) THEN RAISE EXCEPTION 'same retry differs'; END IF;
  IF EXISTS (SELECT FROM events e JOIN original o USING (id) WHERE e.id = '10000000-0000-4000-8000-000000000001' AND wartaq_event_payload(e.payload, e.payload_packed, e.student_id, e.enrollment_id) = jsonb_set(o.payload, '{status}', '"absent"')) THEN RAISE EXCEPTION 'changed retry matched'; END IF;
END $$;
