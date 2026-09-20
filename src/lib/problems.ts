import type { RuleProblem } from "../types";

/**
 * Plain-English wording for a validation failure.
 *
 * The backend reports problems structurally so they can be attached to the
 * field that caused them; the wording lives here because it is presentation.
 * Each one says what is wrong *and* what would go wrong if it were allowed —
 * "a destination inside the source" means nothing until you know the backup
 * would swallow its own output.
 */
export function describeProblem(problem: RuleProblem): string {
  switch (problem.kind) {
    case "empty_name":
      return "Give the rule a name.";
    case "no_destinations":
      return "Add at least one place to back this up to.";
    case "source_is_destination":
      return "This is the same folder as the source. Backing a folder up onto itself does nothing, and in mirror mode it would empty it.";
    case "destination_inside_source":
      return "This is inside the source folder, so every run would copy the previous run's backup back into itself and the folder would grow until the disk is full.";
    case "source_inside_destination":
      return "The source folder is inside this one. In mirror mode the source would become a candidate for deletion by its own rule.";
    case "duplicate_destination":
      return "This is already one of the destinations. Writing to it twice in one run would race with itself.";
    case "destination_is_volume_root":
      return "This is the whole drive, not a folder on it. Choose a folder, so a mirror with deletions cannot reach everything else on the drive.";
    case "destination_is_system_path":
      return `This is a system location (${problem.path}). Shelv will not write backups into it.`;
    case "source_volume_unusable":
      return `This drive cannot be used: ${problem.reason}.`;
    case "destination_volume_unusable":
      return `This drive cannot be used: ${problem.reason}.`;
    case "path_escapes_volume":
      return `This path points outside its drive (${problem.path}).`;
  }
}

/** Which destination a problem belongs to, or `null` if it is not about one. */
export function destinationIndex(problem: RuleProblem): number | null {
  return "destination_index" in problem ? problem.destination_index : null;
}

/** Problems that belong to no particular destination. */
export function generalProblems(problems: RuleProblem[]): RuleProblem[] {
  return problems.filter((p) => destinationIndex(p) === null);
}

/** Problems attached to a given destination. */
export function problemsForDestination(
  problems: RuleProblem[],
  index: number,
): RuleProblem[] {
  return problems.filter((p) => destinationIndex(p) === index);
}
