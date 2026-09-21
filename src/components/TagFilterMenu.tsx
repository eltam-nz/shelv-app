import { useEffect, useId, useRef, useState } from "react";

import { tagStyle } from "../lib/palette";
import type { Tag, TagId } from "../types";

/**
 * The tag control: one button, with filtering and a way through to the tag
 * manager behind it.
 *
 * Filtering used to be one chip per tag laid out in the header, which put
 * the header's width at the mercy of how many tags someone made. Collapsing
 * it costs the at-a-glance view of what is filtered, so the trigger carries
 * the count — a user must never be looking at a table missing half its rows
 * with no indication of why.
 *
 * The panel is a form, not a menu. The rows are real checkboxes rather than
 * `role="menuitemcheckbox"`, because a menu is a list of commands and this
 * is a set of independent toggles; claiming otherwise would cost the native
 * keyboard and screen-reader behaviour and buy nothing.
 */
export function TagFilterMenu({
  tags,
  filter,
  onFilterChange,
  onEditTags,
}: {
  tags: Tag[];
  /** Selected tag ids. A rule must carry every one of them to be shown. */
  filter: TagId[];
  onFilterChange: (filter: TagId[]) => void;
  /** Opens the tag manager. The panel closes first. */
  onEditTags: () => void;
}) {
  const [open, setOpen] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panelId = useId();

  const close = (returnFocus: boolean) => {
    setOpen(false);
    // Focus goes back to the trigger when the panel was dismissed
    // deliberately, and is left alone when the user clicked elsewhere — in
    // that case they have already chosen where they want to be.
    if (returnFocus) trigger.current?.focus();
  };

  useEffect(() => {
    if (!open) return undefined;

    const onPointerDown = (event: PointerEvent) => {
      if (!container.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setOpen(false);
        trigger.current?.focus();
      }
    };

    // Capture, so this runs before a click inside the table can act on it.
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const toggle = (id: TagId) => {
    onFilterChange(
      filter.includes(id) ? filter.filter((t) => t !== id) : [...filter, id],
    );
  };

  const active = filter.length > 0;

  return (
    <div ref={container} className="relative">
      <button
        ref={trigger}
        type="button"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => {
          setOpen(!open);
        }}
        title={
          active
            ? `Filtering by ${String(filter.length)} ${filter.length === 1 ? "tag" : "tags"}`
            : "Filter by tag"
        }
        className="rounded border border-border px-2.5 py-1 text-xs"
        style={
          active
            ? { color: "var(--accent-blue)", backgroundColor: "var(--accent-blue-fill)" }
            : { color: "var(--fg-muted)" }
        }
      >
        {active ? `Tags · ${String(filter.length)}` : "Tags"}
      </button>

      {open && (
        <div
          id={panelId}
          // Above the table's sticky headers, which reach z-30 for the
          // pinned actions column.
          className="absolute right-0 z-50 mt-1 w-64 rounded border border-border bg-surface text-[13px] shadow-xl"
        >
          <div className="max-h-72 overflow-auto p-1.5">
            {tags.length === 0 ? (
              <p className="px-2 py-3 text-center text-xs text-fg-muted">
                No tags yet. Create one to group and filter rules.
              </p>
            ) : (
              tags.map((tag) => (
                <label
                  key={tag.id}
                  className="flex cursor-pointer items-center gap-2 rounded px-2 py-1 hover:bg-surface-raised"
                >
                  <input
                    type="checkbox"
                    checked={filter.includes(tag.id)}
                    onChange={() => {
                      toggle(tag.id);
                    }}
                  />
                  <span
                    className="truncate rounded px-1.5 py-0.5 text-xs"
                    style={tagStyle(tag.colour)}
                  >
                    {tag.name}
                  </span>
                </label>
              ))
            )}
          </div>

          <div className="flex items-center justify-between gap-3 border-t border-border px-2 py-1.5 text-xs">
            <button
              type="button"
              onClick={() => {
                onEditTags();
                close(false);
              }}
              className="text-accent-blue underline-offset-2 hover:underline"
            >
              Edit tags…
            </button>
            {active && (
              <button
                type="button"
                onClick={() => {
                  onFilterChange([]);
                }}
                className="text-fg-muted hover:text-fg"
              >
                Clear
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
