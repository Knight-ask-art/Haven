ALTER TABLE download_tasks ADD COLUMN speed_bps INTEGER CHECK (speed_bps IS NULL OR speed_bps >= 0);
ALTER TABLE download_tasks ADD COLUMN eta_seconds INTEGER CHECK (eta_seconds IS NULL OR eta_seconds >= 0);
