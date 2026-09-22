import type { DestinationPlan, Plan, RuleRow } from "../types";

/** Bytes in the units a person reads, to one decimal above a kilobyte. */
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

function count(n: number, one: string, many: string): string {
  return `${String(n)} ${n === 1 ? one : many}`;
}

/**
 * What a run would do, shown before it does it.
 *
 * Only for a run that would remove something. A mirror deletes whatever the
 * source no longer has, which is the behaviour someone asked for and also the
 * one that destroys files if a rule points at the wrong folder — so the first
 * destructive run of a rule is previewable rather than immediate
 * (`docs/PLAN.md` §5.1).
 *
 * The preview comes from the same planner call the run itself makes, so it
 * cannot describe a different run from the one that follows.
 */
export function RunPreview({
  row,
  plans,
  onConfirm,
  onCancel,
}: {
  row: RuleRow;
  plans: DestinationPlan[];
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const ready = plans.flatMap((plan): Plan[] =>
    plan.outcome.kind === "ready" ? [plan.outcome.plan] : [],
  );
  const unavailable = plans.filter((plan) => plan.outcome.kind === "unavailable");

  const copies = ready.reduce((n, plan) => n + plan.copies.length, 0);
  const bytes = ready.reduce((n, plan) => n + plan.bytes, 0);
  const deletions = ready.flatMap((plan) => plan.deletions);
  const deletedBytes = deletions.reduce((n, deletion) => n + deletion.bytes, 0);

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-auto bg-black/60 p-8"
      role="dialog"
      aria-modal="true"
      aria-label={`Preview of ${row.rule.spec.name}`}
    >
      <div className="w-full max-w-lg rounded border border-border bg-surface shadow-xl">
        <div className="border-b border-border px-5 py-3">
          <h2 className="text-sm font-semibold">
            Before backing up {row.rule.spec.name}
          </h2>
        </div>

        <div className="space-y-4 px-5 py-4 text-sm">
          <p className="text-fg-muted">
            This rule mirrors its source, so files the source no longer has are removed
            from the backup.
          </p>

          <dl className="space-y-1">
            <div className="flex justify-between">
              <dt>Files to copy</dt>
              <dd>
                {count(copies, "file", "files")} · {size(bytes)}
              </dd>
            </div>
            <div className="flex justify-between">
              <dt style={{ color: "var(--status-mismatch)" }}>Files to remove</dt>
              <dd style={{ color: "var(--status-mismatch)" }}>
                {count(deletions.length, "file", "files")} · {size(deletedBytes)}
              </dd>
            </div>
          </dl>

          {deletions.length > 0 && (
            <div>
              <p className="mb-1 text-xs text-fg-muted">
                Removed files are moved to a <code>.shelv-trash</code> folder on the
                destination drive, keeping their paths, so this can be undone by moving
                them back. Nothing is deleted outright.
              </p>
              <ul className="max-h-40 overflow-auto rounded border border-border p-2 font-mono text-xs">
                {deletions.slice(0, 50).map((deletion) => (
                  <li key={deletion.relative} className="truncate">
                    {deletion.relative}
                  </li>
                ))}
              </ul>
              {deletions.length > 50 && (
                <p className="mt-1 text-xs text-fg-muted">
                  and {count(deletions.length - 50, "more file", "more files")}.
                </p>
              )}
            </div>
          )}

          {unavailable.length > 0 && (
            <p className="text-xs text-fg-muted">
              {count(unavailable.length, "destination is", "destinations are")} not
              available and will be left alone.
            </p>
          )}
        </div>

        <div className="flex justify-end gap-2 border-t border-border px-5 py-3">
          <button
            type="button"
            onClick={onCancel}
            className="rounded border border-border px-3 py-1 text-xs"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className="rounded px-3 py-1 text-xs"
            style={{
              color: "var(--accent-blue)",
              backgroundColor: "var(--accent-blue-fill)",
            }}
          >
            Back up now
          </button>
        </div>
      </div>
    </div>
  );
}
