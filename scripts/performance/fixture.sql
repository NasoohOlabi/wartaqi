\set ON_ERROR_STOP on
BEGIN;

CREATE FUNCTION pg_temp.fixture_id(label text) RETURNS uuid
LANGUAGE sql IMMUTABLE STRICT AS $$
  SELECT (substr(md5(label), 1, 12) || '4' || substr(md5(label), 14, 3) ||
          '8' || substr(md5(label), 18, 15))::uuid
$$;

INSERT INTO teachers (id, auth_user_id, name, access_mode)
SELECT pg_temp.fixture_id('teacher-' || n), pg_temp.fixture_id('staff-login-' || n),
       CASE WHEN n = 0 THEN 'Fixture administrator' ELSE 'Fixture teacher ' || n END, 'read_write'
FROM generate_series(0, 12) AS s(n);

INSERT INTO students (id, name, first_name, last_name, teacher_id, group_name, active)
SELECT pg_temp.fixture_id('student-' || n),
       CASE WHEN n % 4 = 0 THEN 'طالب اختباري ' ELSE 'Fixture learner ' END || lpad(n::text, 4, '0'),
       CASE WHEN n % 4 = 0 THEN 'طالب اختباري' ELSE 'Fixture learner' END,
       lpad(n::text, 4, '0'), pg_temp.fixture_id('teacher-' || (1 + (n - 1) % 12)),
       'Level ' || (1 + (n - 1) % 6), n % 29 <> 0
FROM generate_series(1, :students) AS s(n);

INSERT INTO courses (id, name, open)
SELECT pg_temp.fixture_id('course-' || n), 'Fixture course ' || n, n <> 8
FROM generate_series(1, 8) AS s(n);

CREATE TEMP TABLE fixture_memberships ON COMMIT DROP AS
SELECT n, membership, pg_temp.fixture_id('student-' || n) AS student,
       pg_temp.fixture_id('teacher-' || (1 + (n - 1) % 12)) AS teacher,
       pg_temp.fixture_id('course-' || (1 + (n - 1 + membership * 3) % 8)) AS course,
       pg_temp.fixture_id('enrollment-' || n || '-' || membership) AS enrollment
FROM generate_series(1, :students) AS students(n)
CROSS JOIN generate_series(0, 1) AS memberships(membership);

INSERT INTO course_enrollments (id, student_id, course_id, teacher_id, level, practical, theory, active)
SELECT enrollment, student, course, teacher, (1 + (n - 1) % 6)::text,
       CASE WHEN n % 3 = 0 THEN n % 101 END,
       CASE WHEN n % 5 = 0 THEN (n * 7) % 101 END, membership = 0 OR n % 13 <> 0
FROM fixture_memberships ORDER BY n, membership;

WITH accounts AS (
  SELECT n, pg_temp.fixture_id('staff-login-' || n) AS id,
         CASE WHEN n = 0 THEN 'administrator' ELSE 'teacher' END AS role,
         pg_temp.fixture_id('teacher-' || n) AS teacher, NULL::uuid AS student,
         '+12025551' || lpad(n::text, 3, '0') AS phone
  FROM generate_series(0, 12) AS s(n)
  UNION ALL
  SELECT n + 100, pg_temp.fixture_id('student-login-' || n), 'student',
         NULL, pg_temp.fixture_id('student-' || n), '+12025552' || lpad(n::text, 3, '0')
  FROM generate_series(1, least(:students, 24)) AS s(n)
), identities AS (
  INSERT INTO neon_auth."user" (id, name, email, "emailVerified")
  SELECT id, 'Fixture ' || role || ' ' || n, 'fixture-' || n || '@example.test', true FROM accounts
), passwords AS (
  INSERT INTO neon_auth.account (id, "accountId", "providerId", "userId", password)
  SELECT pg_temp.fixture_id('credential-' || n), id, 'credential', id, :'password_hash' FROM accounts
)
INSERT INTO app_accounts (auth_user_id, role, teacher_id, student_id, verified_phone)
SELECT id, role, teacher, student, phone FROM accounts;

INSERT INTO events (id, teacher_id, student_id, actor, client_time, received_at, payload, enrollment_id, edit_time)
SELECT pg_temp.fixture_id('mark-' || m.n || '-' || m.membership || '-' || d.day),
       m.teacher, m.student, 'Fixture teacher ' || (1 + (m.n - 1) % 12), stamped, stamped,
       jsonb_build_object('kind', 'attendance', 'studentId', m.student, 'enrollmentId', m.enrollment,
         'date', recorded, 'status', CASE WHEN (m.n * 7 + d.day) % 10 < 7 THEN 'present'
           WHEN (m.n * 7 + d.day) % 10 < 9 THEN 'absent' ELSE 'excused' END),
       m.enrollment, stamped
FROM fixture_memberships AS m CROSS JOIN generate_series(1, :days) AS d(day)
CROSS JOIN LATERAL (SELECT DATE '2026-11-01' - d.day AS recorded) AS date_range
CROSS JOIN LATERAL (SELECT recorded::timestamptz + INTERVAL '14 hours' AS stamped) AS mark_time
ORDER BY d.day DESC, m.n, m.membership;

INSERT INTO events (id, teacher_id, student_id, actor, client_time, received_at, payload)
SELECT pg_temp.fixture_id('note-' || n || '-' || d.day), teacher, student,
       'Fixture teacher ' || (1 + (n - 1) % 12), recorded::timestamptz, recorded::timestamptz,
       jsonb_build_object('kind', 'note', 'studentId', student, 'date', recorded, 'tags',
         jsonb_build_array('follow-up', 'practice'), 'text',
         'Synthetic follow-up for learner ' || n || ' on ' || recorded ||
         '. Review the previous lesson and ask about the practice assignment. ' ||
         CASE WHEN (n + d.day) % 2 = 0 THEN 'متابعة الدرس والمراجعة' ELSE 'Check the assigned reading.' END)
FROM fixture_memberships CROSS JOIN generate_series(7, :days, 7) AS d(day)
CROSS JOIN LATERAL (SELECT DATE '2026-11-01' - d.day AS recorded) AS date_range
WHERE membership = 0 AND (n + d.day) % 3 = 0
ORDER BY d.day DESC, n;

INSERT INTO events (id, teacher_id, student_id, actor, client_time, received_at, payload)
SELECT pg_temp.fixture_id('reversal-' || id::text), teacher_id, student_id, actor,
       client_time + INTERVAL '1 hour', received_at + INTERVAL '1 hour',
       jsonb_build_object('kind', 'reversal', 'targetId', id, 'reason', 'Synthetic correction')
FROM events WHERE payload->>'kind' = 'attendance' AND sequence % 41 = 0
ORDER BY sequence;

COMMIT;
VACUUM (ANALYZE);
