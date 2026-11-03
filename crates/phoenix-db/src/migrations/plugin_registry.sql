CREATE TABLE plugins (
    name TEXT PRIMARY KEY NOT NULL,
    provenance TEXT NOT NULL CHECK(provenance IN ('managed','unmanaged')),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    install_entry BLOB CHECK(install_entry IS NULL OR (typeof(install_entry) = 'blob' AND length(install_entry) > 0))
);
