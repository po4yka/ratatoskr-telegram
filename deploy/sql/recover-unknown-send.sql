-- Explicit duplicate-risk recovery. The original remains quarantined as audit evidence; a new
-- deliberate send attempt is inserted only while the exact expected state still holds.
begin;

select id as locked_job_id
  from telegram.outbound_jobs
 where id = :'job_id'::uuid
   and state = :'expected_state'
   and kind = 'send_message'
 for update
\gset

\if :{?locked_job_id}
with entropy as (
    select lpad(to_hex(floor(extract(epoch from clock_timestamp()) * 1000)::bigint), 12, '0')
               || '7' || substr(md5(random()::text || clock_timestamp()::text), 1, 3)
               || '8' || substr(md5(clock_timestamp()::text || random()::text), 1, 15) as raw
), recovery_id as (
    select (substr(raw, 1, 8) || '-' || substr(raw, 9, 4) || '-' || substr(raw, 13, 4)
            || '-' || substr(raw, 17, 4) || '-' || substr(raw, 21, 12))::uuid as id
      from entropy
), source as materialized (
    select original.* from telegram.outbound_jobs original
     where original.id = :'job_id'::uuid
), detached_notification as (
    update telegram.outbound_jobs original
       set delivery_class = 'direct', notification_id = null, notification_created_at = null
      from source
     where original.id = source.id and source.delivery_class = 'notification'
    returning original.id
), recovery_source as (
    select source.* from source where source.delivery_class <> 'notification'
    union all
    select source.* from source join detached_notification using (id)
), inserted as (
insert into telegram.outbound_jobs
       (id, bot_id, chat_id, kind, payload, content_hash, operation_id, revision, correlation_id,
        delivery_class, notification_id, notification_created_at, state, attempts,
        next_attempt_at, recovery_of)
select recovery_id.id, source.bot_id, source.chat_id, source.kind, source.payload,
       source.content_hash, source.operation_id, source.revision, source.correlation_id,
       source.delivery_class, source.notification_id, source.notification_created_at,
       'ready', 0, now(), source.id
  from recovery_source source cross join recovery_id
returning id, recovery_of, notification_id
), retargeted as (
    update telegram.notification_decisions decision
       set outbound_job_id = inserted.id, outcome = 'enqueued', updated_at = now()
      from inserted
     where inserted.notification_id is not null
       and decision.outbound_job_id = inserted.recovery_of
    returning decision.outbound_job_id
)
select inserted.id as recovery_job_id, inserted.recovery_of
  from inserted left join retargeted on retargeted.outbound_job_id = inserted.id;
\else
\echo 'unknown send recovery refused: job/state/kind no longer matches' >&2
\quit 3
\endif

commit;
