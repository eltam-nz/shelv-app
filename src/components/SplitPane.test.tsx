import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { SplitPane } from "./SplitPane";

/**
 * The divider is a control, not a decoration. A resizer that only answers to
 * a mouse quietly makes the lower pane unreachable for anyone who does not
 * use one.
 */

function renderSplit(fraction = 0.5) {
  const changed = vi.fn();
  render(
    <SplitPane
      label="Height of the rules pane"
      fraction={fraction}
      onFractionChange={changed}
      top={<div>rules</div>}
      bottom={<div>drives</div>}
    />,
  );
  return { changed, divider: screen.getByRole("separator") };
}

describe("SplitPane", () => {
  it("reports its position to assistive technology", () => {
    const { divider } = renderSplit(0.5);
    expect(divider).toHaveAttribute("aria-valuenow", "50");
    expect(divider).toHaveAttribute("aria-valuemin", "15");
    expect(divider).toHaveAttribute("aria-valuemax", "85");
    expect(divider).toHaveAttribute("aria-label", "Height of the rules pane");
  });

  it("moves with the arrow keys", async () => {
    const user = userEvent.setup();
    const { changed, divider } = renderSplit(0.5);

    divider.focus();
    await user.keyboard("{ArrowDown}");
    expect(changed).toHaveBeenLastCalledWith(0.52);

    await user.keyboard("{ArrowUp}");
    expect(changed).toHaveBeenLastCalledWith(0.48);
  });

  it("goes to its limits with Home and End", async () => {
    const user = userEvent.setup();
    const { changed, divider } = renderSplit(0.5);

    divider.focus();
    await user.keyboard("{Home}");
    expect(changed).toHaveBeenLastCalledWith(0.15);

    await user.keyboard("{End}");
    expect(changed).toHaveBeenLastCalledWith(0.85);
  });

  it("will not let either pane be squeezed out of existence", async () => {
    // A pane collapsed to nothing looks like a bug, and there is no obvious
    // way back from it.
    const user = userEvent.setup();
    const { changed, divider } = renderSplit(0.16);

    divider.focus();
    await user.keyboard("{ArrowUp}");
    expect(changed).toHaveBeenLastCalledWith(0.15);
  });

  it("renders both panes", () => {
    renderSplit();
    expect(screen.getByText("rules")).toBeInTheDocument();
    expect(screen.getByText("drives")).toBeInTheDocument();
  });
});
