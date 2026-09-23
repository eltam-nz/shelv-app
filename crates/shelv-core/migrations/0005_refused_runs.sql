-- A run the deletion guard stopped is not a failure.
--
-- M3 starts runs nobody is watching, and a mirror run removes whatever the
-- source no longer has. When the source mounts empty, or is renamed, that
-- is faithfully reproduced as an empty backup — and with M1 the only thing
-- between that and someone's photographs was a human reading the preview.
--
-- An unattended run that stops for that reason did not fail: nothing is
-- broken, and filing it as 'failed' would put it in the column someone
-- checks for dying drives. It needs its own spelling (docs/M3.md).
--
-- SQLite cannot alter a CHECK constraint, so the table is rebuilt. The
-- column list is copied verbatim; only the constraint changes. Legacy
-- foreign keys are deferred for the swap, which is why this migration
-- carries its own pragma rather than relying on the connection's.
PRAGMA foreign_keys = OFF;

CREATE TABLE run_new (
    id                    INTEGER PRIMARY KEY,
    rule_id               INTEGER NOT NULL REFERENCES rule(id) ON DELETE CASCADE,
    destination_id        INTEGER NOT NULL REFERENCES destination(id) ON DELETE CASCADE,
    trigger               TEXT    NOT NULL
                              CHECK ("trigger" IN ('manual', 'schedule', 'catch_up',
                                                   'on_connect')),
    started_at            INTEGER NOT NULL,
    finished_at           INTEGER,
    -- NULL while the run is in progress. 'partial' is distinct from 'ok' on
    -- purpose: a run that could not read every file has not succeeded.
    -- 'refused' is distinct from 'failed' for the same kind of reason.
    result                TEXT    CHECK (result IN ('ok', 'partial', 'failed',
                                                    'cancelled', 'refused')),
    files_copied          INTEGER NOT NULL DEFAULT 0,
    files_skipped         INTEGER NOT NULL DEFAULT 0,
    files_deleted         INTEGER NOT NULL DEFAULT 0,
    bytes_copied          INTEGER NOT NULL DEFAULT 0,
    compressed_bytes      INTEGER,
    placeholders_hydrated INTEGER NOT NULL DEFAULT 0,
    placeholders_skipped  INTEGER NOT NULL DEFAULT 0,
    bytes_hydrated        INTEGER NOT NULL DEFAULT 0,
    bytes_released        INTEGER NOT NULL DEFAULT 0,
    error                 TEXT,
    snapshot_path         TEXT
);

INSERT INTO run_new SELECT * FROM run;

DROP TABLE run;
ALTER TABLE run_new RENAME TO run;

-- The indexes went with the old table.
CREATE INDEX idx_run_rule_started ON run(rule_id, started_at DESC);
CREATE INDEX idx_run_destination ON run(destination_id);

PRAGMA foreign_keys = ON;
