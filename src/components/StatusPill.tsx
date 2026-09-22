import type { Availability, RunResult } from "../types";

/**
 * Status indicators.
 *
 * Every pill carries an icon *and* a word, not just a colour. The rule table
 * is the whole UI, and a reader who cannot distinguish the hues — or who
 * prints the window — still has to be able to tell a working backup from a
 * broken one.
 */

interface PillProps {
  /** A short glyph, readable without colour. */
  icon: string;
  /** The word. Never omitted. */
  label: string;
  /** Text colour token. */
  color: string;
  /** Chip background token, or none for the neutral state. */
  fill?: string;
  /** Longer explanation, surfaced on hover and to assistive tech. */
  title?: string;
}

function Pill({ icon, label, color, fill, title }: PillProps) {
  return (
    <span
      className="inline-flex items-center gap-1.5 rounded px-2 py-0.5 text-xs whitespace-nowrap"
      style={{ color, backgroundColor: fill ?? "transparent" }}
      title={title ?? label}
    >
      <span aria-hidden="true">{icon}</span>
      {label}
    </span>
  );
}

const AVAILABILITY: Record<Availability, Omit<PillProps, "title"> & { title: string }> = {
  available: {
    icon: "●",
    label: "Available",
    color: "var(--status-available)",
    fill: "var(--status-available-fill)",
    title: "Attached and ready to write",
  },
  disconnected: {
    icon: "○",
    label: "Disconnected",
    color: "var(--status-disconnected)",
    title: "Not currently attached. Plug the drive in to run this rule.",
  },
  refused: {
    icon: "⊘",
    label: "Refused",
    color: "var(--status-refused)",
    fill: "var(--status-refused-fill)",
    title:
      "Attached, but Shelv does not back up to this kind of volume — network shares, optical media and unclassifiable drives are excluded.",
  },
  // Present and permitted, but nothing identifies it across reconnections,
  // so it cannot be told apart from a different drive appearing in the same
  // place. Worded to point at the cause rather than sounding like a refusal
  // the user could argue with.
  unverifiable: {
    icon: "?",
    label: "Unidentified",
    color: "var(--status-refused)",
    fill: "var(--status-refused-fill)",
    title:
      "This drive is connected, but the system does not report anything that identifies it across reconnections. Shelv will not write to it, because it cannot tell it apart from a different drive plugged into the same place.",
  },
  // The dangerous one. Worded so it is obvious this is not just "unplugged":
  // something IS mounted there, and writing to it would be a mistake.
  identity_mismatch: {
    icon: "⚠",
    label: "Different drive",
    color: "var(--status-mismatch)",
    fill: "var(--status-mismatch-fill)",
    title:
      "A drive is mounted where this destination used to be, but it is not the same drive. Shelv will not write to it.",
  },
};

/**
 * The same states as {@link AvailabilityPill}, compressed to their glyph.
 *
 * For use beside a drive's name in the rule table, where a full pill in its
 * own column costs more width than the information is worth — the drives
 * pane already spells every state out, and most rows are `Available`, which
 * is the one state nobody needs to read.
 *
 * Still not colour alone: each state has a distinct glyph, so the shapes
 * differ in greyscale, and the word travels with it for a screen reader and
 * on hover. `Available` renders nothing at all — a row with no mark is the
 * ordinary one, and marking it would train the eye to skip exactly the
 * marks that matter.
 */
export function AvailabilityMark({ availability }: { availability: Availability }) {
  if (availability === "available") return null;
  const spec = AVAILABILITY[availability];
  return (
    <span
      className="shrink-0 text-[11px] leading-none"
      style={{ color: spec.color }}
      title={`${spec.label} — ${spec.title}`}
    >
      <span aria-hidden="true">{spec.icon}</span>
      <span className="sr-only">{spec.label}</span>
    </span>
  );
}

/**
 * Whether a destination can be written to, and if not, why.
 *
 * `mount` puts where the drive currently is inside the pill — "Available —
 * G:\" — rather than in a column of its own. The letter is only ever
 * meaningful as a qualifier on "this drive is here right now"; standing
 * alone it invites being read as part of the drive's identity, which is the
 * one thing it is not.
 *
 * It is absent whenever the drive is not attached, and deliberately absent
 * for a wrong-drive mismatch: something *is* mounted there, but it is not
 * this volume, so printing its location beside this row would name the wrong
 * disk.
 */
export function AvailabilityPill({
  availability,
  mount,
}: {
  availability: Availability;
  mount?: string | null;
}) {
  const spec = AVAILABILITY[availability];
  const at = mount !== null && mount !== undefined && mount !== "" ? mount : null;
  return (
    <Pill
      {...spec}
      label={at === null ? spec.label : `${spec.label} — ${at}`}
      title={at === null ? spec.title : `${spec.title} Currently at ${at}.`}
    />
  );
}

const RESULT: Record<RunResult, Omit<PillProps, "title"> & { title: string }> = {
  ok: {
    icon: "✓",
    label: "OK",
    color: "var(--result-ok)",
    title: "Every file was copied",
  },
  partial: {
    icon: "!",
    label: "Partial",
    color: "var(--result-partial)",
    title: "The run finished, but some files could not be read or written",
  },
  failed: {
    icon: "✕",
    label: "Failed",
    color: "var(--result-failed)",
    title: "The run did not complete",
  },
  cancelled: {
    icon: "–",
    label: "Cancelled",
    color: "var(--result-never)",
    title: "The run was stopped before it finished",
  },
};

/**
 * How the last run ended.
 *
 * `null` means the rule has never run, which is deliberately distinct from
 * having run and failed — the mock-up's date-only column could not express
 * the difference (docs/PLAN.md §1.1e).
 */
export function RunResultPill({ result }: { result: RunResult | null }) {
  if (result === null) {
    return (
      <Pill
        icon="·"
        label="Never run"
        color="var(--result-never)"
        title="This rule has not run yet"
      />
    );
  }
  return <Pill {...RESULT[result]} />;
}
