import { useCallback, useEffect, useState } from "react";

import { AppShell } from "./components/AppShell";
import { RuleEditor } from "./components/RuleEditor";
import { RuleTable } from "./components/RuleTable";
import { TagManager } from "./components/TagManager";
import { deleteRule, listRules, listTags, ShelvError } from "./lib/ipc";
import { tagStyle } from "./lib/palette";
import type { RuleRow, Tag, TagId } from "./types";

type Panel =
  | { kind: "none" }
  | { kind: "new-rule" }
  | { kind: "edit-rule"; row: RuleRow }
  | { kind: "tags" }
  | { kind: "confirm-delete"; row: RuleRow };

export function App() {
  const [rows, setRows] = useState<RuleRow[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [filter, setFilter] = useState<TagId[]>([]);
  const [panel, setPanel] = useState<Panel>({ kind: "none" });
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    try {
      const [nextRows, nextTags] = await Promise.all([listRules(), listTags()]);
      setRows(nextRows);
      setTags(nextTags);
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

  const unreachable = rows.filter(
    (row) => !row.destinations.some((d) => d.status.availability === "available"),
  ).length;

  return (
    <>
      <AppShell
        title="Shelv"
        actions={
          <div className="flex items-center gap-3">
            {tags.length > 0 && (
              <div className="flex items-center gap-1.5">
                {tags.map((tag) => {
                  const active = filter.includes(tag.id);
                  return (
                    <button
                      key={tag.id}
                      type="button"
                      onClick={() => {
                        setFilter((current) =>
                          current.includes(tag.id)
                            ? current.filter((t) => t !== tag.id)
                            : [...current, tag.id],
                        );
                      }}
                      aria-pressed={active}
                      title={
                        active ? `Stop filtering by ${tag.name}` : `Show only ${tag.name}`
                      }
                      className="rounded px-2 py-0.5 text-xs"
                      style={
                        active
                          ? tagStyle(tag.colour)
                          : { color: "var(--fg-muted)", backgroundColor: "transparent" }
                      }
                    >
                      {tag.name}
                    </button>
                  );
                })}
              </div>
            )}
            <button
              type="button"
              onClick={() => {
                setPanel({ kind: "tags" });
              }}
              className="text-xs text-fg-muted hover:text-fg"
            >
              Tags…
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
          ) : (
            <>
              {rows.length} {rows.length === 1 ? "rule" : "rules"}
              {unreachable > 0 &&
                ` · ${String(unreachable)} with no reachable destination`}
              {" · nothing is copied yet — the backup engine is not built"}
            </>
          )
        }
      >
        {!loading && error === null && (
          <RuleTable
            rows={rows}
            tagFilter={filter}
            onEdit={(row) => {
              setPanel({ kind: "edit-rule", row });
            }}
            onCreate={() => {
              setPanel({ kind: "new-rule" });
            }}
          />
        )}
      </AppShell>

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
