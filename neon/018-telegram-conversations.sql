CREATE TABLE IF NOT EXISTS telegram_conversation_turns (
  sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  trace text NOT NULL UNIQUE,
  conversation_id text NOT NULL,
  turn jsonb NOT NULL,
  received_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS telegram_conversation_turns_conversation_sequence_idx
  ON telegram_conversation_turns (conversation_id, sequence DESC);
CREATE INDEX IF NOT EXISTS telegram_conversation_turns_received_at_idx
  ON telegram_conversation_turns (received_at);
