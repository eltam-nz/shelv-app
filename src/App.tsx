import { useCallback, useEffect, useState } from "react";

import { AppShell } from "./components/AppShell";
import { RuleTable } from "./components/RuleTable";
import { listRules, listTags } from "./lib/ipc";
import { tagStyle } from "./lib/palette";
import type { RuleRow, Tag, TagId } from "./types";

export function App() {
  const [rows, setRows] = useState<RuleRow[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [filter, setFilter] = useState<TagId[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    try {
      const [nextRows, nextTags] = await Promise.all([listRules(), listTags()]);
      setRows(nextRows);
      setTags(nextTags);
      setError(null);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // Wrapped rather than called directly: the lint rule cannot see that
    // `refresh` awaits before it touches state, and reads the bare call as a
    // synchronous setState inside an effect.
    const load = async () => {
      await refresh();
    };
    void load();
  }, [refresh]);

  const toggleTag = (id: TagId) => {
    setFilter((current) =>
      current.includes(id) ? current.filter((t) => t !== id) : [...current, id],
    );
  };

  const unavailable = rows.filter(
    (row) => !row.destinations.some((d) => d.status.availability === "available"),
  ).length;

  return (
    <AppShell
      title="Shelv"
      actions={
        tags.length > 0 && (
          <div className="flex items-center gap-1.5">
            {tags.map((tag) => {
              const active = filter.includes(tag.id);
              return (
                <button
                  key={tag.id}
                  type="button"
                  onClick={() => {
                    toggleTag(tag.id);
                  }}
                  aria-pressed={active}
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
        )
      }
      status={
        error !== null ? (
          <span style={{ color: "var(--status-mismatch)" }}>{error}</span>
        ) : loading ? (
          "Loading…"
        ) : (
          <>
            {rows.length} rules
            {unavailable > 0 && ` · ${String(unavailable)} with no reachable destination`}
          </>
        )
      }
    >
      {!loading && error === null && <RuleTable rows={rows} tagFilter={filter} />}
    </AppShell>
  );
}
