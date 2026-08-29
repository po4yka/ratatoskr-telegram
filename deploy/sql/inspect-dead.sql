-- Content-free, read-only projections. No payload, text, title, username, chat id, credentials, or
-- provider diagnostics are selected. `kind` and `limit` are validated by telegram-ops.
select bot_id::text || '/' || update_id::text as identifier,
       settled_at as occurred_at,
       null::integer as attempts,
       state as safe_class,
       null::text as correlation_ref
  from telegram.updates
 where :'kind' in ('updates', 'all') and state = 'failed'
union all
select id::text, updated_at, attempts, coalesce(last_error_class, state), null::text
  from telegram.outbound_jobs
 where :'kind' in ('outbound', 'all') and state in ('failed_permanent', 'outcome_unknown')
union all
select notification_id::text, updated_at, null::integer, outcome, transport_event_id::text
  from telegram.notification_decisions
 where :'kind' in ('notifications', 'all') and outcome = 'failed_permanent'
order by occurred_at desc
limit :'limit'::integer;
