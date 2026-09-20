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
