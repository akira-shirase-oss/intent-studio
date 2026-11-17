-- F8 / D-044: the privacy-notice version each lead consented under. Leads stored before this
-- column existed are marked as such; every new lead must name its version.
ALTER TABLE leads ADD COLUMN consent_notice text NOT NULL DEFAULT 'unrecorded'
    CHECK (char_length(consent_notice) BETWEEN 1 AND 64);
ALTER TABLE leads ALTER COLUMN consent_notice DROP DEFAULT;
