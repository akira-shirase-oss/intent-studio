-- I0, the lead-intake spine: tenants, sites, leads, the notification outbox, operator sessions
-- and the operator audit. PostgreSQL is the only authority (invariant 1).

CREATE TABLE tenants (
    id uuid PRIMARY KEY,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    -- D-019 / D-031: 365 days by default; the domain bounds it to 30..=3650.
    retention_days smallint NOT NULL DEFAULT 365 CHECK (retention_days BETWEEN 30 AND 3650),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sites (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants (id),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    -- Operator-configured recipient of lead notifications; never a submitted value.
    notify_to text NOT NULL CHECK (char_length(notify_to) BETWEEN 3 AND 254),
    accepting_leads boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, id)
);

CREATE TABLE leads (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL,
    site_id uuid NOT NULL,
    -- `c:<client id>` or `d:<derived>`: one logical lead per (site, submission key).
    submission_key text NOT NULL CHECK (char_length(submission_key) BETWEEN 3 AND 80),
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    name text NOT NULL,
    email text,
    phone text,
    message text NOT NULL,
    service text,
    area text,
    consent boolean NOT NULL CHECK (consent),
    -- Preview leads (D-027, I3) are stored with this flag and never notify the client.
    is_test boolean NOT NULL DEFAULT false,
    received_at timestamptz NOT NULL,
    retain_until timestamptz NOT NULL,
    FOREIGN KEY (tenant_id, site_id) REFERENCES sites (tenant_id, id),
    CONSTRAINT leads_one_per_submission UNIQUE (site_id, submission_key),
    CONSTRAINT leads_have_contact CHECK (email IS NOT NULL OR phone IS NOT NULL)
);

CREATE INDEX leads_inbox ON leads (received_at DESC, id DESC);
CREATE INDEX leads_site_inbox ON leads (site_id, received_at DESC, id DESC);
CREATE INDEX leads_retention ON leads (retain_until);

-- One row per notification; its id is the provider's idempotency key (D-020). Committed in the
-- same statement as its lead; attempted later under a lease, never inside a transaction.
CREATE TABLE notification_outbox (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL,
    lead_id uuid NOT NULL REFERENCES leads (id) ON DELETE CASCADE,
    state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'delivered', 'dead')),
    -- Counted when an attempt is claimed, so a crash mid-attempt still uses one up.
    attempts smallint NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 8),
    -- Attempts that may have reached the provider without us knowing (timeouts, lost leases).
    uncertain_attempts smallint NOT NULL DEFAULT 0 CHECK (uncertain_attempts BETWEEN 0 AND 8),
    next_attempt_at timestamptz NOT NULL,
    lease_owner uuid,
    lease_expires_at timestamptz,
    last_error text CHECK (last_error IN ('provider_unavailable', 'provider_timeout', 'provider_rejected', 'lease_expired')),
    created_at timestamptz NOT NULL,
    delivered_at timestamptz,
    dead_at timestamptz,
    CONSTRAINT outbox_delivered_has_time CHECK ((state = 'delivered') = (delivered_at IS NOT NULL)),
    CONSTRAINT outbox_dead_has_time CHECK ((state = 'dead') = (dead_at IS NOT NULL)),
    CONSTRAINT outbox_lease_pair CHECK ((lease_owner IS NULL) = (lease_expires_at IS NULL))
);

CREATE INDEX outbox_due ON notification_outbox (next_attempt_at) WHERE state = 'pending';
CREATE INDEX outbox_by_lead ON notification_outbox (lead_id);

-- The operator's password sessions (D-015): only a SHA-256 digest of the token is stored.
CREATE TABLE operator_sessions (
    token_digest bytea PRIMARY KEY CHECK (octet_length(token_digest) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);

CREATE INDEX operator_sessions_expiry ON operator_sessions (expires_at);

-- Every cross-tenant operator action (D-015). Details carry ids and counts, never lead content.
CREATE TABLE operator_audit (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at timestamptz NOT NULL DEFAULT now(),
    action text NOT NULL CHECK (char_length(action) BETWEEN 1 AND 64),
    detail jsonb NOT NULL DEFAULT '{}'::jsonb
);
