\pset tuples_only on
\pset format unaligned
SELECT jsonb_build_object(
  'databaseBytes', pg_database_size(current_database()),
  'coreRelationBytes', (
    SELECT sum(pg_total_relation_size(oid)) FROM pg_class
    WHERE relnamespace IN ('public'::regnamespace, 'neon_auth'::regnamespace) AND relkind = 'r'
  ),
  'events', (SELECT count(*) FROM events),
  'eventKinds', (
    SELECT jsonb_object_agg(kind, rows) FROM (
      SELECT payload->>'kind' AS kind, count(*) AS rows FROM events GROUP BY payload->>'kind'
    ) AS kinds
  ),
  'eventsHeapBytes', pg_relation_size('events'),
  'eventsIndexBytes', pg_indexes_size('events'),
  'eventsTotalBytes', pg_total_relation_size('events'),
  'eventsIndexes', (
    SELECT jsonb_object_agg(indexrelid::regclass::text, pg_relation_size(indexrelid))
    FROM pg_index WHERE indrelid = 'events'::regclass
  )
);
