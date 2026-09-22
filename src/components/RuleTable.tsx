import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type CellContext,
  type SortingState,
  type VisibilityState,
} from "@tanstack/react-table";
import { useMemo, useState } from "react";

import { tagStyle } from "../lib/palette";
import { volumeName } from "../lib/volumes";
import { DriveLocation } from "./DriveLocation";
import type {
  Layout,
  Packaging,
  PlaceholderPolicy,
  Retention,
  RuleId,
  RuleRow,
  Schedule,
  TagId,
  VolumeStatus,
} from "../types";
import { RunResultPill } from "./StatusPill";

declare module "@tanstack/react-table" {
  // Lets a cell reach the callbacks without threading them through every
  // column definition.
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  interface TableMeta<TData> {
    onEdit: ((row: RuleRow) => void) | undefined;
    onBackUp: ((row: RuleRow) => void) | undefined;
    /** The rule being backed up right now, if any. */
    running: RuleId | null;
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
 * A thin adapter: the core already records exactly what [`DriveLocation`]
 * wants — a volume and a path relative to it — so nothing is assembled here.
 */
function Location({ status, relative }: { status: VolumeStatus; relative: string }) {
  return (
    <div className={DESTINATION_ENTRY}>
      <DriveLocation
        name={volumeName(status)}
        relative={relative}
        mount={status.mount_point}
        availability={status.availability}
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
      return <Location status={row.source} relative={row.rule.spec.source.relative} />;
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
      <div className="flex flex-col items-stretch gap-1.5 text-xs whitespace-nowrap">
        <BackupNowButton ctx={ctx} />
        <button
          type="button"
          onClick={() => {
            ctx.table.options.meta?.onEdit?.(ctx.row.original);
          }}
          className="text-center text-fg-muted underline-offset-2 hover:text-fg hover:underline"
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
/**
 * Backup Now, in the one state it is currently in.
 *
 * Disabled for three different reasons, and the title says which: this rule
 * cannot run, another rule is running, or this rule already is. A button
 * that is merely grey teaches someone nothing about what to do next.
 */
function BackupNowButton({ ctx }: { ctx: CellContext<RuleRow, unknown> }) {
  const row = ctx.row.original;
  const running = ctx.table.options.meta?.running ?? null;
  const isThisRule = running === row.rule.id;
  const runnable = isRunnable(row);
  const disabled = !runnable || running !== null;

  const title = isThisRule
    ? "This backup is running. Cancel it from the status bar."
    : running !== null
      ? "Another backup is running. Shelv runs one at a time so two rules cannot write to one drive at once."
      : runnable
        ? "Copy this rule's source to every destination that is attached"
        : "This rule cannot run: its source, or every destination, is unavailable";

  // A filled button, not a link: this is the one control in the row that
  // does something to the disk, and it should not look like the text beside
  // it. Disabled keeps the shape and drops the fill, so a row where it
  // cannot run still reads as a row that has the button.
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={() => {
        ctx.table.options.meta?.onBackUp?.(row);
      }}
      title={title}
      className="w-full rounded border px-2 py-1 text-center"
      style={
        disabled
          ? { borderColor: "var(--border)", color: "var(--fg-muted)", opacity: 0.6 }
          : {
              borderColor: "transparent",
              color: "var(--accent-blue)",
              backgroundColor: "var(--accent-blue-fill)",
            }
      }
    >
      {isThisRule ? "Backing up…" : "Backup Now"}
    </button>
  );
}

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
const STICKY_CELL = "sticky right-0 z-20 border-l border-border";

export function RuleTable({
  rows,
  tagFilter,
  view = "simple",
  onEdit,
  onBackUp,
  running = null,
  onCreate,
}: {
  rows: RuleRow[];
  /** Show only rules carrying every one of these tags. Empty shows all. */
  tagFilter?: TagId[];
  /** How much of each rule's configuration to show. */
  view?: ViewMode;
  /** Open the editor for a rule. */
  onEdit?: (row: RuleRow) => void;
  /** Start a backup of a rule. */
  onBackUp?: (row: RuleRow) => void;
  /** The rule being backed up right now, if any. */
  running?: RuleId | null;
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
    meta: { onEdit, onBackUp, running },
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
                  className={`sticky top-0 z-10 border-b border-border px-3 py-2 text-xs font-medium tracking-wide text-fg-muted uppercase ${
                    pinned ? `${STICKY_CELL} z-30 bg-surface` : "bg-bg"
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
                  // The pinned column is set apart by sitting on the raised
                  // surface rather than by a bright rule down its edge: it
                  // reads as a shelf the actions live on, which is what it
                  // is, and a high-contrast line there drew the eye to a
                  // border instead of to the rows.
                  className={`overflow-hidden border-b border-border px-3 py-2 align-top ${
                    pinned
                      ? `${STICKY_CELL} bg-surface group-hover:bg-surface-raised`
                      : "bg-bg group-hover:bg-surface"
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
