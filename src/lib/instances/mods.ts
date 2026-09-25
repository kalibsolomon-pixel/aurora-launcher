import type { ModEntry } from "$lib/backend";

export type ModFilter = "all" | "enabled" | "disabled" | "warnings";
export type ModSort = "name" | "state" | "warnings";

export interface RemovalCandidate {
  entryId: string;
  displayName: string;
  fileName: string;
}

/** Pure client-side projection over one already-loaded native snapshot. */
export function visibleMods(
  entries: readonly ModEntry[],
  query: string,
  filter: ModFilter,
  sort: ModSort,
): ModEntry[] {
  const needle = query.trim().toLocaleLowerCase();
  const result = entries.filter((entry) => {
    if (filter === "enabled" && !entry.enabled) return false;
    if (filter === "disabled" && entry.enabled) return false;
    if (filter === "warnings" && entry.warnings.length === 0) return false;
    if (needle === "") return true;
    const searchable = [
      entry.displayName,
      entry.fileName,
      entry.metadata?.id ?? "",
      ...(entry.metadata?.authors ?? []),
    ]
      .join("\n")
      .toLocaleLowerCase();
    return searchable.includes(needle);
  });

  return result.toSorted((left, right) => {
    if (sort === "state") {
      const state = Number(right.enabled) - Number(left.enabled);
      if (state !== 0) return state;
    }
    if (sort === "warnings") {
      const warnings = right.warnings.length - left.warnings.length;
      if (warnings !== 0) return warnings;
    }
    return left.displayName.localeCompare(right.displayName, undefined, {
      sensitivity: "base",
    }) || left.fileName.localeCompare(right.fileName, undefined, { sensitivity: "base" });
  });
}

export function beginRemoval(entry: ModEntry): RemovalCandidate | null {
  if (!entry.canRemove || !["userManaged", "providerManaged"].includes(entry.ownership)) return null;
  return {
    entryId: entry.entryId,
    displayName: entry.displayName,
    fileName: entry.fileName,
  };
}

/** Reconciles confirmation against the current snapshot before invoking Rust. */
export function confirmedRemovalId(
  candidate: RemovalCandidate | null,
  entries: readonly ModEntry[],
): string | null {
  if (!candidate) return null;
  const current = entries.find((entry) => entry.entryId === candidate.entryId);
  return current?.canRemove && ["userManaged", "providerManaged"].includes(current.ownership)
    ? current.entryId
    : null;
}

export function formatModSize(bytes: number | null): string | null {
  if (bytes === null) return null;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}
