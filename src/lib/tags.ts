import type { Tag, TagId } from "../types";

/**
 * Drops filter entries whose tag no longer exists.
 *
 * The filter is a list of tag ids, and tags can be deleted while one is
 * selected. Without this, the id stays behind and matches nothing, so the
 * table empties out and reports "No rules match the selected tags" — about a
 * tag that is not on screen anywhere, because it is gone. There is then no
 * control left to clear it with.
 *
 * Returns the original array when nothing was dropped, so a caller holding
 * it in state does not re-render on every refresh.
 */
export function retainExistingTags(filter: TagId[], tags: Tag[]): TagId[] {
  const existing = new Set(tags.map((tag) => tag.id));
  const kept = filter.filter((id) => existing.has(id));
  return kept.length === filter.length ? filter : kept;
}
