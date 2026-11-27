-- Apply after the web deployment that removes these tables from readiness checks.
DROP TABLE IF EXISTS public.website_session_replay, public.website_diagnostics;
