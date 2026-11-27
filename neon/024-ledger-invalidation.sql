CREATE OR REPLACE FUNCTION public.notify_workspace_changed()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  PERFORM pg_notify('wartaq_workspace_changed', CASE WHEN TG_TABLE_NAME = 'events' THEN 'ledger' ELSE '' END);
  RETURN NULL;
END;
$$;
