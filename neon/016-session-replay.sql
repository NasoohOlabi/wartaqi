CREATE TABLE IF NOT EXISTS website_session_replay (
  actor_id text NOT NULL REFERENCES app_accounts(auth_user_id) ON DELETE CASCADE,
  journey_id uuid NOT NULL,
  step integer NOT NULL CHECK (step BETWEEN 1 AND 600),
  frame jsonb NOT NULL,
  received_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (actor_id, journey_id, step)
);
CREATE INDEX IF NOT EXISTS website_session_replay_received_idx ON website_session_replay (received_at);
REVOKE ALL ON TABLE website_session_replay FROM PUBLIC;
