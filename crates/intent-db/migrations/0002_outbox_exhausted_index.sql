-- The sweep for rows with no attempt left (F3) looks only at these; without the index it would
-- scan every pending row, and a provider outage makes those many.
CREATE INDEX outbox_exhausted ON notification_outbox (id) WHERE state = 'pending' AND attempts >= 8;
