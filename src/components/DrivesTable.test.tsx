import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { DrivesTable } from "./DrivesTable";
import type { DriveRow } from "../types";

function drive(
  over: Partial<DriveRow["status"]["volume"]> = {},
  rest: Partial<DriveRow> = {},
) {
  return {
    status: {
      volume: {
        id: 1,
        identity: {
          kind: "windows_volume_guid" as const,
          value: "\\\\?\\Volume{9f1c2d3e-4a5b-6c7d-8e9f-0a1b2c3d4e5f}\\",
        },
        serial: "1A2B3C4D",
        label: "Expansion",
        nickname: null,
        filesystem: "exFAT",
        drive_type: "removable" as const,
        is_sync_root: false,
        last_seen_at: null,
        last_mount: "E:\\",
        ...over,
      },
      availability: "available" as const,
      mount_point: "E:\\",
    },
    rule_count: 0,
    ...rest,
  } satisfies DriveRow;
}

const noop = () => {
  // Intentionally empty.
};

describe("DrivesTable", () => {
  it("offers the Explorer name as the placeholder a nickname replaces", () => {
    // The field is empty because no nickname is set, but it must not look
    // like the drive has no name at all — what it falls back to is the
    // thing the user would recognise from Explorer.
    render(<DrivesTable rows={[drive()]} onRename={noop} onForget={noop} onAdd={noop} />);
    expect(screen.getByRole("textbox")).toHaveAttribute("placeholder", "Expansion");
  });

  it("commits a nickname on Enter", async () => {
    const user = userEvent.setup();
    const renamed = vi.fn();
    render(
      <DrivesTable rows={[drive()]} onRename={renamed} onForget={noop} onAdd={noop} />,
    );

    await user.type(screen.getByRole("textbox"), "Archive 4TB{Enter}");
    expect(renamed).toHaveBeenCalledWith(1, "Archive 4TB");
  });

  it("reverts on Escape without committing", async () => {
    const user = userEvent.setup();
    const renamed = vi.fn();
    render(
      <DrivesTable
        rows={[drive({ nickname: "Archive 4TB" })]}
        onRename={renamed}
        onForget={noop}
        onAdd={noop}
      />,
    );

    const field = screen.getByRole("textbox");
    await user.clear(field);
    await user.type(field, "Mistake{Escape}");

    expect(renamed).not.toHaveBeenCalled();
    expect(field).toHaveValue("Archive 4TB");
  });

  it("treats clearing the name as meaningful rather than rejecting it", async () => {
    // An empty nickname falls back to the Explorer label, which is a real
    // thing to want, so it commits as null rather than being ignored.
    const user = userEvent.setup();
    const renamed = vi.fn();
    render(
      <DrivesTable
        rows={[drive({ nickname: "Archive 4TB" })]}
        onRename={renamed}
        onForget={noop}
        onAdd={noop}
      />,
    );

    await user.clear(screen.getByRole("textbox"));
    await user.tab();
    expect(renamed).toHaveBeenCalledWith(1, null);
  });

  it("will not forget a drive that rules still point at", async () => {
    // Cascading here would delete backup rules as a side effect of tidying
    // a drive list.
    const user = userEvent.setup();
    const forgotten = vi.fn();
    render(
      <DrivesTable
        rows={[drive({}, { rule_count: 3 })]}
        onRename={noop}
        onForget={forgotten}
        onAdd={noop}
      />,
    );

    const button = screen.getByRole("button", { name: "Forget" });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", expect.stringContaining("3 rules"));
    // And it must look disabled. Both states were the same muted grey, so
    // the only way to find out was to press it.
    expect(button.className).toContain("opacity-40");
    await user.click(button);
    expect(forgotten).not.toHaveBeenCalled();
  });

  it("forgets a drive nothing uses", async () => {
    const user = userEvent.setup();
    const forgotten = vi.fn();
    render(
      <DrivesTable rows={[drive()]} onRename={noop} onForget={forgotten} onAdd={noop} />,
    );

    await user.click(screen.getByRole("button", { name: "Forget" }));
    expect(forgotten).toHaveBeenCalledWith(1);
  });

  it("says a drive is added through the system dialog", () => {
    // The single way a drive enters Shelv, and the button should say so
    // rather than looking like it enrols whatever is plugged in.
    render(<DrivesTable rows={[drive()]} onRename={noop} onForget={noop} onAdd={noop} />);
    expect(screen.getByRole("button", { name: /Add drive/ })).toHaveAttribute(
      "title",
      expect.stringContaining("Choose the drive, or any folder on it"),
    );
  });

  it("says where an attached drive is, inside its status", () => {
    // The letter qualifies "this drive is here right now". In a column of
    // its own it reads as part of the drive's identity, which is the one
    // thing it is not.
    render(<DrivesTable rows={[drive()]} onRename={noop} onForget={noop} onAdd={noop} />);
    expect(screen.getByText("Available — E:\\")).toBeInTheDocument();
    expect(
      screen.queryByRole("columnheader", { name: /^Drive$/ }),
    ).not.toBeInTheDocument();
  });

  it("shows no location for a drive that is not attached", () => {
    const detached = drive();
    detached.status.availability = "disconnected";
    detached.status.mount_point = null;

    render(
      <DrivesTable rows={[detached]} onRename={noop} onForget={noop} onAdd={noop} />,
    );
    expect(screen.getByText("Disconnected")).toBeInTheDocument();
  });

  it("shows only the GUID of a Windows identity", () => {
    render(<DrivesTable rows={[drive()]} onRename={noop} onForget={noop} onAdd={noop} />);
    const shown = screen.getByText("{9f1c2d3e-4a5b-6c7d-8e9f-0a1b2c3d4e5f}");
    expect(shown).toBeInTheDocument();
    // The stored value is never rewritten — it is what recognises the drive
    // across reconnections — so the whole of it stays on the title.
    expect(shown).toHaveAttribute("title", expect.stringContaining("Volume{"));
  });

  it("explains an empty pane rather than showing a bare grid", () => {
    render(<DrivesTable rows={[]} onRename={noop} onForget={noop} onAdd={noop} />);
    expect(screen.getByText(/No drives yet/i)).toBeInTheDocument();
  });
});
