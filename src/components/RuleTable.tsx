import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type SortingState,
  type VisibilityState,
} from "@tanstack/react-table";
import { useMemo, useState } from "react";

import { tagStyle } from "../lib/palette";
import { formatLocation, fullPath, volumeName } from "../lib/volumes";
import { DriveLocation } from "./DriveLocation";
import type {
  Layout,
  Packaging,
  PlaceholderPolicy,
  Retention,
  RuleRow,
  Schedule,
  TagId,
  VolumeStatus,
} from "../types";
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

/** How much of a rule's configuration the table shows. */
export type ViewMode = "simple" | "detailed";

/**
 * Columns hidden in the simple view.
 *
 * These three are the ones you set once and then stop thinking about, so in
 * day-to-day use they are width spent on nothing. Everything the simple view
 * keeps answers "is my data safe right now?" — where it goes, whether the
 * drive is there, whether the last run worked.
 */
const CONFIGURATION_COLUMNS = ["layout", "packaging", "schedule"];

/** Columns only the detailed view shows: the rest of the rule's settings. */
const DETAIL_COLUMNS = [
  "placeholders",
  "run_on_connect",
  "catch_up",
  "retention",
  "follow_symlinks",
  "excludes",
];

/** Which columns each view shows. */
function columnVisibility(view: ViewMode): VisibilityState {
  const hidden =
    view === "simple" ? [...CONFIGURATION_COLUMNS, ...DETAIL_COLUMNS] : DETAIL_COLUMNS;
  const visible = view === "detailed" ? DETAIL_COLUMNS : [];
  return Object.fromEntries([
    ...hidden.map((id) => [id, false] as const),
    ...visible.map((id) => [id, true] as const),
  ]);
}

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

function formatPlaceholders(policy: PlaceholderPolicy): string {
  switch (policy) {
    case "hydrate":
      return "Download";
    case "hydrate_release":
      return "Download & release";
    case "skip":
      return "Skip";
  }
}

function formatRetention(retention: Retention): string {
  switch (retention.kind) {
    case "unlimited":
      return "All";
    case "keep_last_n":
      return `Last ${String(retention.value)}`;
    case "keep_days":
      return `${String(retention.value)} days`;
  }
}

function yesNo(value: boolean): string {
  return value ? "Yes" : "No";
}

const DESTINATION_ENTRY = "flex min-h-9 flex-col justify-center";

/**
 * One stored location in the table.
 *
 * Translates what the core records — a volume plus a path relative to it —
 * into the path and drive name [`DriveLocation`] renders. When the drive has
 * never been seen at a mount point there is no full path to build, so the
 * drive-relative form stands in.
 */
