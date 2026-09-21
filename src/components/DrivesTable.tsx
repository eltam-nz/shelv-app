import { useRef, useState } from "react";

import { volumeName } from "../lib/volumes";
import type { DriveRow, VolumeId } from "../types";
import { AvailabilityPill } from "./StatusPill";

/**
 * The drives Shelv knows about.
 *
 * Recorded drives only. A drive is added by picking a folder on it through
 * the native dialog, which is the operating system's own consent step and
 * the single way a drive enters Shelv at all (docs/PLAN.md §4.2). Listing
 * every attached drive here would mean handing the frontend an inventory of
 * the user's hardware to no purpose — the picker is itself the system's list
 * of what is plugged in, so that is where a new drive comes from.
 */

/** Truncates an identity for display; the full value is on the title. */
function shortIdentity(value: string): string {
  return value.length <= 22 ? value : `${value.slice(0, 10)}…${value.slice(-8)}`;
}

export function DrivesTable({
  rows,
  onRename,
  onForget,
  onAdd,
  error,
}: {
  rows: DriveRow[];
  onRename: (id: VolumeId, nickname: string | null) => void;
  onForget: (id: VolumeId) => void;
  onAdd: () => void;
  /** A failure from the last drive action, shown in place. */
  error?: string | null;
}) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex shrink-0 items-center justify-between gap-3 border-b border-border px-3 py-2">
        <h2 className="text-xs font-medium tracking-wide text-fg-muted uppercase">
          Drives
        </h2>
        <div className="flex items-center gap-3">
          {error != null && error !== "" && (
            <span className="text-xs" style={{ color: "var(--status-mismatch)" }}>
              {error}
            </span>
          )}
          <button
            type="button"
            onClick={onAdd}
            title="Choose the drive, or any folder on it. Shelv records the drive."
            className="rounded px-3 py-1 text-xs"
            style={{
              color: "var(--accent-blue)",
              backgroundColor: "var(--accent-blue-fill)",
            }}
          >
            Add drive…
          </button>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-auto">
        {rows.length === 0 ? (
          <div className="p-6 text-center text-sm text-fg-muted">
            <p>No drives yet.</p>
            <p className="mt-1">
              Add one to give it a name, so rules can say which drive they mean.
            </p>
          </div>
        ) : (
          <table className="w-full min-w-[820px] table-fixed border-separate border-spacing-0 text-left text-[13px]">
            <thead>
              <tr>
                {[
                  ["Name", 180],
                  ["Name in Explorer", 160],
                  ["Drive", 80],
                  ["Status", 130],
                  ["Rules", 70],
                  ["Identity", 180],
                  ["", 90],
                ].map(([header, width]) => (
                  <th
                    key={String(header)}
                    scope="col"
                    style={{ width: `${String(width)}px` }}
                    className="sticky top-0 z-10 border-b border-border bg-bg px-3 py-2 text-xs font-medium tracking-wide text-fg-muted uppercase"
                  >
                    {header}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <DriveRowCells
                  key={row.status.volume.id}
                  row={row}
                  onRename={onRename}
                  onForget={onForget}
                />
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

function DriveRowCells({
  row,
  onRename,
  onForget,
}: {
  row: DriveRow;
  onRename: (id: VolumeId, nickname: string | null) => void;
  onForget: (id: VolumeId) => void;
}) {
  const { volume, availability, mount_point } = row.status;
  const inUse = row.rule_count > 0;

  return (
    <tr className="hover:bg-surface">
      <td className="border-b border-border px-3 py-1.5 align-middle">
        <NicknameField
          value={volume.nickname}
          placeholder={volumeName({
            ...row.status,
            volume: { ...volume, nickname: null },
          })}
          onCommit={(next) => {
            onRename(volume.id, next);
          }}
        />
      </td>
      <td className="truncate border-b border-border px-3 py-1.5 align-middle text-fg-muted">
        {volume.label ?? "—"}
      </td>
      <td className="border-b border-border px-3 py-1.5 align-middle">
        {mount_point ?? <span className="text-fg-muted">—</span>}
      </td>
      <td className="border-b border-border px-3 py-1.5 align-middle">
        <AvailabilityPill availability={availability} />
      </td>
      <td className="border-b border-border px-3 py-1.5 align-middle">
        {inUse ? row.rule_count : <span className="text-fg-muted">—</span>}
      </td>
      <td
        className="truncate border-b border-border px-3 py-1.5 align-middle font-mono text-[11px] text-fg-muted"
        title={volume.identity.value}
      >
        {shortIdentity(volume.identity.value)}
      </td>
      <td className="border-b border-border px-3 py-1.5 align-middle text-xs">
        {/* Forgetting a drive a rule points at would delete backup rules as
            a side effect of tidying a list. Refused, and said so here rather
            than only after the press. */}
        <button
          type="button"
          disabled={inUse}
          onClick={() => {
            onForget(volume.id);
          }}
          title={
            inUse
              ? `Used by ${String(row.rule_count)} ${row.rule_count === 1 ? "rule" : "rules"}. Remove or repoint ${row.rule_count === 1 ? "it" : "them"} first.`
              : "Remove Shelv's record of this drive. Nothing on the drive itself is touched."
          }
          // A disabled control that looks exactly like an enabled one is
          // worse than no control: it invites a press and then does nothing
          // visible. Both states were text-fg-muted, so they were
          // indistinguishable until you tried.
          className={
            inUse
              ? "cursor-not-allowed text-fg-muted opacity-40"
              : "text-fg-muted hover:text-fg"
          }
        >
          Forget
        </button>
      </td>
    </tr>
  );
}

/**
 * The nickname, edited in place.
 *
 * Commits on Enter and on blur, reverts on Escape. Clearing it is meaningful
 * — it falls back to the name Explorer reports — so an empty field is a
 * valid commit rather than something to reject.
 */
function NicknameField({
  value,
  placeholder,
  onCommit,
}: {
  value: string | null;
  placeholder: string;
  onCommit: (nickname: string | null) => void;
}) {
  const [draft, setDraft] = useState(value ?? "");

  // The watcher refreshes this table about once a second, so the field has
  // to follow a name that changed underneath it — the round trip from this
  // very edit, or a rename made elsewhere. Adjusted during render rather
  // than in an effect: an effect would paint the stale value first, and a
  // `key` would remount the input and take the focus with it.
  const [lastValue, setLastValue] = useState(value);
  if (value !== lastValue) {
    setLastValue(value);
    setDraft(value ?? "");
  }

  // Escape reverts and then blurs, and the blur handler would otherwise
  // commit the text Escape just discarded: `setDraft` has not landed by the
  // time blur fires, so `commit` would still read the abandoned draft. The
  // ref says "this blur is a cancellation" and survives the render that
  // state would not.
  const cancelling = useRef(false);

  const commit = () => {
    if (cancelling.current) {
      cancelling.current = false;
      return;
    }
    const trimmed = draft.trim();
    const next = trimmed === "" ? null : trimmed;
    if (next !== value) onCommit(next);
  };

  return (
    <input
      value={draft}
      placeholder={placeholder}
      aria-label={`Name for ${placeholder}`}
      onChange={(e) => {
        setDraft(e.target.value);
      }}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.currentTarget.blur();
        } else if (e.key === "Escape") {
          cancelling.current = true;
          setDraft(value ?? "");
          e.currentTarget.blur();
        }
      }}
      className="w-full rounded border border-transparent bg-transparent px-1.5 py-0.5 hover:border-border focus:border-border focus:bg-bg"
    />
  );
}
