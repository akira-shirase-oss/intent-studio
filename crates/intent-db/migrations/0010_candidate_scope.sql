-- I2-7: tenant/brief and page-tree binding.
--
-- Three tightenings on the I2 tables, all at the schema so a wrong write is a
-- refusal, not a quiet mismatch:
--
-- 1. A candidate names a brief *of its own tenant*: separate foreign keys let
--    (tenant A, brief of B) pair; the composite to briefs(tenant_id, id) —
--    the same pattern 0006 established for facts/brief_claims — binds them.
-- 2. A check record is tenant-scoped: its tenant is the candidate's, bound by
--    a composite foreign key so it can never drift to another tenant's row.
-- 3. The page tree's own hash (D-010's list): the tree is stored as provenance
--    jsonb; its canonical hash lets an approval bind the tree as well as the
--    bytes — filled for every new row.

ALTER TABLE candidates
    DROP CONSTRAINT candidates_tenant_id_fkey,
    DROP CONSTRAINT candidates_brief_id_fkey,
    ADD COLUMN page_tree_sha256 bytea CHECK (octet_length(page_tree_sha256) = 32),
    ADD CONSTRAINT candidates_tenant_brief_fkey
        FOREIGN KEY (tenant_id, brief_id) REFERENCES briefs (tenant_id, id),
    -- The pair check_records' tenant scope targets.
    ADD CONSTRAINT candidates_id_tenant_key UNIQUE (id, tenant_id);

ALTER TABLE check_records
    ADD COLUMN tenant_id uuid;

-- Existing rows inherit their candidate's tenant — the only tenant a record
-- could ever truthfully name.
UPDATE check_records cr
    SET tenant_id = c.tenant_id
    FROM candidates c
    WHERE c.id = cr.candidate_id;

ALTER TABLE check_records
    ALTER COLUMN tenant_id SET NOT NULL,
    ADD CONSTRAINT check_records_candidate_tenant_fkey
        FOREIGN KEY (candidate_id, tenant_id) REFERENCES candidates (id, tenant_id);

CREATE INDEX check_records_by_tenant ON check_records (tenant_id, ran_at DESC, id DESC);
