/**
 * A backup location, shown as the drive it is on and the path within it.
 *
 * The drive leads because the drive is what the location actually *is*. A
 * destination is stored as a volume identity plus a path relative to that
 * volume, and the drive letter is not part of it — Windows hands letters out
 * to whichever disk was plugged in first, so `E:\` names a different drive
 * from one week to the next. Leading with the path would put the least
 * stable thing first and bury the one that stays true.
 *
 * The name is shown for **every** drive, not just ones that look removable.
 * An earlier version gated it on `DriveType::Removable`, which was wrong in
 * the worst possible direction: on Windows `GetDriveType` reports
 * `DRIVE_REMOVABLE` only when the *medium* comes out of the device —
 * floppies, card readers, USB flash sticks. A USB hard disk or SSD in an
 * enclosure has fixed media inside a device you unplug, so it reports
 * `DRIVE_FIXED`. The name was therefore hidden for exactly the drives Shelv
 * exists to manage.
 *
 * There is no corrected heuristic here because none is needed. Shelv's whole
 * premise is destinations that come and go, so every destination is treated
 * as one that might.
 */

/** Where a backup goes, in terms the reader can act on. */
export function DriveLocation({
  name,
  relative,
  mount,
}: {
  /** The drive's name: the user's nickname where they set one. Never empty. */
  name: string;
  /** The path within the drive, from its root. */
  relative: string;
  /** Where the drive is mounted right now, or `null` if it is not attached. */
  mount: string | null;
}) {
  const connected = mount !== null && mount !== "";
  // The letter is worth showing — it is how you would find the drive in
  // Explorer — but only while it means something. A letter printed beside an
  // unplugged drive currently belongs to nothing, or to some other disk.
  const heading = connected ? `${name} (${trimSeparator(mount)})` : name;
  const path = relative === "" ? "The whole drive" : relative;
  const title = connected ? `${name} — ${path}` : `${name} — ${path} (not connected)`;

  return (
    <div className="min-w-0">
      <div className={`truncate ${connected ? "" : "text-fg-muted"}`} title={title}>
        {heading}
      </div>
      <div className="truncate text-[11px] text-fg-muted italic" title={path}>
        {path}
      </div>
    </div>
  );
}

/**
 * `E:\` reads better as `E:` beside a name, and `/media/backup/` as
 * `/media/backup`. A mount point that is only a separator — the Unix root —
 * keeps it, since there is nothing else left of it.
 */
function trimSeparator(mount: string): string {
  const trimmed = mount.replace(/[/\\]$/, "");
  return trimmed === "" ? mount : trimmed;
}
