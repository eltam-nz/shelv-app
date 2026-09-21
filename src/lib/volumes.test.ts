import { describe, expect, it } from "vitest";

import { formatLocation, fullPath, isExternal, joinPath, volumeName } from "./volumes";
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

describe("joinPath", () => {
  it("uses a backslash when the mount point does", () => {
    expect(joinPath("E:\\", "Backups\\Photos")).toBe("E:\\Backups\\Photos");
  });

  it("uses a forward slash when the mount point does", () => {
    expect(joinPath("/media/backup", "Backups")).toBe("/media/backup/Backups");
  });

  it("does not double the separator", () => {
    // `E:\` and `/` already end in one, which is exactly the common case.
    expect(joinPath("E:\\", "Backups")).toBe("E:\\Backups");
    expect(joinPath("/", "srv/data")).toBe("/srv/data");
  });

  it("yields the mount point itself at the volume root", () => {
    expect(joinPath("E:\\", "")).toBe("E:\\");
  });
});

describe("fullPath", () => {
  it("uses where the drive is now", () => {
    expect(fullPath(status({ mount: "E:\\" }), "Backups")).toBe("E:\\Backups");
  });

  it("falls back to where it was last seen when unplugged", () => {
    // Where the backup went is still worth showing for a drive in a drawer.
    expect(fullPath(status({ mount: null, last_mount: "E:\\" }), "Backups")).toBe(
      "E:\\Backups",
    );
  });

  it("is null when the drive has never been seen anywhere", () => {
    expect(fullPath(status({ mount: null }), "Backups")).toBeNull();
  });
});

describe("isExternal", () => {
  it("is true only for a drive that gets unplugged", () => {
    expect(isExternal(status({ drive_type: "removable" }))).toBe(true);
    expect(isExternal(status({ drive_type: "fixed" }))).toBe(false);
  });
});
