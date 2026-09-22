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

/**
 * The position of a column, looked up by its heading.
 *
 * Cells are addressed through this rather than by a hard-coded index: the
 * simple and detailed views show different columns, so an index that is
 * right in one is quietly pointing at the wrong data in the other.
 */
function columnIndex(name: RegExp): number {
  const headers = screen.getAllByRole("columnheader");
  const index = headers.findIndex((h) => name.test(h.textContent));
  expect(index, `no column matching ${String(name)}`).toBeGreaterThanOrEqual(0);
  return index;
}

/** The text of one cell of one body row. */
function cellText(rowIndex: number, column: RegExp): string {
  const row = screen.getAllByRole("row")[rowIndex + 1];
  expect(row).toBeDefined();
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  return within(row!).getAllByRole("cell")[columnIndex(column)]?.textContent ?? "";
}

describe("RuleTable", () => {
  it("renders every column from the reference mock-up, plus Last Result", () => {
    render(<RuleTable rows={sampleRows()} view="detailed" />);

    for (const header of [
      "Tags",
      "Rule",
      "Source",
      "Destination",
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

  it("hides the configuration columns in the simple view", () => {
    // Type, compression and frequency are set once and then forgotten, so
    // in the everyday view they are width spent on nothing.
    render(<RuleTable rows={sampleRows()} view="simple" />);

    for (const hidden of ["Type", "Compression", "Frequency"]) {
      expect(
        screen.queryByRole("columnheader", { name: new RegExp(`^${hidden}$`, "i") }),
        `${hidden} should be hidden`,
      ).not.toBeInTheDocument();
    }

    // What the simple view keeps is everything that answers "is my data safe
    // right now?".
    for (const kept of ["Destination", "Last Result"]) {
      expect(
        screen.getByRole("columnheader", { name: new RegExp(kept, "i") }),
      ).toBeInTheDocument();
    }
  });

  it("shows the remaining rule settings in the detailed view", () => {
    render(<RuleTable rows={sampleRows()} view="detailed" />);

    for (const header of ["Cloud", "On connect", "Catch up", "Keep", "Links", "Ignore"]) {
      expect(
        screen.getByRole("columnheader", { name: new RegExp(header, "i") }),
        `missing column: ${header}`,
      ).toBeInTheDocument();
    }

    // And they carry words, not raw enum spellings.
    expect(screen.getAllByText("Download").length).toBeGreaterThan(0);
  });

  it("defaults to the simple view", () => {
    render(<RuleTable rows={sampleRows()} />);
    expect(
      screen.queryByRole("columnheader", { name: /^Compression$/i }),
    ).not.toBeInTheDocument();
  });

  it("leads a destination with its drive and shows the path within it", () => {
    // The drive is what the destination actually is: a volume identity plus
    // a path relative to it. The drive letter is not part of that, so it
    // must not be what the reader sees first.
    render(<RuleTable rows={sampleRows()} />);

    const text = cellText(0, /Destination/i);
    expect(text).toContain("Archive");
    expect(text).toContain("Backups\\Lightroom");
  });

  it("shows the drive letter only while the drive is attached", () => {
    // A letter printed beside an unplugged drive belongs to nothing, or to
    // some other disk.
    const rows = sampleRows();
    render(<RuleTable rows={rows} />);

    // "Phone Photos" has one attached destination and one that is not.
    expect(screen.getAllByText(/^Photos \(E:\)$/).length).toBeGreaterThan(0);
    expect(screen.queryByText(/^Archive \(E:\)$/)).toBeNull();
    expect(screen.getAllByText("Archive").length).toBeGreaterThan(0);
  });

  it("names the drive even when the platform calls it a fixed disk", () => {
    // The bug this replaces a heuristic for. Windows reports DRIVE_REMOVABLE
    // only when the *medium* comes out — flash sticks, card readers. A USB
    // hard disk or SSD in an enclosure has fixed media inside a device you
    // unplug, so it comes back DRIVE_FIXED, and gating the drive name on
    // "removable" hid it for exactly the drives Shelv is for.
    const rows = sampleRows();
    for (const row of rows) {
      for (const d of row.destinations) d.status.volume.drive_type = "fixed";
    }

    render(<RuleTable rows={rows} />);
    expect(screen.getAllByText("Archive").length).toBeGreaterThan(0);
  });

  it("renders the path in italics beneath the drive, not merged into it", () => {
    render(<RuleTable rows={sampleRows()} />);
    const paths = screen.getAllByText("Backups\\Lightroom");
    expect(paths.length).toBeGreaterThan(0);
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    expect(paths[0]!.className).toContain("italic");
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
    // The word is the screen-reader label inside the mark; the explanation
    // is the mark's own title, which is what hovering it shows.
    expect(mismatch.closest("[title]")).toHaveAttribute(
      "title",
      expect.stringContaining("not the same drive"),
    );
    expect(screen.getAllByText("Disconnected").length).toBeGreaterThan(0);
  });

  it("distinguishes an unidentifiable drive from a refused one", () => {
    // Connected and of a permitted type, but the system reports nothing that
    // identifies it across reconnections, so it cannot be told apart from a
    // different drive in the same place. "Refused" would point at the wrong
    // cause and "Available" would be unsafe.
    const rows = sampleRows();
    const target = rows[0];
    expect(target).toBeDefined();
    const destination = target?.destinations[0];
    expect(destination).toBeDefined();
    if (destination) destination.status.availability = "unverifiable";

    render(<RuleTable rows={rows} />);

    const pill = screen.getByText("Unidentified");
    expect(pill).toBeInTheDocument();
    expect(pill.closest("[title]")).toHaveAttribute(
      "title",
      expect.stringContaining("does not report anything that identifies it"),
    );
    expect(screen.getByRole("row", { name: /Lightroom Catalog/ })).toBeInTheDocument();
  });

  it("marks an attached but refused destination as refused, not available", () => {
    render(<RuleTable rows={sampleRows()} />);
    expect(screen.getByText("Refused")).toBeInTheDocument();
  });

  it("does not convey status by colour alone", () => {
    render(<RuleTable rows={sampleRows()} />);

    // The mark beside a drive name is a glyph and a colour on screen, but
    // the word travels with it — so the table survives greyscale, colour
    // blindness and a screen reader.
    for (const label of ["Disconnected", "Refused", "Different drive"]) {
      expect(screen.getAllByText(label).length).toBeGreaterThan(0);
    }
  });

  it("marks a drive's state beside its name rather than in a column", () => {
    // The state belongs next to the thing whose state it is, and the drives
    // pane already spells every one of them out in full.
    render(<RuleTable rows={sampleRows()} />);

    const mark = screen.getAllByText("Disconnected")[0];
    expect(mark).toBeDefined();
    const cell = mark?.closest("td");
    expect(cell).not.toBeNull();
    // The same cell holds the drive's name, which is what "beside" means.
    expect(cell?.textContent).toContain("Disconnected");
    expect(cell?.className).not.toContain("sticky");
  });

  it("marks an unplugged drive as blocking, not as neutral", () => {
    // Grey is what the em dashes and the italic paths around it are. In a
    // table answering "can this back up?", a destination that is not there
    // is the rule not running, and it should not recede into that.
    render(<RuleTable rows={sampleRows()} />);

    const mark = screen.getAllByText("Disconnected")[0]?.closest("[title]");
    expect(mark).not.toBeNull();
    expect(mark?.getAttribute("style")).toContain("--status-mismatch");
  });

  it("shows no mark at all on a destination that is ready", () => {
    // Nearly every row is available. Marking those too would teach the eye
    // to skip exactly the marks that matter.
    render(<RuleTable rows={sampleRows()} />);
    expect(screen.queryByText("Available")).not.toBeInTheDocument();
  });

  it("offers Backup Now only for rules that could actually run", () => {
    // A button that looks operational on a rule whose drive is unplugged
    // invites someone to believe a backup ran. Phone Photos and Documents
    // each have a reachable destination; the other four do not.
    render(<RuleTable rows={sampleRows()} />);
    const buttons = screen.getAllByRole("button", { name: "Backup Now" });
    expect(buttons).toHaveLength(6);
    expect(buttons.filter((b) => !(b as HTMLButtonElement).disabled)).toHaveLength(2);
  });

  it("says why a rule cannot be backed up right now", () => {
    render(<RuleTable rows={sampleRows()} />);
    const titles = screen
      .getAllByRole("button", { name: "Backup Now" })
      .map((b) => b.getAttribute("title") ?? "");

    expect(titles.filter((t) => t.startsWith("Copy this rule's source"))).toHaveLength(2);
    expect(titles.filter((t) => t.startsWith("This rule cannot run"))).toHaveLength(4);
  });

  it("starts a backup for the row whose Backup Now was pressed", async () => {
    const user = userEvent.setup();
    const started: string[] = [];
    render(
      <RuleTable
        rows={sampleRows()}
        onBackUp={(row) => started.push(row.rule.spec.name)}
      />,
    );

    const [first] = screen
      .getAllByRole("button", { name: "Backup Now" })
      .filter((b) => !(b as HTMLButtonElement).disabled);
    if (first === undefined) throw new Error("no rule was offered a backup");
    await user.click(first);

    expect(started).toHaveLength(1);
  });

  it("lights Backup Now under the pointer only where it can run", () => {
    // A button that looks identical under the pointer gives no sign it is a
    // button until it is pressed — and one that lights up on a rule that
    // cannot run says the opposite of the truth.
    const rows = sampleRows();
    render(<RuleTable rows={rows} onBackUp={() => undefined} />);

    for (const button of screen.getAllByRole("button", { name: "Backup Now" })) {
      const hoverable = button.className.includes("btn-soft");
      expect(hoverable).toBe(!(button as HTMLButtonElement).disabled);
    }
  });

  it("refuses every rule while one backup is already running", () => {
    // Shelv runs one at a time so two rules cannot write to one drive. The
    // rule that is running says so in its own words rather than looking the
    // same as the ones merely waiting.
    const rows = sampleRows();
    const [first] = rows;
    if (first === undefined) throw new Error("the fixtures should hold rules");
    render(<RuleTable rows={rows} running={first.rule.id} onBackUp={() => undefined} />);

    expect(screen.getAllByRole("button", { name: "Backing up…" })).toHaveLength(1);
    for (const button of screen.getAllByRole("button", { name: "Backup Now" })) {
      expect(button).toBeDisabled();
    }
  });

  it("opens the editor for the row whose Edit Rule was pressed", async () => {
    const user = userEvent.setup();
    const edited: string[] = [];
    render(
      <RuleTable rows={sampleRows()} onEdit={(row) => edited.push(row.rule.spec.name)} />,
    );

    const buttons = screen.getAllByRole("button", { name: "Edit Rule" });
    const third = buttons[2];
    expect(third).toBeDefined();
    if (third) await user.click(third);

    expect(edited).toEqual(["Phone Photos"]);
  });

  it("offers rule creation from the empty state", async () => {
    const user = userEvent.setup();
    let created = 0;
    render(
      <RuleTable
        rows={[]}
        onCreate={() => {
          created += 1;
        }}
      />,
    );

    await user.click(screen.getByRole("button", { name: "New rule" }));
    expect(created).toBe(1);
  });

  it("sorts Last Backup by timestamp, not by its formatted text", async () => {
    const user = userEvent.setup();
    render(<RuleTable rows={sampleRows()} />);

    const timestamps = () =>
      screen
        .getAllByRole("row")
        .slice(1)
        .map(
          (row) =>
            within(row).getAllByRole("cell")[columnIndex(/Last Backup/i)]?.textContent ??
            "",
        )
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
    expect(cellText(0, /Next Backup/i)).toBe("—");
  });

  it("keeps the actions pinned to the right edge of every row", () => {
    // They have to stay reachable while the table scrolls sideways, and
    // stay beside their own row while doing it. A sticky cell inside the row
    // is aligned by construction; two separate lists would have to have
    // their heights measured and copied, and would drift.
    render(<RuleTable rows={sampleRows()} />);

    for (const button of screen.getAllByRole("button", { name: "Edit Rule" })) {
      const cell = button.closest("td");
      expect(cell).not.toBeNull();
      expect(cell?.className).toContain("sticky");
      expect(cell?.className).toContain("right-0");
      // Opaque, or the columns scrolling past would show through it. It is
      // set apart by sitting on the raised surface rather than by a bright
      // rule down its edge.
      expect(cell?.className).toContain("bg-surface");
      expect(cell?.className).toContain("border-l");
      expect(cell?.className).not.toContain("border-border-strong");
    }
  });
});
