/**
 * Table fixtures, modelled on the rules in the reference mock-up.
 *
 * Deliberately includes the awkward cases: an unplugged destination, a
 * destination that is attached but refused, a drive that has been replaced by
 * a different one, a rule that has never run, and one with two destinations
 * where only one is reachable.
 */

import type {
  Availability,
  DestinationStatus,
  RuleRow,
  RunResult,
  StoredVolume,
  VolumeStatus,
} from "../types";

let nextId = 1;
const id = () => nextId++;

/**
 * Tags are shared rows, so the same name must carry the same id across rules
 * — otherwise a filter on "files" would match only the rule it came from.
 */
const tagIds = new Map<string, number>();
function tagId(name: string): number {
  const existing = tagIds.get(name);
  if (existing !== undefined) return existing;
  const fresh = id();
  tagIds.set(name, fresh);
  return fresh;
}

function volume(label: string, value: string): StoredVolume {
  return {
    id: id(),
    identity: { kind: "windows_volume_guid", value },
    serial: "A1B2C3D4",
    label,
    nickname: null,
    filesystem: "NTFS",
    drive_type: "removable",
    is_sync_root: false,
    last_seen_at: 1_757_000_000,
    last_mount: "E:\\",
  };
}

function status(label: string, availability: Availability): VolumeStatus {
  return {
    volume: volume(label, `vol-${label}`),
    availability,
    mount_point: availability === "available" ? "E:\\" : null,
  };
}

function destination(
  rule: number,
  label: string,
  relative: string,
  availability: Availability,
): DestinationStatus {
  return {
    destination: { id: id(), rule, path: { volume: id(), relative }, sort_order: 0 },
    status: status(label, availability),
  };
}

interface RuleOptions {
  name: string;
  tags: { name: string; colour: string }[];
  sourceRelative: string;
  sourceAvailability?: Availability;
  destinations: DestinationStatus[];
  layout: RuleRow["rule"]["spec"]["layout"];
  packaging: RuleRow["rule"]["spec"]["packaging"];
  schedule: RuleRow["rule"]["spec"]["schedule"];
  lastResult?: RunResult | null;
  lastRunAt?: number | null;
  enabled?: boolean;
}

export function makeRule(options: RuleOptions): RuleRow {
  const ruleId = id();
  const lastRunAt = options.lastRunAt ?? null;
  return {
    rule: {
      id: ruleId,
      created_at: 1_750_000_000,
      spec: {
        name: options.name,
        enabled: options.enabled ?? true,
        source: { volume: id(), relative: options.sourceRelative },
        layout: options.layout,
        packaging: options.packaging,
        retention: { kind: "keep_last_n", value: 6 },
        schedule: options.schedule,
        run_on_connect: true,
        placeholders: "hydrate",
        hydrate_budget_bytes: null,
        follow_symlinks: false,
        excludes: [],
      },
    },
    tags: options.tags.map((t) => ({
      id: tagId(t.name),
      name: t.name,
      colour: t.colour,
    })),
    destinations: options.destinations,
    source: status("System", options.sourceAvailability ?? "available"),
    last_run:
      lastRunAt === null
        ? null
        : {
            id: id(),
            rule: ruleId,
            destination: id(),
            trigger: "schedule",
            started_at: lastRunAt,
            finished_at: lastRunAt + 120,
            result: options.lastResult ?? "ok",
            stats: {
              files_copied: 1200,
              files_skipped: 0,
              files_deleted: 0,
              bytes_copied: 9_000_000_000,
              compressed_bytes: null,
              placeholders_hydrated: 0,
              placeholders_skipped: 0,
              bytes_hydrated: 0,
              bytes_released: 0,
            },
            error: null,
            snapshot_path: null,
          },
  };
}

/** The mock-up's six rules, plus the states it could not express. */
export function sampleRows(): RuleRow[] {
  nextId = 1;
  tagIds.clear();
  return [
    makeRule({
      name: "Lightroom Catalog",
      tags: [
        { name: "images", colour: "pastel-blue" },
        { name: "Lightroom", colour: "pastel-lilac" },
      ],
      sourceRelative: "Pictures\\Lightroom",
      destinations: [destination(1, "Archive", "Backups\\Lightroom", "disconnected")],
      layout: "snapshot",
      packaging: "files",
      schedule: { kind: "monthly" },
      lastResult: "ok",
      lastRunAt: 1_756_684_800,
    }),
    makeRule({
      name: "Lightroom Exports",
      tags: [
        { name: "images", colour: "pastel-blue" },
        { name: "Lightroom", colour: "pastel-lilac" },
      ],
      sourceRelative: "Pictures\\Exports",
      destinations: [destination(2, "Archive", "Backups\\Exports", "disconnected")],
      layout: "mirror",
      packaging: "files",
      schedule: { kind: "weekly" },
      lastResult: "partial",
      lastRunAt: 1_757_808_000,
    }),
    makeRule({
      name: "Phone Photos",
      tags: [
        { name: "images", colour: "pastel-blue" },
        { name: "Phone", colour: "pastel-teal" },
      ],
      sourceRelative: "Pictures\\Phone",
      destinations: [destination(3, "Photos", "Phone", "available")],
      layout: "mirror",
      packaging: "files",
      schedule: { kind: "monthly" },
      lastResult: "ok",
      lastRunAt: 1_756_684_800,
    }),
    makeRule({
      name: "Documents",
      tags: [{ name: "files", colour: "pastel-sage" }],
      sourceRelative: "Documents",
      // Two destinations, one reachable: the rule can still run.
      destinations: [
        destination(4, "Archive", "Backups\\Documents", "disconnected"),
        destination(4, "Photos", "Documents", "available"),
      ],
      layout: "mirror",
      packaging: "zip_deflate",
      schedule: { kind: "monthly" },
      lastResult: "failed",
      lastRunAt: 1_756_684_800,
    }),
    makeRule({
      name: "Documents - Important",
      tags: [
        { name: "files", colour: "pastel-sage" },
        { name: "Important", colour: "pastel-rose" },
      ],
      sourceRelative: "Documents\\Important",
      // The dangerous case: a drive is mounted there, but not the right one.
      destinations: [
        destination(5, "Archive", "Backups\\Important", "identity_mismatch"),
      ],
      layout: "snapshot",
      packaging: "zip_deflate",
      schedule: { kind: "manual" },
      lastRunAt: null,
    }),
    makeRule({
      name: "Calibre Library",
      tags: [{ name: "books", colour: "pastel-amber" }],
      sourceRelative: "Books\\Calibre",
      // Attached, but a network share: refused, not merely unavailable.
      destinations: [destination(6, "NAS", "Backups\\Calibre", "refused")],
      layout: "mirror",
      packaging: "zip_store",
      schedule: { kind: "weekly" },
      lastResult: "cancelled",
      lastRunAt: 1_757_808_000,
      enabled: false,
    }),
  ];
}
