-- The reconciliation sweeper re-submits a charge with the same idempotency key.
-- The mock PSP dedupes on that key, but only if the first call actually arrived;
-- to re-submit safely in every case the sweeper also needs the card token, so we
-- store it. `request_fingerprint` stays as the case-(d) "same key, different
-- body" check.
alter table payment_attempts
    add column card_token text not null default '';
