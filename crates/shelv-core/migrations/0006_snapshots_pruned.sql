-- Retention deletes snapshots outright, so the history has to say so.
--
-- It is the only thing Shelv removes without a way back: mirror deletions
-- move to a trash folder on the same volume, but pruning exists to reclaim
-- that space, so there is nowhere for a pruned snapshot to go. Something
-- that deletes and leaves no record is exactly what the run history is for
-- (docs/M3.md).
ALTER TABLE run ADD COLUMN snapshots_pruned INTEGER NOT NULL DEFAULT 0;
