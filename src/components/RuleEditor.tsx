import { useCallback, useEffect, useState } from "react";

import {
  createRule,
  pickFolder,
  problemsFrom,
  ShelvError,
  updateRule,
  validateRule,
} from "../lib/ipc";
import { tagStyle } from "../lib/palette";
import {
  describeProblem,
  generalProblems,
  problemsForDestination,
} from "../lib/problems";
import type {
  Layout,
  Packaging,
  PlaceholderPolicy,
  RuleProblem,
  RuleRow,
  RuleSpec,
  Schedule,
  Tag,
  TagId,
  VolumePath,
} from "../types";

/**
 * Create and edit a rule.
 *
 * Source and destinations can only be set through the native folder picker —
 * there is no path field to type into, because the picker is the operating
 * system's consent step and the only thing that registers a drive
 * (docs/PLAN.md §4.2).
 */

/** A destination being edited, with enough context to show it. */
interface DestinationDraft {
  path: VolumePath;
  label: string | null;
  display: string;
}

/** A source being edited. */
interface SourceDraft {
  path: VolumePath;
  label: string | null;
  display: string;
}

function emptySpec(source: VolumePath): RuleSpec {
  return {
    name: "",
    enabled: true,
    source,
    layout: "mirror",
    packaging: "files",
    allow_deletions: false,
    retention: { kind: "unlimited" },
    schedule: { kind: "manual" },
    run_on_connect: true,
    catch_up: true,
    placeholders: "hydrate",
    hydrate_budget_bytes: null,
    follow_symlinks: false,
    excludes: [],
  };
}

function scheduleValue(schedule: Schedule): string {
  return schedule.kind === "cron" ? "cron" : schedule.kind;
}