function Location({ status, relative }: { status: VolumeStatus; relative: string }) {
  const path = fullPath(status, relative);

  return (
    <div className={DESTINATION_ENTRY}>
      <DriveLocation
        name={volumeName(status)}
        path={path ?? formatLocation(status, relative)}
        connected={status.mount_point !== null}
      />
    </div>
  );
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
    size: 250,
    cell: (ctx) => {
      const row = ctx.row.original;
      return (
        <span className="flex items-start gap-2">
          <span className="min-w-0 flex-1">
            <Location status={row.source} relative={row.rule.spec.source.relative} />
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
    size: 250,
    cell: (ctx) => {
      const { destinations } = ctx.row.original;
      if (destinations.length === 0) {
        return <span className="text-fg-muted">No destination</span>;
      }
      return (
        <div>
          {destinations.map((d) => (
            <Location
              key={d.destination.id}
              status={d.status}
              relative={d.destination.path.relative}
            />
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
        <div>
          {destinations.map((d) => (
            <div key={d.destination.id} className={DESTINATION_ENTRY}>
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

  columnHelper.accessor((row) => row.rule.spec.placeholders, {
    id: "placeholders",
    header: "Cloud",
    size: 130,
    cell: (ctx) => {
      const { spec } = ctx.row.original.rule;
      const budget = spec.hydrate_budget_bytes;
      return (
        <span
          title={
            budget === null
              ? undefined
              : `At most ${String(Math.round(budget / 1_000_000))} MB downloaded per run`
          }
        >
          {formatPlaceholders(ctx.getValue())}
          {budget !== null && " *"}
        </span>
      );
    },
  }),

  columnHelper.accessor((row) => row.rule.spec.run_on_connect, {
    id: "run_on_connect",
    header: "On connect",
    size: 90,
    cell: (ctx) => yesNo(ctx.getValue()),
  }),

  columnHelper.accessor((row) => row.rule.spec.catch_up, {
    id: "catch_up",
    header: "Catch up",
    size: 80,
    cell: (ctx) => yesNo(ctx.getValue()),
  }),

  columnHelper.accessor((row) => formatRetention(row.rule.spec.retention), {
    id: "retention",
    header: "Keep",
    size: 80,
  }),

  columnHelper.accessor((row) => row.rule.spec.follow_symlinks, {
    id: "follow_symlinks",
    header: "Links",
    size: 70,
    cell: (ctx) => yesNo(ctx.getValue()),
  }),

  columnHelper.accessor((row) => row.rule.spec.excludes.length, {
    id: "excludes",
    header: "Ignore",
    size: 80,
    cell: (ctx) => {
      const count = ctx.getValue();
      if (count === 0) return <span className="text-fg-muted">—</span>;
      return (
        <span title={ctx.row.original.rule.spec.excludes.join("\n")}>
          {count} {count === 1 ? "pattern" : "patterns"}
        </span>
      );
    },
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
      <div className="flex flex-col items-start gap-1 text-xs whitespace-nowrap">
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

/**
 * The actions column is pinned to the right edge.
 *
 * It has to stay reachable and stay beside its own row. Rendering it as a
 * separate element next to the table would satisfy the first and break the
 * second: rows are not a fixed height — a rule with three destinations is
 * three times as tall as one with a single destination — so two independent
 * lists would have to have their heights measured and copied across on every
 * change, and would drift whenever that measurement lagged a render.
 *
 * A sticky cell inside the same row is the same thing without the drift. It
 * is aligned by construction because it *is* the row, and it still detaches
 * visually and stays put while the rest of the table scrolls underneath —
 * which it does, since the detailed view is far wider than any window.
 */
const STICKY_CELL = "sticky right-0 z-20 border-l border-border-strong";

export function RuleTable({
  rows,
  tagFilter,
  view = "simple",
  onEdit,
  onCreate,
}: {
  rows: RuleRow[];
  /** Show only rules carrying every one of these tags. Empty shows all. */
  tagFilter?: TagId[];
  /** How much of each rule's configuration to show. */
  view?: ViewMode;
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

  const visibility = useMemo(() => columnVisibility(view), [view]);

  // React Compiler cannot analyse TanStack's hook and skips optimising this
  // component. That is expected and harmless — the table memoises internally
  // — but it is suppressed explicitly so CI can run at zero warnings.
  // eslint-disable-next-line react-hooks/incompatible-library
  const table = useReactTable({
    data: filtered,
    columns,
    state: { sorting, columnVisibility: visibility },
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
    // `border-separate` rather than `border-collapse`: a collapsed border is
    // owned by the table, not the cell, so it does not travel with a sticky
    // cell and the pinned column loses its edge as soon as you scroll.
    <table
      className="w-full table-fixed border-separate border-spacing-0 text-left text-[13px]"
      style={{ minWidth: `${String(table.getTotalSize())}px` }}
    >
      <thead>
        {table.getHeaderGroups().map((group) => (
          <tr key={group.id}>
            {group.headers.map((header) => {
              const sortable = header.column.getCanSort();
              const direction = header.column.getIsSorted();
              const pinned = header.column.id === "actions";
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
                  className={`sticky top-0 z-10 border-b border-border bg-bg px-3 py-2 text-xs font-medium tracking-wide text-fg-muted uppercase ${
                    pinned ? `${STICKY_CELL} z-30` : ""
                  }`}
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
          // The pinned cell needs its own background so the rest of the row
          // scrolls under it rather than through it, and `group` is what
          // lets that background follow the row's hover state.
          <tr key={row.id} className="group">
            {row.getVisibleCells().map((cell) => {
              const pinned = cell.column.id === "actions";
              return (
                <td
                  key={cell.id}
                  className={`overflow-hidden border-b border-border bg-bg px-3 py-2 align-top group-hover:bg-surface ${
                    pinned ? STICKY_CELL : ""
                  }`}
                >
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </td>
              );
            })}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
