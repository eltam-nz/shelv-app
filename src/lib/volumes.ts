import type { VolumeStatus } from "../types";

/**
 * Naming a drive for display.
 *
 * Shared between the table and the editor so a rule reads the same in both.
 */

/**
 * The best available name for a drive.
 *
 * Plenty of drives have no filesystem label — a freshly formatted one usually
 * does not, and Linux only reports one when udev has published it. Falling
 * back to where the drive is, or was, mounted is far more use than a bare
 * "?", which tells the reader nothing about which disk a rule points at.
 */
export function volumeName(status: VolumeStatus): string {
  const label = status.volume.label;
  if (label !== null && label !== "") return label;
  if (status.mount_point !== null && status.mount_point !== "") {
    return status.mount_point;
  }
  const last = status.volume.last_mount;
  if (last !== null && last !== "") return last;
  return "Unnamed drive";
}

/** A stored location, as `Drive · relative/path`. */
export function formatLocation(status: VolumeStatus, relative: string): string {
  const name = volumeName(status);
  // A location at the volume root has no relative part to show.
  return relative === "" ? name : `${name} · ${relative}`;
}

/**
 * Whether this is a drive the user plugs in and unplugs.
 *
 * Only these get a drive-name line of their own in the table: on an internal
 * disk the path already says everything — `C:\Users\...` is unambiguous —
 * whereas on a removable one the same path can belong to any of several
 * drives, and which one it is is the whole question.
 */
export function isExternal(status: VolumeStatus): boolean {
  return status.volume.drive_type === "removable";
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
