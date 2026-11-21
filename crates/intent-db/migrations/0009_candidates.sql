-- I2: candidates (a built bundle with its provenance) and check records (the
-- D-022 suite's evidence). The invariant both tables serve: an approval may one
-- day bind an exact bundle hash, so a check record can only ever describe the
-- bundle its candidate names — enforced as a foreign key on the pair, not just
-- by the code that writes it.

CREATE TABLE candidates (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants (id),
    brief_id uuid NOT NULL REFERENCES briefs (id),
    -- The bundle's SHA-256 over its canonical framing; the store's key for the bytes.
    bundle_sha256 bytea NOT NULL CHECK (octet_length(bundle_sha256) = 32),
    -- The validated page tree the bundle was built from (provenance, never re-parsed for trust).
    tree jsonb NOT NULL,
    component_set text NOT NULL CHECK (char_length(component_set) BETWEEN 1 AND 100),
    -- What built it: `baseline@1` for the deterministic builder; model/version rows join later.
    builder text NOT NULL CHECK (char_length(builder) BETWEEN 1 AND 100),
    -- The model name when a model built it; NULL for the deterministic baseline.
    model text CHECK (char_length(model) BETWEEN 1 AND 100),
    created_at timestamptz NOT NULL DEFAULT now(),
    -- The same brief producing the same bytes is one candidate, not two rows.
    UNIQUE (brief_id, bundle_sha256),
    -- The pair check_records' foreign key targets: a record may only ever attach
    -- to the hash its candidate itself names (D-010's binding at the schema).
    UNIQUE (id, bundle_sha256)
);

CREATE INDEX candidates_by_brief ON candidates (brief_id, created_at DESC, id DESC);

CREATE TABLE check_records (
    id uuid PRIMARY KEY,
    candidate_id uuid NOT NULL,
    bundle_sha256 bytea NOT NULL CHECK (octet_length(bundle_sha256) = 32),
    -- The suite and its version (`d-022@0`); a record is meaningless without it.
    suite text NOT NULL CHECK (char_length(suite) BETWEEN 1 AND 40),
    -- `static`, `chromium` or `webkit` — one row per backend cell that ran.
    engine text NOT NULL CHECK (engine IN ('static', 'chromium', 'webkit')),
    -- `static` for byte-level checks; else `360x800`, `390x844`, `1280x800`.
    viewport text NOT NULL CHECK (char_length(viewport) BETWEEN 1 AND 20),
    -- Per-check outcomes: check, status, detail — pending entries included, honestly.
    results jsonb NOT NULL,
    -- pass | fail | pending. `pending` is not a pass and can never promote (D-010).
    verdict text NOT NULL CHECK (verdict IN ('pass', 'fail', 'pending')),
    ran_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (candidate_id, bundle_sha256)
        REFERENCES candidates (id, bundle_sha256)
);

CREATE INDEX check_records_by_candidate ON check_records (candidate_id, ran_at DESC, id DESC);
