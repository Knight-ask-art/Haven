-- Legacy Windows downloads stored native separators. Index their equivalent
-- identity without rewriting resource rows or changing Unix filename semantics.
CREATE INDEX idx_resources_windows_local_path
ON resources (
    storage_location_id,
    replace(json_extract(locator_json, '$.local_path.path'), char(92), '/')
)
WHERE locator_kind = 'local_path';
