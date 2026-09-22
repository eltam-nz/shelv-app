import { useCallback, useEffect, useState } from "react";

import { AboutPanel } from "./components/AboutPanel";
import { AppShell } from "./components/AppShell";
import { DrivesTable } from "./components/DrivesTable";
import { RuleEditor } from "./components/RuleEditor";
import { RuleTable, type ViewMode } from "./components/RuleTable";
import { SplitPane } from "./components/SplitPane";
import { TagFilterMenu } from "./components/TagFilterMenu";
import { TagManager } from "./components/TagManager";
import {
  cancelRun,
  deleteRule,
  forgetDrive,
  listDrives,
  listRules,
  listTags,
  onRunFinished,
  onRunProgress,
  onVolumesChanged,
  pickFolder,
  planRun,
  runningRule,
  runRule,
  setDriveNickname,
  ShelvError,
} from "./lib/ipc";
import { retainExistingTags } from "./lib/tags";
import { RunPreview } from "./components/RunPreview";
import type {
  DestinationPlan,
  DriveRow,
  RuleId,
  RuleRow,
  RunFinished,
  RunProgress,
  Tag,
  TagId,
  VolumeId,
} from "./types";

type Panel =
  | { kind: "none" }
  | { kind: "new-rule" }
  | { kind: "edit-rule"; row: RuleRow }
  | { kind: "tags" }
  | { kind: "about" }
  | { kind: "confirm-delete"; row: RuleRow }
  | { kind: "preview-run"; row: RuleRow; plans: DestinationPlan[] };

/**
 * Remembers a window-layout preference across sessions.
 *
 * Wrapped because `localStorage` throws outright in some configurations
 * rather than returning nothing, and a window that will not open because a
 * divider position could not be read would be a ridiculous way to fail.
 */
function usePersisted<T>(key: string, initial: T): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const stored = localStorage.getItem(key);
      return stored === null ? initial : (JSON.parse(stored) as T);
    } catch {
      return initial;
    }
  });

  const store = (next: T) => {
    setValue(next);
    try {
      localStorage.setItem(key, JSON.stringify(next));
    } catch {
      // A preference that cannot be saved is not worth interrupting anyone.
    }
  };

  return [value, store];
}

