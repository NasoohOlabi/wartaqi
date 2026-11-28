-- Add a reader for either layout before switching the web deployment.

CREATE OR REPLACE FUNCTION wartaq_event_payload(p jsonb, packed boolean, student uuid, enrollment uuid)
RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
  SELECT CASE WHEN packed THEN p || jsonb_build_object('studentId', student::text, 'enrollmentId', enrollment::text) ELSE p END
$$;

ALTER TABLE events ADD COLUMN IF NOT EXISTS payload_packed boolean NOT NULL DEFAULT false;
