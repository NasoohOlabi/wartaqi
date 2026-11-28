-- The canonical payload remains the external and backup representation.
CREATE OR REPLACE FUNCTION wartaq_attendance_packable(p jsonb, student uuid, enrollment uuid)
RETURNS boolean LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE day date;
BEGIN
  IF jsonb_typeof(p) <> 'object' OR enrollment IS NULL OR
    p->>'kind' IS DISTINCT FROM 'attendance' OR
    p->>'studentId' IS DISTINCT FROM student::text OR
    p->>'enrollmentId' IS DISTINCT FROM enrollment::text OR
    student::text !~ '^(00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff|[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$' OR
    enrollment::text !~ '^(00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff|[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$' OR
    (p - ARRAY['kind', 'studentId', 'enrollmentId', 'date', 'status']) <> '{}'::jsonb OR
    jsonb_typeof(p->'date') IS DISTINCT FROM 'string' OR
    p->>'date' !~ '^\d{4}-\d{2}-\d{2}$' OR
    NOT (p ? 'status') OR
    NOT (p->'status' = 'null'::jsonb OR p->'status' IN ('"present"'::jsonb, '"absent"'::jsonb, '"excused"'::jsonb, '"early"'::jsonb, '"late"'::jsonb)) THEN
    RETURN false;
  END IF;
  BEGIN
    day := (p->>'date')::date;
    RETURN to_char(day, 'YYYY-MM-DD') = p->>'date';
  EXCEPTION WHEN datetime_field_overflow OR invalid_datetime_format THEN
    RETURN false;
  END;
END $$;

DO $$
DECLARE first_install boolean := NOT EXISTS (
  SELECT 1 FROM pg_trigger WHERE tgrelid = 'events'::regclass AND tgname = 'wartaq_pack_event'
);
BEGIN
  ALTER TABLE events DROP CONSTRAINT IF EXISTS events_attendance_enrollment_check;
  ALTER TABLE events ADD CONSTRAINT events_attendance_enrollment_check CHECK (
    payload_packed OR payload->>'kind' <> 'attendance' OR (payload->>'enrollmentId') IS NOT DISTINCT FROM enrollment_id::text
  );
  IF first_install THEN
    UPDATE events SET payload = payload - ARRAY['studentId', 'enrollmentId'], payload_packed = true
      WHERE wartaq_attendance_packable(payload, student_id, enrollment_id);
    -- Reclaim these obsolete tuple versions after committing this migration.
  END IF;
END $$;

ALTER TABLE events DROP CONSTRAINT IF EXISTS events_packed_payload_check;
ALTER TABLE events ADD CONSTRAINT events_packed_payload_check CHECK (
  NOT payload_packed OR (
    NOT (payload ?| ARRAY['studentId', 'enrollmentId']) AND
    wartaq_attendance_packable(wartaq_event_payload(payload, true, student_id, enrollment_id), student_id, enrollment_id)
  )
);

CREATE OR REPLACE FUNCTION wartaq_pack_event() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.payload_packed THEN
    RAISE EXCEPTION 'packed event input is not allowed';
  END IF;
  IF wartaq_attendance_packable(NEW.payload, NEW.student_id, NEW.enrollment_id) THEN
    NEW.payload := NEW.payload - ARRAY['studentId', 'enrollmentId'];
    NEW.payload_packed := true;
  END IF;
  RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS wartaq_pack_event ON events;
CREATE TRIGGER wartaq_pack_event BEFORE INSERT ON events FOR EACH ROW EXECUTE FUNCTION wartaq_pack_event();

DROP INDEX IF EXISTS events_teacher_sequence_idx;
DROP INDEX IF EXISTS events_attendance_version_idx;
CREATE INDEX IF NOT EXISTS events_student_idx ON events(student_id);
