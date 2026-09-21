import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

/**
 * Two stacked panes with a divider the user can drag.
 *
 * The divider is a real control, not a decoration: it carries
 * `role="separator"` with the value range assistive technology needs, it
 * takes focus, and it responds to the arrow keys. A resizer that only works
 * with a mouse quietly makes the lower pane unreachable for anyone who does
 * not use one, and the rest of this app holds the keyboard line.
 */

/** How small either pane may get, as a fraction of the whole. */
const MIN_FRACTION = 0.15;
const MAX_FRACTION = 0.85;
/** How far one arrow-key press moves the divider. */
const KEY_STEP = 0.02;

function clamp(fraction: number): number {
  return Math.min(MAX_FRACTION, Math.max(MIN_FRACTION, fraction));
}

export function SplitPane({
  top,
  bottom,
  fraction,
  onFractionChange,
  label,
}: {
  top: ReactNode;
  bottom: ReactNode;
  /** The top pane's share of the height, 0–1. */
  fraction: number;
  onFractionChange: (fraction: number) => void;
  /** Names the divider for assistive technology. */
  label: string;
}) {
  const container = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);

  const fractionAt = useCallback((clientY: number): number | null => {
    const box = container.current?.getBoundingClientRect();
    if (!box || box.height === 0) return null;
    return clamp((clientY - box.top) / box.height);
  }, []);

  // Listeners go on the window rather than the divider so the drag survives
  // the pointer leaving it, which it will: the pointer moves faster than
  // React re-renders, and a drag that stops the moment you overshoot is
  // worse than no drag at all.
  useEffect(() => {
    if (!dragging) return undefined;

    const move = (event: MouseEvent) => {
      const next = fractionAt(event.clientY);
      if (next !== null) onFractionChange(next);
    };
    const stop = () => {
      setDragging(false);
    };

    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", stop);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", stop);
    };
  }, [dragging, fractionAt, onFractionChange]);

  const onKeyDown = (event: React.KeyboardEvent) => {
    const step =
      event.key === "ArrowUp" ? -KEY_STEP : event.key === "ArrowDown" ? KEY_STEP : 0;
    if (step !== 0) {
      event.preventDefault();
      onFractionChange(clamp(fraction + step));
      return;
    }
    // Home and End go to the extremes, which is how a separator is expected
    // to behave and saves forty key presses.
    if (event.key === "Home") {
      event.preventDefault();
      onFractionChange(MIN_FRACTION);
    } else if (event.key === "End") {
      event.preventDefault();
      onFractionChange(MAX_FRACTION);
    }
  };

  return (
    <div ref={container} className="flex h-full min-h-0 flex-col">
      <div
        className="min-h-0 overflow-auto"
        style={{ height: `${String(fraction * 100)}%` }}
      >
        {top}
      </div>

      <div
        role="separator"
        aria-label={label}
        aria-orientation="horizontal"
        aria-valuenow={Math.round(fraction * 100)}
        aria-valuemin={Math.round(MIN_FRACTION * 100)}
        aria-valuemax={Math.round(MAX_FRACTION * 100)}
        tabIndex={0}
        onMouseDown={(event) => {
          event.preventDefault();
          setDragging(true);
        }}
        onKeyDown={onKeyDown}
        // Taller than it looks: the visible line is 1px, but a 1px drag
        // target is a test of aim rather than a control.
        className={`group relative h-2 shrink-0 cursor-row-resize ${
          dragging ? "bg-surface-raised" : "hover:bg-surface-raised"
        }`}
      >
        <div className="pointer-events-none absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-border-strong" />
      </div>

      <div className="min-h-0 flex-1 overflow-auto">{bottom}</div>
    </div>
  );
}
