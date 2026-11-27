CREATE OR REPLACE FUNCTION public.notify_workspace_changed()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  PERFORM pg_notify('wartaq_workspace_changed', '');
  RETURN NULL;
END;
$$;

DO $$
DECLARE
  domain_table text;
BEGIN
  FOREACH domain_table IN ARRAY ARRAY['app_accounts', 'teachers', 'students', 'courses', 'course_enrollments', 'events']
  LOOP
    EXECUTE format(
      'CREATE OR REPLACE TRIGGER notify_workspace_changed AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON public.%I FOR EACH STATEMENT EXECUTE FUNCTION public.notify_workspace_changed()',
      domain_table
    );
  END LOOP;
END;
$$;
