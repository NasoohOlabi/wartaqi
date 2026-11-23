CREATE TABLE IF NOT EXISTS website_diagnostics (
  sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  actor_id text NOT NULL REFERENCES app_accounts(auth_user_id) ON DELETE CASCADE,
  event_id uuid NOT NULL,
  journey_id uuid NOT NULL,
  role text NOT NULL CHECK (role IN ('administrator', 'teacher', 'student')),
  event jsonb NOT NULL,
  received_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (actor_id, event_id)
);
CREATE INDEX IF NOT EXISTS website_diagnostics_received_idx ON website_diagnostics (received_at);
CREATE INDEX IF NOT EXISTS website_diagnostics_journey_idx ON website_diagnostics (actor_id, journey_id, sequence DESC);
REVOKE ALL ON TABLE website_diagnostics FROM PUBLIC;
