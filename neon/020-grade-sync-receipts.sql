CREATE TABLE IF NOT EXISTS public.grade_sync_receipts (
  request_id uuid PRIMARY KEY,
  fingerprint text NOT NULL CHECK (fingerprint ~ '^[0-9a-f]{64}$')
);
REVOKE ALL ON TABLE public.grade_sync_receipts FROM PUBLIC;
