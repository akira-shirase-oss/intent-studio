-- I1: fact records (D-013, D-056) and briefs with revisions (D-057). Every row carries its
-- tenant, and composite foreign keys keep a brief's claims to its own tenant's facts.

CREATE TABLE facts (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants (id),
    kind text NOT NULL CHECK (kind IN ('testimonial', 'metric', 'price', 'guarantee', 'licence',
        'certification', 'insurance', 'service_area', 'opening_hours', 'contact_details',
        'legal_promise')),
    -- Content: written once (D-056); a corrected statement is a new fact.
    statement text NOT NULL CHECK (char_length(statement) BETWEEN 1 AND 1000),
    source text NOT NULL CHECK (char_length(source) BETWEEN 1 AND 500),
    link_kind text CHECK (link_kind IN ('tel', 'mailto', 'url')),
    link_value text CHECK (char_length(link_value) BETWEEN 1 AND 2048),
    created_at timestamptz NOT NULL DEFAULT now(),
    -- The current approval: who, the day, the last day it holds, when it was recorded.
    approved_by text CHECK (char_length(approved_by) BETWEEN 1 AND 200),
    approved_on date,
    review_by date,
    approval_recorded_at timestamptz,
    withdrawn_at timestamptz,
    CONSTRAINT facts_link_pair CHECK ((link_kind IS NULL) = (link_value IS NULL)),
    CONSTRAINT facts_link_only_on_contact CHECK (link_kind IS NULL OR kind = 'contact_details'),
    CONSTRAINT facts_approval_whole CHECK (
        (approved_by IS NULL) = (approved_on IS NULL)
        AND (approved_on IS NULL) = (review_by IS NULL)
        AND (review_by IS NULL) = (approval_recorded_at IS NULL)),
    CONSTRAINT facts_review_after_approval CHECK (review_by IS NULL OR review_by > approved_on),
    UNIQUE (tenant_id, id)
);

CREATE INDEX facts_by_tenant ON facts (tenant_id, created_at DESC, id DESC);

CREATE TABLE briefs (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants (id),
    latest_revision integer NOT NULL DEFAULT 1 CHECK (latest_revision >= 1),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, id)
);

CREATE INDEX briefs_by_tenant ON briefs (tenant_id, updated_at DESC, id DESC);

-- Every revision is kept; the content is the validated brief's own JSON (D-057).
CREATE TABLE brief_revisions (
    tenant_id uuid NOT NULL,
    brief_id uuid NOT NULL,
    revision integer NOT NULL CHECK (revision >= 1),
    business_name text NOT NULL CHECK (char_length(business_name) BETWEEN 1 AND 120),
    content jsonb NOT NULL CHECK (octet_length(content::text) <= 131072),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (brief_id, revision),
    FOREIGN KEY (tenant_id, brief_id) REFERENCES briefs (tenant_id, id)
);

-- A revision's approved claims. The (tenant_id, fact_id) key means a brief can only ever list
-- its own tenant's facts, whatever the application does.
CREATE TABLE brief_claims (
    tenant_id uuid NOT NULL,
    brief_id uuid NOT NULL,
    revision integer NOT NULL,
    fact_id uuid NOT NULL,
    PRIMARY KEY (brief_id, revision, fact_id),
    FOREIGN KEY (brief_id, revision) REFERENCES brief_revisions (brief_id, revision),
    FOREIGN KEY (tenant_id, brief_id) REFERENCES briefs (tenant_id, id),
    FOREIGN KEY (tenant_id, fact_id) REFERENCES facts (tenant_id, id)
);

CREATE INDEX brief_claims_by_fact ON brief_claims (fact_id);
