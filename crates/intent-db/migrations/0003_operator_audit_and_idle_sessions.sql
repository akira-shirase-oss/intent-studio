-- F6: who, from where and in which request every operator action happened, failed sign-ins
-- included; sessions end after an idle period as well as at their absolute expiry.
ALTER TABLE operator_audit
    ADD COLUMN actor text NOT NULL DEFAULT 'operator' CHECK (char_length(actor) BETWEEN 1 AND 64),
    ADD COLUMN address text CHECK (address IS NULL OR char_length(address) <= 64),
    ADD COLUMN request_id text CHECK (request_id IS NULL OR char_length(request_id) <= 64);

ALTER TABLE operator_sessions
    ADD COLUMN last_seen_at timestamptz NOT NULL DEFAULT now();
