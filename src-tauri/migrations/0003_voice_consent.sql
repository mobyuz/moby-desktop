-- Version 3: the cloud voice consent (stage D4), kept with the other settings of this computer.
-- Voice is off until the person accepts. The row names the account that accepted and the
-- cloud_voice version it accepted, so another account on this computer starts with voice off and
-- a newer version has to be accepted again. NULL means no consent.
ALTER TABLE settings ADD COLUMN voice_consent_account TEXT;
ALTER TABLE settings ADD COLUMN voice_consent_version TEXT;
ALTER TABLE settings ADD COLUMN voice_consent_at INTEGER;
