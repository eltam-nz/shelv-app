import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { sampleRows } from "../test/fixtures";
import { RuleEditor } from "./RuleEditor";

// The editor validates against the backend as fields change. There is no
// backend in a unit test, so the IPC module is stubbed; what is under test
// here is the copy the editor puts in front of someone about to save a
// standing instruction that can delete files.
const picked = {
  path: { volume: 99, relative: "Backups\\Photos" },
  volume_label: "Archive 4TB",
  volume_serial: "1A2B3C4D",
  mount_point: "E:\\",
  display_path: "E:\\Backups\\Photos",
};

vi.mock("../lib/ipc", () => ({
  createRule: vi.fn(),
  pickFolder: vi.fn(),
  problemsFrom: () => [],
  ShelvError: class extends Error {},
  updateRule: vi.fn(),
  validateRule: vi.fn(() => Promise.resolve([])),
}));

/** The editor needs both callbacks; neither is what these tests are about. */
function noop(): void {
  // Intentionally empty.
}

function editFirstRule() {
  const row = sampleRows()[0];
  expect(row).toBeDefined();
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  render(<RuleEditor existing={row!} tags={[]} onClose={noop} onSaved={noop} />);
}

describe("RuleEditor", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("no longer asks separately whether deletions are allowed", () => {
    // The question the layout had already answered. A mirror that will not
    // delete is not a mirror, and a snapshot must never delete, so there was
    // never a fourth combination worth offering.
    editFirstRule();
    expect(screen.queryByLabelText(/Delete files from the backup/i)).toBeNull();
    expect(screen.queryByText(/Delete files from the backup/i)).toBeNull();
  });

  it("says a mirror will remove files, before the rule is saved", async () => {
    // "Mirror" quietly includes removals. Someone who has not thought that
    // through deserves to be told now rather than after the first run.
    const user = userEvent.setup();
    editFirstRule();

    const type = screen.getByLabelText(/^Backup type$/i);
    await user.selectOptions(type, "mirror");

    expect(
      screen.getByText(/files you delete from the source are removed/i),
    ).toBeInTheDocument();
    // And that it is recoverable, which is the part that decides whether
    // choosing mirror is frightening or fine.
    expect(screen.getByText(/recycle bin rather than erased/i)).toBeInTheDocument();
  });

  it("says a snapshot will never modify what it has already written", async () => {
    const user = userEvent.setup();
    editFirstRule();

    const type = screen.getByLabelText(/^Backup type$/i);
    await user.selectOptions(type, "snapshot");

    expect(
      screen.getByText(/Nothing is deleted from a previous snapshot/i),
    ).toBeInTheDocument();
  });
});

describe("RuleEditor destinations", () => {
  it("names the drive of a destination as soon as it is picked", async () => {
    // A just-picked destination used to render the bare absolute path while
    // a saved one rendered `Drive · relative`, so the same folder read
    // differently depending on how it got here — and the drive, the part
    // that matters for a disk that comes and goes, was missing from exactly
    // the moment the user was choosing it.
    const { pickFolder } = await import("../lib/ipc");
    vi.mocked(pickFolder).mockResolvedValue(picked);

    const user = userEvent.setup();
    editFirstRule();
    await user.click(screen.getByText("Add destination…"));

    expect(await screen.findByText("E:\\Backups\\Photos")).toBeInTheDocument();
    expect(screen.getAllByText("Archive 4TB").length).toBeGreaterThan(0);
  });

  it("falls back to the serial when the picked drive has no label", async () => {
    const { pickFolder } = await import("../lib/ipc");
    vi.mocked(pickFolder).mockResolvedValue({ ...picked, volume_label: null });

    const user = userEvent.setup();
    editFirstRule();
    await user.click(screen.getByText("Add destination…"));

    expect(await screen.findByText("Unnamed drive (1A2B3C4D)")).toBeInTheDocument();
  });
});