export function RuleEditor({
  existing,
  tags,
  onClose,
  onSaved,
}: {
  /** The rule being edited, or `undefined` to create a new one. */
  existing?: RuleRow;
  /** Every tag, for the assignment picker. */
  tags: Tag[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const [source, setSource] = useState<SourceDraft | null>(
    existing
      ? {
          path: existing.rule.spec.source,
          label: existing.source.volume.label,
          display: `${existing.source.volume.label ?? "?"} · ${existing.rule.spec.source.relative}`,
        }
      : null,
  );
  const [spec, setSpec] = useState<RuleSpec | null>(
    existing ? { ...existing.rule.spec } : null,
  );
  const [destinations, setDestinations] = useState<DestinationDraft[]>(
    existing
      ? existing.destinations.map((d) => ({
          path: d.destination.path,
          label: d.status.volume.label,
          display: `${d.status.volume.label ?? "?"} · ${d.destination.path.relative}`,
        }))
      : [],
  );
  const [selectedTags, setSelectedTags] = useState<TagId[]>(
    existing ? existing.tags.map((t) => t.id) : [],
  );
  const [problems, setProblems] = useState<RuleProblem[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const update = (patch: Partial<RuleSpec>) => {
    setSpec((current) => (current ? { ...current, ...patch } : current));
  };

  // Re-check as the rule is edited, so a problem shows up when it is
  // introduced rather than only when Save is pressed.
  useEffect(() => {
    if (!spec) return;
    const check = async () => {
      try {
        setProblems(
          await validateRule(
            spec,
            destinations.map((d) => d.path),
          ),
        );
      } catch {
        // A failed check must not block editing; Save validates again.
      }
    };
    void check();
  }, [spec, destinations]);

  const chooseSource = useCallback(async () => {
    setError(null);
    try {
      const picked = await pickFolder();
      if (!picked) return;
      const draft: SourceDraft = {
        path: picked.path,
        label: picked.volume_label,
        display: picked.display_path,
      };
      setSource(draft);
      setSpec((current) =>
        current ? { ...current, source: picked.path } : emptySpec(picked.path),
      );
    } catch (e: unknown) {
      setError(e instanceof ShelvError ? e.message : String(e));
    }
  }, []);

  const addDestination = useCallback(async () => {
    setError(null);
    try {
      const picked = await pickFolder();
      if (!picked) return;
      setDestinations((current) => [
        ...current,
        {
          path: picked.path,
          label: picked.volume_label,
          display: picked.display_path,
        },
      ]);
    } catch (e: unknown) {
      setError(e instanceof ShelvError ? e.message : String(e));
    }
  }, []);

  const save = async () => {
    if (!spec) return;
    setSaving(true);
    setError(null);
    try {
      const paths = destinations.map((d) => d.path);
      if (existing) {
        await updateRule(existing.rule.id, spec, paths, selectedTags);
      } else {
        await createRule(spec, paths, selectedTags);
      }
      onSaved();
    } catch (e: unknown) {
      const refused = problemsFrom(e);
      if (refused.length > 0) {
        setProblems(refused);
        setError("This rule was refused. See the problems below.");
      } else {
        setError(e instanceof ShelvError ? e.message : String(e));
      }
    } finally {
      setSaving(false);
    }
  };

  const blocked = problems.length > 0 || !spec || !source;

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-auto bg-black/60 p-8"
      role="dialog"
      aria-modal="true"
      aria-label={existing ? "Edit rule" : "New rule"}
    >
      <div className="w-full max-w-2xl rounded border border-border bg-surface shadow-xl">
        <header className="flex items-center justify-between border-b border-border px-4 py-3">
          <h2 className="text-sm font-semibold">{existing ? "Edit rule" : "New rule"}</h2>
          <button type="button" onClick={onClose} className="text-fg-muted hover:text-fg">
            Close
          </button>
        </header>

        <div className="space-y-5 p-4 text-[13px]">
          {error !== null && (
            <p
              className="rounded px-3 py-2"
              style={{
                color: "var(--status-mismatch)",
                backgroundColor: "var(--status-mismatch-fill)",
              }}
            >
              {error}
            </p>
          )}

          <Field label="Source folder" hint="The folder being backed up.">
            <div className="flex items-center gap-3">
              <button
                type="button"
                onClick={() => void chooseSource()}
                className="rounded border border-border-strong px-3 py-1 hover:bg-surface-raised"
              >
                {source ? "Change…" : "Choose folder…"}
              </button>
              {source && (
                <span className="truncate text-fg-muted" title={source.display}>
                  {source.display}
                </span>
              )}
            </div>
          </Field>

          {spec && source && (
            <>
              <Field label="Name">
                <input
                  value={spec.name}
                  onChange={(e) => {
                    update({ name: e.target.value });
                  }}
                  placeholder="Lightroom Catalog"
                  className="w-full rounded border border-border bg-bg px-2 py-1"
                />
              </Field>

              <Field
                label="Destinations"
                hint="Where copies are written. A rule can have several; it runs against whichever are connected."
              >
                <div className="space-y-2">
                  {destinations.map((destination, index) => {
                    const theirs = problemsForDestination(problems, index);
                    return (
                      <div
                        key={`${String(destination.path.volume)}:${destination.path.relative}`}
                        className="rounded border border-border p-2"
                      >
                        <div className="flex items-start justify-between gap-3">
                          <span className="truncate" title={destination.display}>
                            {destination.display}
                          </span>
                          <button
                            type="button"
                            onClick={() => {
                              setDestinations((current) =>
                                current.filter((_, i) => i !== index),
                              );
                            }}
                            className="shrink-0 text-fg-muted hover:text-fg"
                          >
                            Remove
                          </button>
                        </div>
                        {theirs.map((problem) => (
                          <p
                            key={problem.kind}
                            className="mt-1 text-xs"
                            style={{ color: "var(--status-mismatch)" }}
                          >
                            {describeProblem(problem)}
                          </p>
                        ))}
                      </div>
                    );
                  })}
                  <button
                    type="button"
                    onClick={() => void addDestination()}
                    className="rounded border border-border-strong px-3 py-1 hover:bg-surface-raised"
                  >
                    Add destination…
                  </button>
                </div>
              </Field>

              <div className="grid grid-cols-2 gap-4">
                <Field label="Backup type">
                  <Select
                    value={spec.layout}
                    onChange={(v) => {
                      update({ layout: v as Layout });
                    }}
                    options={[
                      ["mirror", "Mirror — keep the destination matching the source"],
                      ["snapshot", "Snapshot — a new timestamped folder each run"],
                    ]}
                  />
                </Field>

                <Field
                  label="Compression"
                  hint="All options are lossless. Photos and video are already compressed, so deflate costs time for almost nothing."
                >
                  <Select
                    value={spec.packaging}
                    onChange={(v) => {
                      update({ packaging: v as Packaging });
                    }}
                    options={[
                      ["files", "None — plain files and folders"],
                      ["zip_store", "Zip, stored — one archive, not compressed"],
                      ["zip_deflate", "Zip, deflated — one compressed archive"],
                    ]}
                  />
                </Field>

                <Field label="How often">
                  <Select
                    value={scheduleValue(spec.schedule)}
                    onChange={(v) => {
                      update({
                        schedule:
                          v === "cron"
                            ? { kind: "cron", value: "0 3 * * 1" }
                            : ({ kind: v } as Schedule),
                      });
                    }}
                    options={[
                      ["manual", "Only when I ask"],
                      ["daily", "Daily"],
                      ["weekly", "Weekly"],
                      ["monthly", "Monthly"],
                      ["cron", "Custom schedule"],
                    ]}
                  />
                  {spec.schedule.kind === "cron" && (
                    <input
                      value={spec.schedule.value}
                      onChange={(e) => {
                        update({ schedule: { kind: "cron", value: e.target.value } });
                      }}
                      className="mt-2 w-full rounded border border-border bg-bg px-2 py-1 font-mono text-xs"
                    />
                  )}
                </Field>

                {spec.layout === "snapshot" && (
                  <Field
                    label="Keep"
                    hint="Snapshots accumulate until the drive is full, so a limit is part of this backup type."
                  >
                    <Select
                      value={spec.retention.kind}
                      onChange={(v) => {
                        update({
                          retention:
                            v === "unlimited"
                              ? { kind: "unlimited" }
                              : v === "keep_last_n"
                                ? { kind: "keep_last_n", value: 6 }
                                : { kind: "keep_days", value: 90 },
                        });
                      }}
                      options={[
                        ["keep_last_n", "The newest few"],
                        ["keep_days", "Anything recent"],
                        ["unlimited", "Everything (the drive will fill)"],
                      ]}
                    />
                    {spec.retention.kind !== "unlimited" && (
                      <input
                        type="number"
                        min={1}
                        value={spec.retention.value}
                        onChange={(e) => {
                          const value = Math.max(1, Number(e.target.value));
                          update({
                            retention:
                              spec.retention.kind === "keep_last_n"
                                ? { kind: "keep_last_n", value }
                                : { kind: "keep_days", value },
                          });
                        }}
                        className="mt-2 w-24 rounded border border-border bg-bg px-2 py-1"
                      />
                    )}
                  </Field>
                )}

                <Field
                  label="Cloud files"
                  hint="OneDrive files that are not stored on this computer."
                >
                  <Select
                    value={spec.placeholders}
                    onChange={(v) => {
                      update({ placeholders: v as PlaceholderPolicy });
                    }}
                    options={[
                      ["hydrate", "Download them and back them up"],
                      ["hydrate_release", "Download, back up, then free the space again"],
                      ["skip", "Skip them"],
                    ]}
                  />
                </Field>
              </div>

              <Field label="Tags">
                <div className="flex flex-wrap gap-1.5">
                  {tags.length === 0 && (
                    <span className="text-fg-muted">No tags yet.</span>
                  )}
                  {tags.map((tag) => {
                    const on = selectedTags.includes(tag.id);
                    return (
                      <button
                        key={tag.id}
                        type="button"
                        aria-pressed={on}
                        onClick={() => {
                          setSelectedTags((current) =>
                            on
                              ? current.filter((t) => t !== tag.id)
                              : [...current, tag.id],
                          );
                        }}
                        className="rounded px-2 py-0.5 text-xs"
                        style={
                          on
                            ? tagStyle(tag.colour)
                            : {
                                color: "var(--fg-muted)",
                                backgroundColor: "var(--surface-raised)",
                              }
                        }
                      >
                        {tag.name}
                      </button>
                    );
                  })}
                </div>
              </Field>

              <Toggle
                checked={spec.enabled}
                onChange={(v) => {
                  update({ enabled: v });
                }}
                label="Enabled"
                hint="A disabled rule is never run automatically."
              />

              <Toggle
                checked={spec.run_on_connect}
                onChange={(v) => {
                  update({ run_on_connect: v });
                }}
                label="Run when the drive is connected"
                hint="For a drive that is only plugged in occasionally, this matters more than the schedule."
              />

              <Toggle
                checked={spec.catch_up}
                onChange={(v) => {
                  update({ catch_up: v });
                }}
                label="Catch up on missed runs"
                hint="Run as soon as possible if the machine was off or the drive absent when it was due."
              />

              {spec.layout === "mirror" && (
                <Toggle
                  checked={spec.allow_deletions}
                  onChange={(v) => {
                    update({ allow_deletions: v });
                  }}
                  label="Delete files from the backup when they are deleted from the source"
                  hint="The only setting here that can destroy data. Off, the backup only ever grows. On, it tracks the source exactly — including removals."
                  danger
                />
              )}

              <Field label="Ignore" hint="One pattern per line, e.g. *.tmp or Thumbs.db.">
                <textarea
                  value={spec.excludes.join("\n")}
                  onChange={(e) => {
                    update({
                      excludes: e.target.value
                        .split("\n")
                        .map((line) => line.trim())
                        .filter((line) => line.length > 0),
                    });
                  }}
                  rows={3}
                  className="w-full rounded border border-border bg-bg px-2 py-1 font-mono text-xs"
                />
              </Field>

              {generalProblems(problems).map((problem) => (
                <p
                  key={problem.kind}
                  className="text-xs"
                  style={{ color: "var(--status-mismatch)" }}
                >
                  {describeProblem(problem)}
                </p>
              ))}
            </>
          )}
        </div>

        <footer className="flex items-center justify-end gap-3 border-t border-border px-4 py-3">
          <button type="button" onClick={onClose} className="text-fg-muted hover:text-fg">
            Cancel
          </button>
          <button
            type="button"
            onClick={() => void save()}
            disabled={blocked || saving}
            title={blocked ? "Fix the problems above first" : "Save this rule"}
            className="rounded px-3 py-1 disabled:cursor-not-allowed disabled:opacity-50"
            style={{
              color: "var(--accent-blue)",
              backgroundColor: "var(--accent-blue-fill)",
            }}
          >
            {saving ? "Saving…" : existing ? "Save changes" : "Create rule"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs font-medium tracking-wide text-fg-muted uppercase">
        {label}
      </span>
      {children}
      {hint !== undefined && (
        <span className="mt-1 block text-xs text-fg-muted">{hint}</span>
      )}
    </label>
  );
}

function Select({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (value: string) => void;
  options: [string, string][];
}) {
  return (
    <select
      value={value}
      onChange={(e) => {
        onChange(e.target.value);
      }}
      className="w-full rounded border border-border bg-bg px-2 py-1"
    >
      {options.map(([key, text]) => (
        <option key={key} value={key}>
          {text}
        </option>
      ))}
    </select>
  );
}

function Toggle({
  checked,
  onChange,
  label,
  hint,
  danger,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
  hint?: string;
  danger?: boolean;
}) {
  return (
    <label
      className="flex gap-2 rounded p-2"
      style={
        danger === true && checked
          ? { backgroundColor: "var(--status-mismatch-fill)" }
          : undefined
      }
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => {
          onChange(e.target.checked);
        }}
        className="mt-0.5"
      />
      <span>
        <span
          className="block"
          style={
            danger === true && checked ? { color: "var(--status-mismatch)" } : undefined
          }
        >
          {label}
        </span>
        {hint !== undefined && (
          <span className="block text-xs text-fg-muted">{hint}</span>
        )}
      </span>
    </label>
  );
}
