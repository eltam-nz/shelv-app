import type { VolumeStatus } from "../types";

/**
 * Naming a drive for display.
 *
 * Shared between the table and the editor so a rule reads the same in both.
 */

/** The parts of a drive's identity that can contribute to its name. */
export interface DriveNameParts {
  /** The filesystem label, if the drive has one. */
  label: string | null;
  /** The volume serial, where the platform reports one. */
  serial?: string | null;
  /** Where the drive is mounted right now. */
  mount: string | null;
  /** Where it was last seen mounted. */
  lastMount?: string | null;
}

/**
 * The best available name for a drive.
 *
 * Plenty of drives have no filesystem label — a freshly formatted one usually
 * does not, and Linux only reports one when udev has published it.
 *
 * The serial comes before the mount point in the fallback chain, even though
 * it is uglier, because it identifies the drive and the mount point does not.
 * Two unlabelled drives that have taken turns in the same USB port would
 * otherwise both be called `E:\\`, which is worse than unhelpful: it says
 * they are the same drive.
 */
export function driveName(parts: DriveNameParts): string {
  const { label, serial, mount, lastMount } = parts;
  if (label !== null && label !== "") return label;
  if (serial !== null && serial !== undefined && serial !== "") {
    return `Unnamed drive (${serial})`;
  }
  if (mount !== null && mount !== "") return mount;
  if (lastMount !== null && lastMount !== undefined && lastMount !== "") {
    return lastMount;
  }
  return "Unnamed drive";
}

/** The best available name for a recorded drive. */
export function volumeName(status: VolumeStatus): string {
  return driveName({
    label: status.volume.label,
    serial: status.volume.serial,
    mount: status.mount_point,
    lastMount: status.volume.last_mount,
  });
}

/** A stored location, as `Drive · relative/path`. */
export function formatLocation(status: VolumeStatus, relative: string): string {
  const name = volumeName(status);
  // A location at the volume root has no relative part to show.
  return relative === "" ? name : `${name} · ${relative}`;
}

/**
 * The drive's mount point, live if it is attached and otherwise the last one
 * it was seen at.
 *
 * The fallback is labelled as such by the caller rather than silently — a
 * path for an unplugged drive is where it *was*, which is useful to see but
 * must not read as where it is.
 */
export function mountPoint(status: VolumeStatus): string | null {
  const live = status.mount_point;
  if (live !== null && live !== "") return live;
  const last = status.volume.last_mount;
  return last !== null && last !== "" ? last : null;
}

/**
 * Joins a mount point and a relative path the way its own platform writes it.
 *
 * The separator comes from the mount point rather than from the host running
 * this code: the same database is read by a Windows build showing `E:\` and
 * would be read by a Linux build showing `/media/backup`, and rendering
 * `E:\/Backups` or `/media/backup\Backups` at someone is worse than showing
 * no path at all, because it looks like a path they could type.
 */
export function joinPath(mount: string, relative: string): string {
  if (relative === "") return mount;
  const separator = mount.includes("\\") ? "\\" : "/";
  const base = mount.endsWith(separator) ? mount.slice(0, -1) : mount;
  return `${base}${separator}${relative}`;
}

/**
 * The full path of a stored location, as the user would type it.
 *
 * `null` when the drive has never been seen at a mount point, which leaves
 * the caller to fall back to the drive-relative form.
 */
export function fullPath(status: VolumeStatus, relative: string): string | null {
  const mount = mountPoint(status);
  return mount === null ? null : joinPath(mount, relative);
}
