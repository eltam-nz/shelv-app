import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { TagFilterMenu } from "./TagFilterMenu";
import type { Tag, TagId } from "../types";

const TAGS: Tag[] = [
  { id: 1, name: "photos", colour: "pastel-lilac" },
  { id: 2, name: "documents", colour: "pastel-mint" },
];

function setup(filter: TagId[] = [], tags: Tag[] = TAGS) {
  const onFilterChange = vi.fn();
  const onEditTags = vi.fn();
  render(
    <TagFilterMenu
      tags={tags}
      filter={filter}
      onFilterChange={onFilterChange}
      onEditTags={onEditTags}
    />,
  );
  return {
    onFilterChange,
    onEditTags,
    trigger: screen.getByRole("button", { name: /Tags/ }),
    user: userEvent.setup(),
  };
}

describe("TagFilterMenu", () => {
  it("says how many tags are being filtered by", () => {
    // Chips used to make this visible at a glance. A closed dropdown hides
    // it, so someone could be staring at a table missing half its rows with
    // no indication of why.
    setup([1, 2]);
    expect(screen.getByRole("button", { name: /Tags · 2/ })).toBeInTheDocument();
  });

  it("reads as just Tags when nothing is filtered", () => {
    setup();
    expect(screen.getByRole("button", { name: "Tags" })).toBeInTheDocument();
  });

  it("keeps the panel shut until it is asked for", () => {
    const { trigger } = setup();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  it("offers every tag as a checkbox once opened", async () => {
    const { user, trigger } = setup([1]);
    await user.click(trigger);

    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("checkbox", { name: /photos/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /documents/ })).not.toBeChecked();
  });

  it("adds and removes a tag from the filter", async () => {
    const { user, trigger, onFilterChange } = setup([1]);
    await user.click(trigger);

    await user.click(screen.getByRole("checkbox", { name: /documents/ }));
    expect(onFilterChange).toHaveBeenLastCalledWith([1, 2]);

    await user.click(screen.getByRole("checkbox", { name: /photos/ }));
    expect(onFilterChange).toHaveBeenLastCalledWith([]);
  });

  it("stays open while filters are toggled", async () => {
    // Filtering is multi-select; reopening between each choice would be
    // tedious for the one case the control exists to serve.
    const { user, trigger } = setup();
    await user.click(trigger);
    await user.click(screen.getByRole("checkbox", { name: /photos/ }));
    expect(trigger).toHaveAttribute("aria-expanded", "true");
  });

  it("clears the whole filter", async () => {
    const { user, trigger, onFilterChange } = setup([1, 2]);
    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: "Clear" }));
    expect(onFilterChange).toHaveBeenCalledWith([]);
  });

  it("offers nothing to clear when nothing is filtered", async () => {
    const { user, trigger } = setup();
    await user.click(trigger);
    expect(screen.queryByRole("button", { name: "Clear" })).toBeNull();
  });

  it("closes on Escape and hands focus back to the trigger", async () => {
    // Otherwise focus is stranded inside a panel that is no longer there.
    const { user, trigger } = setup();
    await user.click(trigger);
    await user.keyboard("{Escape}");

    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(trigger).toHaveFocus();
  });

  it("closes when the pointer goes down outside it", async () => {
    const { user, trigger } = setup();
    await user.click(trigger);
    await user.click(document.body);
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("opens the tag manager and gets out of the way", async () => {
    const { user, trigger, onEditTags } = setup();
    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: /Edit tags/ }));

    expect(onEditTags).toHaveBeenCalledOnce();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("explains itself when there are no tags", async () => {
    const { user, trigger } = setup([], []);
    await user.click(trigger);
    expect(screen.getByText(/No tags yet/i)).toBeInTheDocument();
    // And still offers the way to make one.
    expect(screen.getByRole("button", { name: /Edit tags/ })).toBeInTheDocument();
  });
});
