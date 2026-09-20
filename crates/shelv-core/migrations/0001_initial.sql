-- Shelv schema v1. See docs/PLAN.md §2.3.
--
-- Append-only: once this has shipped it is never edited. Changes go in a new
-- numbered migration.

-- A volume Shelv has seen, identified by something that survives replugging.
-- Mount points and drive letters are recorded for display only; Windows
-- reassigns letters, so a rule keyed on one can end up writing to a different
-- disk (PLAN §1.1a).
CREATE TABLE volume (
    id            INTEGER PRIMARY KEY,
    identity_kind TEXT    NOT NULL,
    identity      TEXT    NOT NULL,
    serial        TEXT,
    label         TEXT,
    filesystem    TEXT,
    drive_type    TEXT    NOT NULL,
    is_sync_root  INTEGER NOT NULL DEFAULT 0 CHECK (is_sync_root IN (0, 1)),
    last_seen_at  INTEGER,
    last_mount    TEXT,
    UNIQUE (identity_kind, identity)
);

CREATE TABLE rule (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT    NOT NULL,
    enabled              INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    source_volume        INTEGER NOT NULL REFERENCES volume(id),
    source_rel           TEXT    NOT NULL,
    layout               TEXT    NOT NULL CHECK (layout IN ('mirror', 'snapshot')),
    packaging            TEXT    NOT NULL
                             CHECK (packaging IN ('files', 'zip_store', 'zip_deflate')),
    -- Deletions default off. This is the one setting that can destroy data,
    -- so it is opt-in per rule (PLAN §4, T4).
    allow_deletions      INTEGER NOT NULL DEFAULT 0 CHECK (allow_deletions IN (0, 1)),
    retention_kind       TEXT    CHECK (retention_kind IN ('keep_last_n', 'keep_days')),
    retention_value      INTEGER CHECK (retention_value IS NULL OR retention_value > 0),
    schedule             TEXT    NOT NULL,
    run_on_connect       INTEGER NOT NULL DEFAULT 1 CHECK (run_on_connect IN (0, 1)),
    catch_up             INTEGER NOT NULL DEFAULT 1 CHECK (catch_up IN (0, 1)),
    placeholders         TEXT    NOT NULL DEFAULT 'hydrate'
                             CHECK (placeholders IN ('hydrate', 'hydrate_release', 'skip')),
    hydrate_budget_bytes INTEGER CHECK (hydrate_budget_bytes IS NULL
                                        OR hydrate_budget_bytes > 0),
    -- Following links lets one inside the source tree redirect the copy
    -- outside it (PLAN §4, T2).
    follow_symlinks      INTEGER NOT NULL DEFAULT 0 CHECK (follow_symlinks IN (0, 1)),
    excludes             TEXT    NOT NULL DEFAULT '[]',
    created_at           INTEGER NOT NULL,
    -- A retention kind without a count, or a count without a kind, would be
    -- read as "keep everything" and quietly stop pruning.
    CHECK ((retention_kind IS NULL) = (retention_value IS NULL))
);

CREATE INDEX idx_rule_enabled ON rule(enabled);
CREATE INDEX idx_rule_source_volume ON rule(source_volume);

-- A rule may write to several destinations ("add backup directories").
CREATE TABLE destination (
    id         INTEGER PRIMARY KEY,
    rule_id    INTEGER NOT NULL REFERENCES rule(id) ON DELETE CASCADE,
    volume_id  INTEGER NOT NULL REFERENCES volume(id),
    dest_rel   TEXT    NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    -- The same rule writing twice to one place would race with itself.
    UNIQUE (rule_id, volume_id, dest_rel)
);

CREATE INDEX idx_destination_rule ON destination(rule_id);
CREATE INDEX idx_destination_volume ON destination(volume_id);

CREATE TABLE tag (
    id     INTEGER PRIMARY KEY,
    name   TEXT NOT NULL UNIQUE,
    -- A palette token, not a raw colour, so the theme can guarantee contrast.
    colour TEXT NOT NULL
);

CREATE TABLE rule_tag (
    rule_id INTEGER NOT NULL REFERENCES rule(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tag(id)  ON DELETE CASCADE,
    PRIMARY KEY (rule_id, tag_id)
);

CREATE INDEX idx_rule_tag_tag ON rule_tag(tag_id);

CREATE TABLE run (
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
    result                TEXT    CHECK (result IN ('ok', 'partial', 'failed', 'cancelled')),
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

-- The rule table's "last run" column is the hot query.
CREATE INDEX idx_run_rule_started ON run(rule_id, started_at DESC);
CREATE INDEX idx_run_destination ON run(destination_id);

CREATE TABLE run_event (
    id      INTEGER PRIMARY KEY,
    run_id  INTEGER NOT NULL REFERENCES run(id) ON DELETE CASCADE,
    level   TEXT    NOT NULL CHECK (level IN ('warn', 'error')),
    path    TEXT    NOT NULL,
    message TEXT    NOT NULL
);

CREATE INDEX idx_run_event_run ON run_event(run_id);
