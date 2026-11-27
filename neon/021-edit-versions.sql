ALTER TABLE students ADD COLUMN IF NOT EXISTS edit_time timestamptz;
ALTER TABLE students ADD COLUMN IF NOT EXISTS edit_id uuid;
ALTER TABLE students ADD COLUMN IF NOT EXISTS edit_xid bigint;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS edit_time timestamptz;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS edit_id uuid;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS edit_xid bigint;
ALTER TABLE course_enrollments ADD COLUMN IF NOT EXISTS edit_time timestamptz;
ALTER TABLE course_enrollments ADD COLUMN IF NOT EXISTS edit_id uuid;
ALTER TABLE course_enrollments ADD COLUMN IF NOT EXISTS edit_xid bigint;

CREATE OR REPLACE FUNCTION wartaq_check_edit_version() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  incoming_time timestamptz := COALESCE(NULLIF(current_setting('wartaq.edit_time', true), '')::timestamptz, clock_timestamp());
  incoming_id uuid := COALESCE(NULLIF(current_setting('wartaq.edit_id', true), '')::uuid, gen_random_uuid());
BEGIN
  IF incoming_time > clock_timestamp() + interval '5 minutes' THEN
    RAISE EXCEPTION 'edit clock is ahead' USING ERRCODE = 'W0001';
  END IF;
  IF TG_OP = 'UPDATE' AND OLD.edit_time IS NOT NULL AND
    (incoming_time, incoming_id) <= (OLD.edit_time, OLD.edit_id) AND
    NOT (incoming_time = OLD.edit_time AND incoming_id = OLD.edit_id AND OLD.edit_xid = txid_current()) THEN
    RAISE EXCEPTION 'stale edit' USING ERRCODE = 'W0001';
  END IF;
  NEW.edit_time := incoming_time;
  NEW.edit_id := incoming_id;
  NEW.edit_xid := txid_current();
  RETURN NEW;
END $$;

DROP TRIGGER IF EXISTS wartaq_edit_version ON students;
CREATE TRIGGER wartaq_edit_version BEFORE INSERT OR UPDATE ON students FOR EACH ROW EXECUTE FUNCTION wartaq_check_edit_version();
DROP TRIGGER IF EXISTS wartaq_edit_version ON courses;
CREATE TRIGGER wartaq_edit_version BEFORE INSERT OR UPDATE ON courses FOR EACH ROW EXECUTE FUNCTION wartaq_check_edit_version();
DROP TRIGGER IF EXISTS wartaq_edit_version ON course_enrollments;
CREATE TRIGGER wartaq_edit_version BEFORE INSERT OR UPDATE ON course_enrollments FOR EACH ROW EXECUTE FUNCTION wartaq_check_edit_version();

ALTER TABLE events ADD COLUMN IF NOT EXISTS edit_time timestamptz;

CREATE INDEX IF NOT EXISTS events_attendance_version_idx ON events (student_id, enrollment_id, received_at);
