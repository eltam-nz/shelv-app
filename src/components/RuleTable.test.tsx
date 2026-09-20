import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { sampleRows } from "../test/fixtures";
import { RuleTable } from "./RuleTable";

/**
 * These lean on the states that would mislead someone about their backups:
 * a run that happened but failed, a destination that looks present but is the
 * wrong disk, and a rule that cannot run being offered as if it could.
 */

describe("RuleTable", () => {
  it("renders every column from the reference mock-up, plus Last Result", () => {
    render(<RuleTable rows={sampleRows()} />);

    for (const header of [
      "Tags",
      "Rule",
      "Source",
      "Destination",
      "Status",
      "Type",
      "Compression",
      "Frequency",
      "Last Result",
      "Last Backup",
      "Next Backup",
    ]) {
      expect(
        screen.getByRole("columnheader", { name: new RegExp(header, "i") }),
        `missing column: ${header}`,
      ).toBeInTheDocument();
    }
  });

  it("renders one row per rule", () => {
    render(<RuleTable rows={sampleRows()} />);
    // Six rules plus the header row.
    expect(screen.getAllByRole("row")).toHaveLength(7);
  });

  it("distinguishes a rule that never ran from one that failed", () => {
    render(<RuleTable rows={sampleRows()} />);

    // "Documents - Important" has no runs; "Documents" ran and failed. A date
    // column alone cannot tell these apart, which is why Last Result exists.
    expect(screen.getByText("Never run")).toBeInTheDocument();
    expect(screen.getByText("Failed")).toBeInTheDocument();
    expect(screen.getByText("Partial")).toBeInTheDocument();
  });

  it("names the wrong-drive case distinctly from an unplugged one", () => {
    render(<RuleTable rows={sampleRows()} />);

    // Treating these the same is what would let a backup be written to the
    // wrong disk (docs/PLAN.md §1.1a).
    const mismatch = screen.getByText("Different drive");
    expect(mismatch).toBeInTheDocument();
    expect(mismatch.closest("span")).toHaveAttribute(
      "title",
      expect.stringContaining("not the same drive"),
    );
    expect(screen.getAllByText("Disconnected").length).toBeGreaterThan(0);
  });

  it("marks an attached but refused destination as refused, not available", () => {
    render(<RuleTable rows={sampleRows()} />);
    expect(screen.getByText("Refused")).toBeInTheDocument();
  });

  it("does not convey status by colour alone", () => {
    render(<RuleTable rows={sampleRows()} />);

    // Every status pill must carry a word, so the table survives greyscale
    // and colour blindness.
    for (const label of ["Available", "Disconnected", "Refused", "Different drive"]) {
      expect(screen.getAllByText(label).length).toBeGreaterThan(0);
    }
  });

  it("enables Backup Now only when the rule can actually run", () => {
    render(<RuleTable rows={sampleRows()} />);
    const buttons = screen.getAllByRole("button", { name: "Backup Now" });

    // Phone Photos (available) and Documents (one of two destinations
    // available) can run. The rest cannot.
    const enabled = buttons.filter((b) => !b.hasAttribute("disabled"));
    expect(enabled).toHaveLength(2);
  });

  it("keeps Backup Now disabled for a disabled rule", () => {
    const rows = sampleRows().filter((r) => !r.rule.spec.enabled);
    expect(rows).toHaveLength(1);
    render(<RuleTable rows={rows} />);
    expect(screen.getByRole("button", { name: "Backup Now" })).toBeDisabled();
  });

  it("sorts Last Backup by timestamp, not by its formatted text", async () => {
    const user = userEvent.setup();
    render(<RuleTable rows={sampleRows()} />);

    const timestamps = () =>
      screen
        .getAllByRole("row")
        .slice(1)
        .map((row) => within(row).getAllByRole("cell")[9]?.textContent ?? "")
        .filter((text) => text !== "—")
        .map((text) => Date.parse(text));

    // Most recent first: the useful default for a "when did this last run"
    // column.
    await user.click(screen.getByRole("button", { name: /Last Backup/i }));
    const descending = timestamps();
    expect(descending).toEqual([...descending].sort((a, b) => b - a));

    await user.click(screen.getByRole("button", { name: /Last Backup/i }));
    const ascending = timestamps();
    expect(ascending).toEqual([...ascending].sort((a, b) => a - b));

    // The real point: "1 Sep" must not sort after "14 Sep", which is what
    // ordering the rendered strings would do.
    expect(new Set(ascending).size).toBeGreaterThan(1);
  });

  it("marks the sorted column for assistive technology", async () => {
    const user = userEvent.setup();
    render(<RuleTable rows={sampleRows()} />);

    const header = screen.getByRole("columnheader", { name: /Rule/i });
    expect(header).not.toHaveAttribute("aria-sort");

    await user.click(within(header).getByRole("button"));
    expect(header).toHaveAttribute("aria-sort", "ascending");
  });

  it("filters to rules carrying every selected tag", () => {
    const rows = sampleRows();
    const filesTag = rows[3]?.tags[0];
    expect(filesTag).toBeDefined();

    render(<RuleTable rows={rows} tagFilter={[filesTag?.id ?? 0]} />);
    // "Documents" and "Documents - Important" both carry `files`.
    expect(screen.getAllByRole("row")).toHaveLength(3);
  });

  it("explains an empty table rather than showing a bare grid", () => {
    render(<RuleTable rows={[]} />);
    expect(screen.getByText(/No backup rules yet/i)).toBeInTheDocument();
  });

  it("says when a filter, not the absence of rules, emptied the table", () => {
    render(<RuleTable rows={sampleRows()} tagFilter={[999999]} />);
    expect(screen.getByText(/No rules match the selected tags/i)).toBeInTheDocument();
  });

  it("shows an em dash for Next Backup until the scheduler exists", () => {
    render(<RuleTable rows={sampleRows()} />);
    const firstRow = screen.getAllByRole("row")[1];
    expect(firstRow).toBeDefined();
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    const cells = within(firstRow!).getAllByRole("cell");
    expect(cells[10]).toHaveTextContent("—");
  });
});
