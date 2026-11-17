-- I1: uploaded assets with their licence and provenance (D-011, D-016, D-062) and reference
-- snapshots fetched through the egress guard (D-012, D-060). The bytes live in the
-- content-addressed store under their SHA-256; these rows say whose they are, where they came
-- from and what may be done with them.

CREATE TABLE assets (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants (id),
    -- The original, exactly as uploaded (kept, with its licence: invariant 6).
    original_sha256 bytea NOT NULL CHECK (octet_length(original_sha256) = 32),
    original_bytes bigint NOT NULL CHECK (original_bytes > 0),
    original_media_type text NOT NULL CHECK (char_length(original_media_type) BETWEEN 1 AND 100),
    original_name text CHECK (char_length(original_name) BETWEEN 1 AND 200),
    -- What the worker produced: a sanitized SVG, a re-encoded image, or the checked font.
    sha256 bytea NOT NULL CHECK (octet_length(sha256) = 32),
    bytes bigint NOT NULL CHECK (bytes > 0),
    media_type text NOT NULL CHECK (media_type IN ('image/svg+xml', 'image/png', 'image/jpeg', 'font/ttf', 'font/otf')),
    width integer CHECK (width > 0),
    height integer CHECK (height > 0),
    -- Elements and attributes the SVG sanitizer dropped (0 for everything else).
    removed_parts integer NOT NULL DEFAULT 0 CHECK (removed_parts >= 0),
    licence text NOT NULL CHECK (char_length(licence) BETWEEN 1 AND 200),
    source text NOT NULL CHECK (char_length(source) BETWEEN 1 AND 500),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, id)
);

CREATE INDEX assets_by_tenant ON assets (tenant_id, created_at DESC, id DESC);

-- One row per fetch attempt of a brief's reference, refused attempts included: the refusal is
-- part of the record of what was and was not taken from where.
CREATE TABLE reference_snapshots (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL,
    brief_id uuid NOT NULL,
    url text NOT NULL CHECK (char_length(url) BETWEEN 1 AND 2048),
    outcome text NOT NULL CHECK (outcome IN ('stored', 'refused')),
    refusal text CHECK (char_length(refusal) BETWEEN 1 AND 64),
    final_url text CHECK (char_length(final_url) BETWEEN 1 AND 2048),
    sha256 bytea CHECK (octet_length(sha256) = 32),
    bytes bigint CHECK (bytes >= 0),
    media_type text CHECK (char_length(media_type) BETWEEN 1 AND 100),
    fetched_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT snapshots_outcome_shape CHECK (
        (outcome = 'stored') = (sha256 IS NOT NULL AND bytes IS NOT NULL AND refusal IS NULL)),
    FOREIGN KEY (tenant_id, brief_id) REFERENCES briefs (tenant_id, id)
);

CREATE INDEX snapshots_by_brief ON reference_snapshots (brief_id, fetched_at DESC);
