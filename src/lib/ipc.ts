/**
 * Typed access to the Rust core.
 *
 * Every backend call goes through here. The wrappers exist so that a command
 * rename or a changed payload is a TypeScript error rather than a runtime
 * `undefined`: the argument and return types come from `src/types/`, which is
 * generated from the Rust and checked by CI for drift.
 *
 * Note what these signatures take. Commands accept **ids**, never paths — the
 * core resolves paths from the database, so nothing here can point a backup at
 * a location the user did not choose through the native folder picker
 * (docs/PLAN.md §4, T1).
 */

import { invoke } from "@tauri-apps/api/core";

import type {
  CoreError,
  DestinationId,
  PickedFolder,
  Rule,
  RuleId,
  RuleProblem,
  RuleRow,
  RuleSpec,
  Run,
  Tag,
  TagId,
  VolumePath,
  VolumeStatus,
} from "../types";

/** A `CoreError` that has been through the IPC boundary. */
export class ShelvError extends Error {
  /** Which kind of failure this was, for branching in the UI. */
  readonly kind: CoreError["kind"];

  constructor(error: CoreError) {
    super(error.message);
    this.name = "ShelvError";
    this.kind = error.kind;
  }
}

/** Whether an unknown value has the shape of a `CoreError`. */
function isCoreError(value: unknown): value is CoreError {
  return (
    typeof value === "object" &&
    value !== null &&
    "kind" in value &&
    "message" in value &&
    typeof (value as { message: unknown }).message === "string"
  );
}

/**
 * Calls a command, converting a structured core error into `ShelvError`.
 *
 * Anything that is not a recognisable `CoreError` is rethrown untouched rather
 * than flattened to a string — losing a stack trace makes an unexpected
 * failure much harder to diagnose than it needs to be.
 */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (error: unknown) {
    if (isCoreError(error)) throw new ShelvError(error);
    throw error;
  }
}

/**
 * Calls a command that returns nothing.
 *
 * A Rust command returning `()` serialises as `null`, so the call is typed
 * against that and the `null` discarded — `call<void>` would be a lie about
 * the wire format.
 */
async function callVoid(cmd: string, args?: Record<string, unknown>): Promise<void> {
  await call<null>(cmd, args);
}

/** The running application version. */
export const appVersion = (): Promise<string> => call<string>("app_version");

/** Every rule, with tags, destinations, availability and last result. */
export const listRules = (): Promise<RuleRow[]> => call<RuleRow[]>("list_rules");

/** One rule, in the same shape as a table row. */
export const getRule = (id: RuleId): Promise<RuleRow> =>
  call<RuleRow>("get_rule", { id });

/** A rule's raw configuration, for the editor. */
export const getRuleSpec = (id: RuleId): Promise<Rule> =>
  call<Rule>("get_rule_spec", { id });

/**
 * Opens the native folder dialog.
 *
 * The only way a location enters Shelv: the dialog is the operating system's
 * consent step, and what comes back is a volume id plus a relative path,
 * never something this side could have invented. Resolves to `null` if the
 * user cancelled, and rejects with a `refused` error if the drive is one
 * Shelv will not use.
 */
export const pickFolder = (): Promise<PickedFolder | null> =>
  call<PickedFolder | null>("pick_folder");

/** Checks a rule without saving it, so the editor can show problems early. */
export const validateRule = (
  spec: RuleSpec,
  destinations: VolumePath[],
): Promise<RuleProblem[]> => call<RuleProblem[]>("validate_rule", { spec, destinations });

/** Creates a rule with its destinations and tags. */
export const createRule = (
  spec: RuleSpec,
  destinations: VolumePath[],
  tags: TagId[],
): Promise<RuleId> => call<RuleId>("create_rule", { spec, destinations, tags });

/** Replaces a rule's configuration, destinations and tags. */
export const updateRule = (
  id: RuleId,
  spec: RuleSpec,
  destinations: VolumePath[],
  tags: TagId[],
): Promise<void> => callVoid("update_rule", { id, spec, destinations, tags });

/**
 * Parses the problem list out of a refusal from create or update.
 *
 * The backend validates again on save, because a rule is a standing
 * instruction and the editor's copy of the rules could be stale. It reports
 * the failures as JSON in the error message; this turns them back into
 * something renderable, and yields an empty list for any other refusal.
 */
export function problemsFrom(error: unknown): RuleProblem[] {
  if (!(error instanceof ShelvError) || error.kind !== "refused") return [];
  try {
    const parsed: unknown = JSON.parse(error.message);
    return Array.isArray(parsed) ? (parsed as RuleProblem[]) : [];
  } catch {
    return [];
  }
}

/**
 * Deletes a rule.
 *
 * Removes Shelv's record of the backup only; files already backed up are left
 * alone. The UI should still confirm, because the rule's history goes with it.
 */
export const deleteRule = (id: RuleId): Promise<void> => callVoid("delete_rule", { id });

/** Adds a destination to a rule. */
export const addDestination = (
  rule: RuleId,
  path: VolumePath,
  sortOrder: number,
): Promise<DestinationId> =>
  call<DestinationId>("add_destination", { rule, path, sortOrder });

/** Removes a destination from a rule. */
export const deleteDestination = (id: DestinationId): Promise<void> =>
  callVoid("delete_destination", { id });

/** Every tag. */
export const listTags = (): Promise<Tag[]> => call<Tag[]>("list_tags");

/**
 * Creates a tag.
 *
 * `colour` is a palette token such as `pastel-blue`, not a raw colour, so the
 * theme can keep every tag readable against the dark background.
 */
export const createTag = (name: string, colour: string): Promise<TagId> =>
  call<TagId>("create_tag", { name, colour });

/** Deletes a tag and unlinks it from every rule. */
export const deleteTag = (id: TagId): Promise<void> => callVoid("delete_tag", { id });

/** Replaces the set of tags on a rule. */
export const setRuleTags = (rule: RuleId, tags: TagId[]): Promise<void> =>
  callVoid("set_rule_tags", { rule, tags });

/** Every known volume, and whether it can be written to right now. */
export const listVolumes = (): Promise<VolumeStatus[]> =>
  call<VolumeStatus[]>("list_volumes");

/** A rule's run history, newest first. */
export const listRuns = (rule: RuleId, limit: number): Promise<Run[]> =>
  call<Run[]>("list_runs", { rule, limit });
