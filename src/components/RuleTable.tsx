import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type SortingState,
} from "@tanstack/react-table";
import { useMemo, useState } from "react";

import { tagStyle } from "../lib/palette";
import { formatLocation } from "../lib/volumes";
import type { Layout, Packaging, RuleRow, Schedule, TagId } from "../types";
import { AvailabilityPill, RunResultPill } from "./StatusPill";

declare module "@tanstack/react-table" {
  // Lets a cell reach the callbacks without threading them through every
  // column definition.
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  interface TableMeta<TData> {
    onEdit: ((row: RuleRow) => void) | undefined;
  }
}
/**
 * The rule table.
 *
 * Follows the reference mock-up's columns, with two additions the mock-up
 * could not express: a Last Result column, because a date alone does not say
 * whether the backup worked (docs/PLAN.md §1.1e), and a source availability
 * indicator, because a rule whose *source* drive is missing cannot run either.
 */

const columnHelper = createColumnHelper<RuleRow>();

/** Renders a Unix timestamp as a short date, or an em dash for none. */
function formatDate(seconds: number | null): string {
  if (seconds === null) return "—";
  return new Date(seconds * 1000).toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    year: "2-digit",
  });
}

function formatSchedule(schedule: Schedule): string {
  switch (schedule.kind) {
    case "manual":
      return "Manual";
    case "daily":
      return "Daily";
    case "weekly":
      return "Weekly";
    case "monthly":
      return "Monthly";
    case "cron":
      return schedule.value;
  }
}

function formatLayout(layout: Layout): string {
  return layout === "mirror" ? "Mirror" : "Snapshot";
}

function formatPackaging(packaging: Packaging): string {
  switch (packaging) {
    case "files":
      return "None";
    case "zip_store":
      return "Zip (store)";
    case "zip_deflate":
      return "Zip (deflate)";
  }
}

/** A rule's source, as `Drive · relative/path`. */
function sourceLocation(row: RuleRow): string {
  return formatLocation(row.source, row.rule.spec.source.relative);
}

const columns = [
  columnHelper.accessor((row) => row.tags.map((t) => t.name).join(" "), {
    id: "tags",
    header: "Tags",
    size: 120,
    cell: (ctx) => (
      <div className="flex flex-wrap gap-1">
        {ctx.row.original.tags.map((tag) => (
          <span
            key={tag.id}
            className="rounded px-1.5 py-0.5 text-[11px] leading-tight"
            style={tagStyle(tag.colour)}
          >
            {tag.name}
          </span>
        ))}
      </div>
    ),
  }),

  columnHelper.accessor((row) => row.rule.spec.name, {
    id: "name",
    header: "Rule",
    size: 150,
    cell: (ctx) => (
      <span
        className={ctx.row.original.rule.spec.enabled ? "" : "text-fg-muted line-through"}
      >
        {ctx.getValue()}
      </span>
    ),
  }),

  columnHelper.accessor((row) => row.rule.spec.source.relative, {
    id: "source",
    header: "Source",
    size: 165,
    cell: (ctx) => {
      const row = ctx.row.original;
      return (
        <span className="flex items-center gap-2">
          <span className="truncate" title={sourceLocation(row)}>
            {sourceLocation(row)}
          </span>
          {/* A missing source is as blocking as a missing destination, and the
              mock-up has nowhere to show it. Flag it inline rather than
              leaving the row looking runnable. */}
          {row.source.availability !== "available" && (
            <AvailabilityPill availability={row.source.availability} />
          )}
        </span>
      );
    },
  }),

  columnHelper.display({
    id: "destinations",
    header: "Destination",
    size: 165,
    cell: (ctx) => {
      const { destinations } = ctx.row.original;
      if (destinations.length === 0) {
        return <span className="text-fg-muted">No destination</span>;
      }
      return (
        <div className="space-y-0.5">
          {destinations.map((d) => (
            <div
              key={d.destination.id}
              className="truncate"
              title={formatLocation(d.status, d.destination.path.relative)}
            >
              {formatLocation(d.status, d.destination.path.relative)}
            </div>
          ))}
        </div>
      );
    },
  }),

  columnHelper.display({
    id: "availability",
    header: "Status",
    size: 125,
    cell: (ctx) => {
      const { destinations } = ctx.row.original;
      if (destinations.length === 0) return <span className="text-fg-muted">—</span>;
      return (
        <div className="space-y-0.5">
          {destinations.map((d) => (
            <div key={d.destination.id}>
              <AvailabilityPill availability={d.status.availability} />
            </div>
          ))}
        </div>
      );
    },
  }),

  columnHelper.accessor((row) => row.rule.spec.layout, {
    id: "layout",
    header: "Type",
    size: 90,
    cell: (ctx) => formatLayout(ctx.getValue()),
  }),

  columnHelper.accessor((row) => row.rule.spec.packaging, {
    id: "packaging",
    header: "Compression",
    size: 105,
    cell: (ctx) => formatPackaging(ctx.getValue()),
  }),

  columnHelper.accessor((row) => formatSchedule(row.rule.spec.schedule), {
    id: "schedule",
    header: "Frequency",
    size: 90,
  }),

  columnHelper.accessor((row) => row.last_run?.result ?? null, {
    id: "result",
    header: "Last Result",
    size: 100,
    cell: (ctx) => <RunResultPill result={ctx.getValue()} />,
  }),

  columnHelper.accessor((row) => row.last_run?.started_at ?? null, {
    id: "last_run",
    header: "Last Backup",
    size: 95,
    // Sorts on the raw timestamp rather than the formatted string, so
    // "1 Sep" does not sort after "14 Sep".
    sortingFn: "basic",
    cell: (ctx) => formatDate(ctx.getValue()),
  }),

  columnHelper.display({
    id: "next_run",
    header: "Next Backup",
    size: 95,
    // The scheduler lands in M3. Showing a guess here would be worse than
    // showing nothing, since the whole point of the column is to be trusted.
    cell: () => <span className="text-fg-muted">—</span>,
  }),

  columnHelper.display({
    id: "actions",
    header: "",
    size: 155,
    cell: (ctx) => (
      <div className="flex gap-3 text-xs whitespace-nowrap">
        {/* Always disabled: the engine lands in M1 and nothing copies files
            yet. A button that looks operational and silently does nothing is
            worse than no button in a backup tool — it invites someone to
            believe a backup ran. The reason still distinguishes "not built"
            from "this rule could not run anyway", because those are
            different things to know. */}
        <button
          type="button"
          disabled
          className="cursor-not-allowed text-fg-muted"
          title={
            isRunnable(ctx.row.original)
              ? "Running backups is not built yet — it arrives with the backup engine. Nothing is copied at this stage."
              : "This rule could not run in any case: the source or every destination is unavailable. Running backups is also not built yet."
          }
        >
          Backup Now
        </button>
        <button
          type="button"
          onClick={() => {
            ctx.table.options.meta?.onEdit?.(ctx.row.original);
          }}
          className="text-accent-blue underline-offset-2 hover:underline"
        >
          Edit Rule
        </button>
      </div>
    ),
  }),
];

