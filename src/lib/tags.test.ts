import { describe, expect, it } from "vitest";

import { retainExistingTags } from "./tags";
import type { Tag } from "../types";

const tag = (id: number, name: string): Tag => ({ id, name, colour: "pastel-blue" });

describe("retainExistingTags", () => {
  it("drops a filter whose tag has been deleted", () => {
    // Otherwise the table empties out and blames "the selected tags" for a
    // tag that is no longer anywhere on screen, with nothing left to clear
    // the filter with.
    expect(retainExistingTags([1, 2], [tag(1, "photos")])).toEqual([1]);
  });

  it("keeps the same array when nothing was dropped", () => {
    // Identity matters: this runs on every refresh, and a fresh array would
    // re-render the table about once a second for nothing.
    const filter = [1, 2];
    expect(retainExistingTags(filter, [tag(1, "photos"), tag(2, "docs")])).toBe(filter);
  });

  it("empties a filter whose tags have all gone", () => {
    expect(retainExistingTags([1, 2], [])).toEqual([]);
  });

  it("leaves an empty filter alone", () => {
    const filter: never[] = [];
    expect(retainExistingTags(filter, [tag(1, "photos")])).toBe(filter);
  });
});
