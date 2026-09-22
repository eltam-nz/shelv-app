import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { RunPreview } from "./RunPreview";
import type { DestinationPlan, RuleRow } from "../types";

function row(): RuleRow {
  return {
    rule: {
      id: 1,
      created_at: 0,
      spec: {
        name: "Lightroom",
        enabled: true,
        source: { volume: 1, relative: "Pictures" },
        layout: "mirror",
        packaging: "files",
        retention: { kind: "unlimited" },
        schedule: { kind: "manual" },
        run_on_connect: false,
        catch_up: false,
        placeholders: "hydrate",
        hydrate_budget_bytes: null,
        follow_symlinks: false,
        excludes: [],
      },
    },
    tags: [],
    source: { availability: "available", mount_point: "/src", volume: null },
    destinations: [],
    last_run: null,
  } as unknown as RuleRow;
}

function plans(deletions: string[]): DestinationPlan[] {
  return [
    {
      destination: 1,
      outcome: {
        kind: "ready",
        plan: {
          copies: [{ relative: "a.dng", bytes: 2048, reason: "added" }],
          deletions: deletions.map((relative) => ({ relative, bytes: 1024 })),
          directories: [],
          skipped: [],
          bytes: 2048,
        },
      },
    },
  ];
}

describe("RunPreview", () => {
  it("says how much would be copied and how much would be removed", () => {
    render(
      <RunPreview
        row={row()}
        plans={plans(["old/one.dng", "old/two.dng"])}
        onConfirm={() => undefined}
        onCancel={() => undefined}
      />,
    );

    expect(screen.getByText("1 file · 2.0 KB")).toBeInTheDocument();
    expect(screen.getByText("2 files · 2.0 KB")).toBeInTheDocument();
  });

  it("names the files that would go, rather than only counting them", () => {
    // A count alone cannot be checked against what someone expected. The
    // paths are the only way to notice a rule aimed at the wrong folder
    // before it removes anything.
    render(
      <RunPreview
        row={row()}
        plans={plans(["old/one.dng"])}
        onConfirm={() => undefined}
        onCancel={() => undefined}
      />,
    );

    expect(screen.getByText("old/one.dng")).toBeInTheDocument();
  });

  it("says removals are recoverable, and where from", () => {
    render(
      <RunPreview
        row={row()}
        plans={plans(["old/one.dng"])}
        onConfirm={() => undefined}
        onCancel={() => undefined}
      />,
    );

    expect(screen.getByText(/\.shelv-trash/)).toBeInTheDocument();
    expect(screen.getByText(/Nothing is deleted outright/)).toBeInTheDocument();
  });

  it("runs only when the run is confirmed", async () => {
    const user = userEvent.setup();
    let confirmed = 0;
    let cancelled = 0;
    render(
      <RunPreview
        row={row()}
        plans={plans(["old/one.dng"])}
        onConfirm={() => (confirmed += 1)}
        onCancel={() => (cancelled += 1)}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(confirmed).toBe(0);
    expect(cancelled).toBe(1);

    await user.click(screen.getByRole("button", { name: "Back up now" }));
    expect(confirmed).toBe(1);
  });

  it("mentions destinations that will be left alone", () => {
    const withAbsent: DestinationPlan[] = [
      ...plans(["old/one.dng"]),
      {
        destination: 2,
        outcome: { kind: "unavailable", reason: "the drive is not attached" },
      },
    ];
    render(
      <RunPreview
        row={row()}
        plans={withAbsent}
        onConfirm={() => undefined}
        onCancel={() => undefined}
      />,
    );

    expect(screen.getByText(/1 destination is not/)).toBeInTheDocument();
  });
});
