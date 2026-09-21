import { describe, expect, it } from "vitest";

import { driveName, formatLocation, fullPath, joinPath, volumeName } from "./volumes";
import type { VolumeStatus } from "../types";

function status(over: Partial<VolumeStatus["volume"]> & { mount?: string | null }) {
  const { mount, ...volume } = over;
  return {
    volume: {
      id: 1,
      identity: { kind: "linux_fs_uuid" as const, value: "u" },
      serial: null,
      label: null,
      nickname: null,
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

  it("falls back to where the drive is mounted when nothing else names it", () => {
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

describe("driveName", () => {
  it("prefers the serial over the mount point for an unlabelled drive", () => {
    // Two unlabelled drives that have taken turns in the same port would
    // both be called "E:\\", which does not merely fail to help — it says
    // they are the same drive.
    expect(driveName({ label: null, serial: "1A2B3C4D", mount: "E:\\" })).toBe(
      "Unnamed drive 1A2B3C4D",
    );
  });

  it("falls back to the mount point when there is no serial either", () => {
    expect(driveName({ label: null, serial: null, mount: "/media/backup" })).toBe(
      "/media/backup",
    );
  });
});

describe("driveName precedence", () => {
  it("puts the name the user chose above everything the system reports", () => {
    // It is the only one of these they chose, the only one that stays put
    // when the drive is relabelled or comes back at another letter, and the
    // only one that can tell two identical drives apart.
    expect(
      driveName({
        nickname: "Archive 4TB",
        label: "Expansion",
        serial: "1A2B3C4D",
        mount: "E:\\",
      }),
    ).toBe("Archive 4TB");
  });

  it("falls back to the Explorer label when no nickname is set", () => {
    expect(driveName({ nickname: null, label: "Expansion", mount: "E:\\" })).toBe(
      "Expansion",
    );
  });
});