export function App() {
  const [rows, setRows] = useState<RuleRow[]>([]);
  const [drives, setDrives] = useState<DriveRow[]>([]);
  const [driveError, setDriveError] = useState<string | null>(null);
  const [showDrives, setShowDrives] = usePersisted("shelv.drives.shown", true);
  const [split, setSplit] = usePersisted("shelv.drives.split", 2 / 3);
  const [tags, setTags] = useState<Tag[]>([]);
  const [filter, setFilter] = useState<TagId[]>([]);
  const [panel, setPanel] = useState<Panel>({ kind: "none" });
  const [view, setView] = useState<ViewMode>("simple");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [running, setRunning] = useState<RuleId | null>(null);
  const [progress, setProgress] = useState<RunProgress | null>(null);
  const [outcome, setOutcome] = useState<RunFinished | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [nextRows, nextTags, nextDrives] = await Promise.all([
        listRules(),
        listTags(),
        listDrives(),
      ]);
      setRows(nextRows);
      setTags(nextTags);
      setDrives(nextDrives);
      // A tag can be deleted while it is being filtered by. Left alone, the
      // id would match nothing and the table would sit empty blaming a tag
      // that is no longer anywhere on screen.
      setFilter((current) => retainExistingTags(current, nextTags));
      setError(null);
    } catch (e: unknown) {
      setError(e instanceof ShelvError ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    const load = async () => {
      await refresh();
    };
    void load();
  }, [refresh]);

  // Plugging a drive in or pulling one out changes which rules can run, so
  // the table has to follow it. The backend only emits when something really
  // changed, so this is not a once-a-second re-render.
  useEffect(() => {
    let stop: (() => void) | null = null;
    let cancelled = false;

    void onVolumesChanged(() => {
      void refresh();
    }).then((unlisten) => {
      // The component can unmount before the listener is registered. Tear it
      // straight down in that case, or it outlives the component and keeps
      // calling refresh on a dead tree.
      if (cancelled) unlisten();
      else stop = unlisten;
    });

    return () => {
      cancelled = true;
      stop?.();
    };
  }, [refresh]);

  // A run outlives any one render, and the window can be reloaded while one
  // is going, so the backend is the source of truth for whether one is.
  useEffect(() => {
    let stop: (() => void)[] = [];
    let cancelled = false;

    const attach = (unlisten: () => void) => {
      if (cancelled) unlisten();
      else stop.push(unlisten);
    };

    void runningRule()
      .then(setRunning)
      .catch(() => {
        // Not knowing is the same as not running, as far as the table is
        // concerned: the backend refuses a second run anyway.
      });
    void onRunProgress((next) => {
      setRunning(next.rule);
      setProgress(next);
    }).then(attach);
    void onRunFinished((finished) => {
      setRunning(null);
      setProgress(null);
      setOutcome(finished);
      // The history, the last result and the last backup time all changed.
      void refresh();
    }).then(attach);

    return () => {
      cancelled = true;
      for (const unlisten of stop) unlisten();
      stop = [];
    };
  }, [refresh]);

  /**
   * Starts a backup, previewing it first when it would remove files.
   *
   * A mirror deletes whatever its source no longer has. That is what was
   * asked for, and it is also what destroys files when a rule points
   * somewhere unintended, so the first look at a destructive run is a
   * preview rather than the aftermath (`docs/PLAN.md` §5.1). A run that only
   * copies starts straight away: a confirmation nobody needs is a
   * confirmation nobody reads.
   */
  const backUp = async (row: RuleRow) => {
    setError(null);
    setOutcome(null);
    try {
      const plans = await planRun(row.rule.id);
      const deletes = plans.some(
        (plan) => plan.outcome.kind === "ready" && plan.outcome.plan.deletions.length > 0,
      );
      if (deletes) {
        setPanel({ kind: "preview-run", row, plans });
        return;
      }
      await start(row);
    } catch (e: unknown) {
      setError(e instanceof ShelvError ? e.message : String(e));
    }
  };

  const start = async (row: RuleRow) => {
    close();
    setRunning(row.rule.id);
    try {
      await runRule(row.rule.id);
    } catch (e: unknown) {
      // The run never started, so nothing will arrive to clear this.
      setRunning(null);
      setError(e instanceof ShelvError ? e.message : String(e));
    }
  };

  const close = () => {
    setPanel({ kind: "none" });
  };

  const afterChange = () => {
    close();
    void refresh();
  };

  const remove = async (row: RuleRow) => {
    try {
      await deleteRule(row.rule.id);
      afterChange();
    } catch (e: unknown) {
      setError(e instanceof ShelvError ? e.message : String(e));
      close();
    }
  };

  /** Runs a drive action, surfacing its refusal next to the drives table. */
  const driveAction = async (action: () => Promise<void>) => {
    setDriveError(null);
    try {
      await action();
      await refresh();
    } catch (e: unknown) {
      setDriveError(e instanceof ShelvError ? e.message : String(e));
    }
  };

  /**
   * Adds a drive through the native picker.
   *
   * The picker registers the volume as a side effect of the choice, which is
   * the whole mechanism: a drive enters Shelv only by the user choosing it in
   * the operating system's own dialog. The folder itself is not wanted here
   * — this is registering a drive, not remembering a location — so the
   * returned path is discarded.
   */
  const addDrive = () =>
    driveAction(async () => {
      await pickFolder();
    });

  const unreachable = rows.filter(
    (row) => !row.destinations.some((d) => d.status.availability === "available"),
  ).length;

  return (
    <>
      <AppShell
        title="Shelv — Backup Manager"
        actions={
          <div className="flex items-center gap-3">
            <TagFilterMenu
              tags={tags}
              filter={filter}
              onFilterChange={setFilter}
              onEditTags={() => {
                setPanel({ kind: "tags" });
              }}
            />
            <ViewToggle view={view} onChange={setView} />
            <button
              type="button"
              aria-pressed={showDrives}
              onClick={() => {
                setShowDrives(!showDrives);
              }}
              title={showDrives ? "Hide the drives pane" : "Show the drives pane"}
              className="rounded border border-border px-2.5 py-1 text-xs"
              style={
                showDrives
                  ? {
                      color: "var(--accent-blue)",
                      backgroundColor: "var(--accent-blue-fill)",
                    }
                  : { color: "var(--fg-muted)" }
              }
            >
              Drives
            </button>
            <button
              type="button"
              onClick={() => {
                setPanel({ kind: "new-rule" });
              }}
              className="rounded px-3 py-1 text-xs"
              style={{
                color: "var(--accent-blue)",
                backgroundColor: "var(--accent-blue-fill)",
              }}
            >
              New rule
            </button>
          </div>
        }
        status={
          error !== null ? (
            <span style={{ color: "var(--status-mismatch)" }}>{error}</span>
          ) : loading ? (
            "Loading…"
          ) : running !== null ? (
            <RunStatus
              rows={rows}
              running={running}
              progress={progress}
              onCancel={() => void cancelRun()}
            />
          ) : outcome !== null ? (
            <RunOutcomeLine rows={rows} outcome={outcome} />
          ) : (
            <>
              {rows.length} {rows.length === 1 ? "rule" : "rules"}
              {unreachable > 0 &&
                ` · ${String(unreachable)} with no reachable destination`}
            </>
          )
        }
        statusRight={
          <button
            type="button"
            onClick={() => {
              setPanel({ kind: "about" });
            }}
            title="Version, and where Shelv keeps your rules"
            className="shrink-0 whitespace-nowrap text-fg-muted hover:text-fg"
          >
            About Shelv
          </button>
        }
      >
        {!loading &&
          error === null &&
          (() => {
            const rulesPane = (
              <RuleTable
                rows={rows}
                tagFilter={filter}
                view={view}
                onEdit={(row) => {
                  setPanel({ kind: "edit-rule", row });
                }}
                onBackUp={(row) => void backUp(row)}
                running={running}
                onCreate={() => {
                  setPanel({ kind: "new-rule" });
                }}
              />
            );

            if (!showDrives) return rulesPane;

            return (
              <SplitPane
                label="Height of the rules pane"
                fraction={split}
                onFractionChange={setSplit}
                top={rulesPane}
                bottom={
                  <DrivesTable
                    rows={drives}
                    error={driveError}
                    onAdd={() => void addDrive()}
                    onRename={(id: VolumeId, nickname: string | null) =>
                      void driveAction(() => setDriveNickname(id, nickname))
                    }
                    onForget={(id: VolumeId) => void driveAction(() => forgetDrive(id))}
                  />
                }
              />
            );
          })()}
      </AppShell>

      {panel.kind === "preview-run" && (
        <RunPreview
          row={panel.row}
          plans={panel.plans}
          onCancel={close}
          onConfirm={() => void start(panel.row)}
        />
      )}

      {panel.kind === "new-rule" && (
        <RuleEditor tags={tags} onClose={close} onSaved={afterChange} />
      )}

      {panel.kind === "edit-rule" && (
        <RuleEditor
          existing={panel.row}
          tags={tags}
          onClose={close}
          onSaved={afterChange}
        />
      )}

      {panel.kind === "tags" && (
        <TagManager
          tags={tags}
          onClose={close}
          onChanged={() => {
            void refresh();
          }}
        />
      )}

      {panel.kind === "about" && <AboutPanel onClose={close} />}

      {panel.kind === "confirm-delete" && (
        <ConfirmDelete
          row={panel.row}
          onCancel={close}
          onConfirm={() => void remove(panel.row)}
        />
      )}
    </>
  );
}

