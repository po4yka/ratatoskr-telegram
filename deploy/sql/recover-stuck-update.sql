-- Telegram-owned conditional recovery only. psql variables are provided by telegram-ops.
with recovered as (
    update telegram.updates
       set state = 'accepted', settled_at = null
     where bot_id = :'bot_id'::bigint
       and update_id = :'update_id'::bigint
       and state = :'expected_state'
       and payload is not null
     returning bot_id, update_id, state
)
select case count(*) when 1 then 'recovered_one' else 'refused_state_mismatch' end
from recovered;
