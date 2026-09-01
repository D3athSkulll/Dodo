-- Run once as a Postgres superuser to create the role + database this service
-- expects. Matches the DATABASE_URL in .env.example.
--
--   psql -U postgres -f scripts/db-setup.sql
--
-- (docker compose does this for you; only needed for a bare local run.)

create role dodo login password 'dodo';
create database dodo owner dodo;
