import { useState } from "react";

import { createTag, deleteTag, ShelvError } from "../lib/ipc";
import { TAG_TOKENS, tagStyle } from "../lib/palette";
import type { Tag, TagId } from "../types";

/**
 * Create and delete tags.
 *
 * Colours come from a fixed palette rather than a colour picker. That is the
 * mechanism by which every tag stays readable: the tokens are the only values
 * a tag can take, and each is verified against its chip background by
 * `scripts/check-contrast.mjs`.
 */
export function TagManager({
  tags,
  onClose,
  onChanged,
}: {
  tags: Tag[];
  onClose: () => void;
  onChanged: () => void;
}) {
  const [name, setName] = useState("");
  const [colour, setColour] = useState<string>(TAG_TOKENS[0]);
  const [error, setError] = useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = useState<TagId | null>(null);

  const add = async () => {
    setError(null);
    try {
      await createTag(name.trim(), colour);
      setName("");
      onChanged();
    } catch (e: unknown) {
      setError(e instanceof ShelvError ? e.message : String(e));
    }
  };

  const remove = async (id: TagId) => {
    setError(null);
    try {
      await deleteTag(id);
      setPendingDelete(null);
      onChanged();
    } catch (e: unknown) {
      setError(e instanceof ShelvError ? e.message : String(e));
    }
  };

  const duplicate = tags.some((t) => t.name.toLowerCase() === name.trim().toLowerCase());

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-auto bg-black/60 p-8"
      role="dialog"
      aria-modal="true"
      aria-label="Tags"
    >
      <div className="w-full max-w-md rounded border border-border bg-surface shadow-xl">
        <header className="flex items-center justify-between border-b border-border px-4 py-3">
          <h2 className="text-sm font-semibold">Tags</h2>
          <button type="button" onClick={onClose} className="text-fg-muted hover:text-fg">
            Close
          </button>
        </header>

        <div className="space-y-4 p-4 text-[13px]">
          {error !== null && (
            <p className="text-xs" style={{ color: "var(--status-mismatch)" }}>
              {error}
            </p>
          )}

          <div className="space-y-2">
            {tags.length === 0 && (
              <p className="text-fg-muted">
                No tags yet. Tags group rules in the table and filter it.
              </p>
            )}
            {tags.map((tag) => (
              <div key={tag.id} className="flex items-center justify-between gap-3">
                <span
                  className="rounded px-2 py-0.5 text-xs"
                  style={tagStyle(tag.colour)}
                >
                  {tag.name}
                </span>
                {pendingDelete === tag.id ? (
                  <span className="flex items-center gap-2 text-xs">
                    <span className="text-fg-muted">
                      Remove from every rule that uses it?
                    </span>
                    <button
                      type="button"
                      onClick={() => void remove(tag.id)}
                      style={{ color: "var(--status-mismatch)" }}
                    >
                      Delete
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setPendingDelete(null);
                      }}
                      className="text-fg-muted"
                    >
                      Keep
                    </button>
                  </span>
                ) : (
                  <button
                    type="button"
                    onClick={() => {
                      setPendingDelete(tag.id);
                    }}
                    className="text-xs text-fg-muted hover:text-fg"
                  >
                    Delete
                  </button>
                )}
              </div>
            ))}
          </div>

          <div className="space-y-2 border-t border-border pt-4">
            <input
              value={name}
              onChange={(e) => {
                setName(e.target.value);
              }}
              placeholder="New tag name"
              className="w-full rounded border border-border bg-bg px-2 py-1"
            />
            <div className="flex flex-wrap gap-1.5">
              {TAG_TOKENS.map((token) => (
                <button
                  key={token}
                  type="button"
                  aria-pressed={colour === token}
                  aria-label={token.replace("pastel-", "")}
                  onClick={() => {
                    setColour(token);
                  }}
                  className="rounded px-2 py-0.5 text-xs"
                  style={{
                    ...tagStyle(token),
                    outline: colour === token ? "2px solid var(--focus)" : undefined,
                    outlineOffset: "1px",
                  }}
                >
                  {name.trim() === "" ? token.replace("pastel-", "") : name.trim()}
                </button>
              ))}
            </div>
            <button
              type="button"
              onClick={() => void add()}
              disabled={name.trim() === "" || duplicate}
              title={duplicate ? "A tag with this name already exists" : "Add this tag"}
              className="rounded px-3 py-1 disabled:cursor-not-allowed disabled:opacity-50"
              style={{
                color: "var(--accent-blue)",
                backgroundColor: "var(--accent-blue-fill)",
              }}
            >
              Add tag
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
