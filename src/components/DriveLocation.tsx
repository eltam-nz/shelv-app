/**
 * A backup location, shown as its path with the drive's name beneath it.
 *
 * The name is shown for **every** drive, not just ones that look removable.
 * An earlier version gated it on `DriveType::Removable`, which was wrong in
 * the worst possible direction: on Windows `GetDriveType` reports
 * `DRIVE_REMOVABLE` only when the *medium* comes out of the device — floppies,
 * card readers, USB flash sticks. A USB hard disk or SSD in an enclosure has
 * fixed media inside a device you unplug, so it reports `DRIVE_FIXED`. The
 * name was therefore hidden for exactly the drives Shelv exists to manage.
 *
 * There is no corrected heuristic here because none is needed. Shelv's whole
 * premise is destinations that come and go, so every destination is treated
 * as one that might: the drive is always named, and no code has to guess
 * which kind of disk it is looking at.
 */

/** Where a backup goes, in terms the reader can act on. */
export function DriveLocation({
  name,
  path,
  connected,
}: {
  /** The drive's name. Never empty — see `driveName`. */
  name: string;
  /** The location's path. The full path when known, otherwise the part of it
   *  relative to the drive. */
  path: string;
  /** Whether the drive is attached right now. */
  connected: boolean;
}) {
  // A path for a drive that is not plugged in is where the backup *was*, and
  // the drive letter in it currently belongs to nothing, or to some other
  // disk. Muting it is what stops it reading as somewhere you could open.
  const title = connected
    ? `${path} — ${name}`
    : `${path} — ${name} (not connected; this is where it was last seen)`;

  return (
    <div className="min-w-0">
      <div className={`truncate ${connected ? "" : "text-fg-muted"}`} title={title}>
        {path}
      </div>
      <div className="truncate text-[11px] text-fg-muted italic" title={name}>
        {name}
      </div>
    </div>
  );
}