/**
 * Switches between the everyday view and the full one.
 *
 * A segmented control rather than a checkbox: both states are named, so the
 * one you are not in is as legible as the one you are, and neither reads as
 * "off".
 */
function ViewToggle({
  view,
  onChange,
}: {
  view: ViewMode;
  onChange: (view: ViewMode) => void;
}) {
  const options: { value: ViewMode; label: string; hint: string }[] = [
    {
      value: "simple",
      label: "Simple",
      hint: "Where each backup goes and whether it is working",
    },
    {
      value: "detailed",
      label: "Detailed",
      hint: "Every setting on every rule, side by side",
    },
  ];

  return (
    <div
      role="group"
      aria-label="Table detail"
      className="flex overflow-hidden rounded border border-border text-xs"
    >
      {options.map((option) => {
        const active = view === option.value;
        return (
          <button
            key={option.value}
            type="button"
            aria-pressed={active}
            title={option.hint}
            onClick={() => {
              onChange(option.value);
            }}
            className="px-2.5 py-1"
            style={
              active
                ? {
                    color: "var(--accent-blue)",
                    backgroundColor: "var(--accent-blue-fill)",
                  }
                : { color: "var(--fg-muted)" }
            }
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

/**
 * Deleting a rule is irreversible for its history but harmless to files.
 *
 * Saying both explicitly matters: in a backup tool, "delete" reads as "delete
 * my backups", and someone who believes that will not press it.
 */
function ConfirmDelete({
  row,
  onCancel,
  onConfirm,
}: {
  row: RuleRow;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-8"
      role="dialog"
      aria-modal="true"
      aria-label="Delete rule"
    >
      <div className="w-full max-w-md rounded border border-border bg-surface p-4 text-[13px] shadow-xl">
        <h2 className="mb-2 text-sm font-semibold">Delete “{row.rule.spec.name}”?</h2>
        <p className="text-fg-muted">
          This removes the rule and its run history from Shelv. Files that have already
          been backed up are left exactly where they are — nothing on any drive is
          deleted.
        </p>
        <div className="mt-4 flex justify-end gap-3">
          <button
            type="button"
            onClick={onCancel}
            className="text-fg-muted hover:text-fg"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className="rounded px-3 py-1"
            style={{
              color: "var(--status-mismatch)",
              backgroundColor: "var(--status-mismatch-fill)",
            }}
          >
            Delete rule
          </button>
        </div>
      </div>
    </div>
  );
}

/** Bytes in the units a person reads. */
function size(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? String(value) : value.toFixed(1)} ${units[unit] ?? "B"}`;
}

function ruleName(rows: RuleRow[], id: RuleId): string {
  return rows.find((row) => row.rule.id === id)?.rule.spec.name ?? "a rule";
}

/**
 * What is happening, while it happens.
 *
 * Counts rather than a bar: the totals grow as the run reaches each
 * destination, so a bar would appear to go backwards. The file being copied
 * is shown because "something is happening" and "this file is taking a long
 * time" are different things to know.
 */
function RunStatus({
  rows,
  running,
  progress,
  onCancel,
}: {
  rows: RuleRow[];
  running: RuleId;
  progress: RunProgress | null;
  onCancel: () => void;
}) {
  return (
    <span className="flex min-w-0 items-center gap-3">
      <span className="shrink-0" style={{ color: "var(--accent-blue)" }}>
        Backing up {ruleName(rows, running)}
      </span>
      {progress !== null && (
        <>
          <span className="shrink-0">
            {progress.files_done} of {progress.files_total} files ·{" "}
            {size(progress.bytes_done)} of {size(progress.bytes_total)}
          </span>
          <span className="min-w-0 truncate font-mono text-fg-muted">
            {progress.file}
          </span>
        </>
      )}
      <button
        type="button"
        onClick={onCancel}
        className="shrink-0 underline-offset-2 hover:underline"
        style={{ color: "var(--status-mismatch)" }}
      >
        Cancel
      </button>
    </span>
  );
}

/**
 * How the last run went, until something else needs the status bar.
 *
 * Every destination is named separately, because each one succeeds or fails
 * on its own — a drive that was unplugged must not make the drive that
 * finished look like it failed.
 */
function RunOutcomeLine({ rows, outcome }: { rows: RuleRow[]; outcome: RunFinished }) {
  const name = ruleName(rows, outcome.rule);

  if (outcome.error !== null) {
    return (
      <span style={{ color: "var(--status-mismatch)" }}>
        {name} could not run: {outcome.error}
      </span>
    );
  }

  const ran = outcome.summaries.filter((summary) => summary.result !== null);
  const skipped = outcome.summaries.length - ran.length;
  const copied = ran.reduce((n, summary) => n + summary.stats.files_copied, 0);
  const deleted = ran.reduce((n, summary) => n + summary.stats.files_deleted, 0);
  const bytes = ran.reduce((n, summary) => n + summary.stats.bytes_copied, 0);
  const worst = ran.some((s) => s.result === "failed")
    ? "failed"
    : ran.some((s) => s.result === "cancelled")
      ? "cancelled"
      : ran.some((s) => s.result === "partial")
        ? "partial"
        : "ok";

  const words: Record<string, string> = {
    ok: "finished",
    partial: "finished, with files it could not copy",
    cancelled: "was cancelled",
    failed: "failed",
  };

  return (
    <span style={worst === "ok" ? undefined : { color: "var(--status-mismatch)" }}>
      {name} {words[worst] ?? "finished"} · {copied} copied · {size(bytes)}
      {deleted > 0 && ` · ${String(deleted)} moved to trash`}
      {skipped > 0 && ` · ${String(skipped)} destination unavailable`}
    </span>
  );
}
