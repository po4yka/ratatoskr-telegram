-- Run as a PostgreSQL administrator after Edge has created the `ratatoskr` database and before the
-- first start of the webhook or the dispatcher, which share this role. Set the password separately
-- with `\password ratatoskr_telegram`; no credential belongs in this repository.

do $$
begin
    if not exists (select 1 from pg_roles where rolname = 'ratatoskr_telegram') then
        create role ratatoskr_telegram login;
    end if;
end
$$;

alter role ratatoskr_telegram nosuperuser nocreatedb nocreaterole noreplication nobypassrls;
grant connect, create on database ratatoskr to ratatoskr_telegram;

\connect ratatoskr
revoke all on schema public from ratatoskr_telegram;