/**
 * Whether a rule could run right now.
 *
 * Mirrors `RuleRow::is_runnable` in the core. A rule with two destinations,
 * one unplugged, can still back up to the other.
 */
function isRunnable(row: RuleRow): boolean {
  return (
    row.rule.spec.enabled &&
    row.source.availability === "available" &&
    row.destinations.some((d) => d.status.availability === "available")
  );
}

export function RuleTable({
  rows,
  tagFilter,
  onEdit,
  onCreate,
}: {
  rows: RuleRow[];
  /** Show only rules carrying every one of these tags. Empty shows all. */
  tagFilter?: TagId[];
  /** Open the editor for a rule. */
  onEdit?: (row: RuleRow) => void;
  /** Start a new rule, offered from the empty state. */
  onCreate?: () => void;
}) {
  const [sorting, setSorting] = useState<SortingState>([]);

  const filtered = useMemo(() => {
    if (tagFilter === undefined || tagFilter.length === 0) return rows;
    return rows.filter((row) => {
      const ids = new Set(row.tags.map((t) => t.id));
      return tagFilter.every((id) => ids.has(id));
    });
  }, [rows, tagFilter]);

  // React Compiler cannot analyse TanStack's hook and skips optimising this
  // component. That is expected and harmless — the table memoises internally
  // — but it is suppressed explicitly so CI can run at zero warnings.
  // eslint-disable-next-line react-hooks/incompatible-library
  const table = useReactTable({
    data: filtered,
    columns,
    state: { sorting },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    columnResizeMode: "onChange",
    meta: { onEdit },
  });

  if (rows.length === 0) {
    return (
      <div className="p-8 text-center text-sm text-fg-muted">
        <p>No backup rules yet.</p>
        <p className="mt-1">Create one to choose a folder and where to copy it.</p>
        {onCreate && (
          <button
            type="button"
            onClick={onCreate}
            className="mt-4 rounded px-3 py-1"
            style={{
              color: "var(--accent-blue)",
              backgroundColor: "var(--accent-blue-fill)",
            }}
          >
            New rule
          </button>
        )}
      </div>
    );
  }

  if (filtered.length === 0) {
    return (
      <div className="p-8 text-center text-sm text-fg-muted">
        No rules match the selected tags.
      </div>
    );
  }

  return (
    <table className="w-full min-w-[1455px] table-fixed border-collapse text-left text-[13px]">
      <thead className="sticky top-0 z-10 bg-surface">
        {table.getHeaderGroups().map((group) => (
          <tr key={group.id} className="border-b border-border">
            {group.headers.map((header) => {
              const sortable = header.column.getCanSort();
              const direction = header.column.getIsSorted();
              return (
                <th
                  key={header.id}
                  scope="col"
                  style={{ width: `${String(header.getSize())}px` }}
                  aria-sort={
                    direction === "asc"
                      ? "ascending"
                      : direction === "desc"
                        ? "descending"
                        : undefined
                  }
                  className="px-3 py-2 text-xs font-medium tracking-wide text-fg-muted uppercase"
                >
                  {sortable ? (
                    <button
                      type="button"
                      onClick={header.column.getToggleSortingHandler()}
                      className="flex items-center gap-1 uppercase hover:text-fg"
                    >
                      {flexRender(header.column.columnDef.header, header.getContext())}
                      <span aria-hidden="true" className="text-[10px]">
                        {direction === "asc" ? "▲" : direction === "desc" ? "▼" : ""}
                      </span>
                    </button>
                  ) : (
                    flexRender(header.column.columnDef.header, header.getContext())
                  )}
                </th>
              );
            })}
          </tr>
        ))}
      </thead>
      <tbody>
        {table.getRowModel().rows.map((row) => (
          <tr key={row.id} className="border-b border-border hover:bg-surface">
            {row.getVisibleCells().map((cell) => (
              <td key={cell.id} className="overflow-hidden px-3 py-2 align-top">
                {flexRender(cell.column.columnDef.cell, cell.getContext())}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
