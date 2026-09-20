import { describe, expect, it } from "vitest";

import { formatLocation, volumeName } from "./volumes";
import type { VolumeStatus } from "../types";

function status(over: Partial<VolumeStatus["volume"]> & { mount?: string | null }) {
  const { mount, ...volume } = over;
  return {
    volume: {
      id: 1,
      identity: { kind: "linux_fs_uuid" as const, value: "u" },
      serial: null,
      label: null,
      filesystem: "ext4",
      drive_type: "removable" as const,
      is_sync_root: false,
      last_seen_at: null,
      last_mount: null,
      ...volume,
    },
    availability: "available" as const,
    mount_point: mount ?? null,
  } satisfies VolumeStatus;
}

describe("volumeName", () => {
  it("prefers the filesystem label", () => {
    expect(volumeName(status({ label: "Backup Drive", mount: "/media/backup" }))).toBe(
      "Backup Drive",
    );
  });

  it("falls back to where the drive is mounted", () => {
    // Plenty of drives have no label — a freshly formatted one usually does
    // not — and a bare "?" tells the reader nothing about which disk a rule
    // points at.
    expect(volumeName(status({ mount: "/media/backup" }))).toBe("/media/backup");
  });

  it("falls back to where it was last seen when it is unplugged", () => {
    expect(volumeName(status({ last_mount: "E:\\", mount: null }))).toBe("E:\\");
  });

  it("never renders an empty name", () => {
    expect(volumeName(status({}))).toBe("Unnamed drive");
    // An empty string is as useless as null and must not slip through.
    expect(volumeName(status({ label: "", mount: "" }))).toBe("Unnamed drive");
  });
});

describe("formatLocation", () => {
  it("joins the drive and the relative path", () => {
    expect(formatLocation(status({ label: "Archive" }), "Backups/Photos")).toBe(
      "Archive · Backups/Photos",
    );
  });

  it("shows only the drive when the location is its root", () => {
    expect(formatLocation(status({ label: "Archive" }), "")).toBe("Archive");
  });
});
