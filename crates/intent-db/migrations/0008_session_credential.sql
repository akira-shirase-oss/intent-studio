-- A session proves the credential it was created under (F1): rotating
-- INTENT_OPERATOR_PASSWORD_HASH ends every session issued before the rotation,
-- because a session's check requires the row to carry the fingerprint of the
-- credential the server is configured with NOW. Sessions created before this
-- column existed carry NULL — they can prove nothing, and the next check drops
-- them: an upgrade is a rotation for every old row, which is the safe answer.
ALTER TABLE operator_sessions
    ADD COLUMN credential_fp text
        CHECK (credential_fp IS NULL OR char_length(credential_fp) = 32);
