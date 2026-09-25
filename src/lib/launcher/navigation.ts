/**
 * The launcher's typed navigation model — pure data and transitions with no
 * Svelte and no backend imports (only erased `import type`), so the whole
 * model is deterministically testable with the plain Node test runner.
 *
 * Two levels exist and never blur:
 *
 * - the GLOBAL level (sidebar destinations: Home, Instances, Accounts,
 *   Settings, About, and Developer in development builds only);
 * - the INSTANCE WORKSPACE level (one open instance plus its local tab).
 *
 * Instance-local content (Mods, Resource Packs, Shaders, and future Logs)
 * extends `InstanceTab`, never the global destinations.
 */

export const GLOBAL_PAGES = [
  "home",
  "instances",
  "accounts",
  "settings",
  "about",
  "developer",
] as const;

export type GlobalPage = (typeof GLOBAL_PAGES)[number];

/** Instance-local workspace tabs with real implemented functionality. */
export const INSTANCE_TABS = ["overview", "mods", "resourcePacks", "shaders", "settings"] as const;

export type InstanceTab = (typeof INSTANCE_TABS)[number];

export type NavigationState =
  | { kind: "global"; page: GlobalPage }
  | { kind: "instance"; instanceId: string; tab: InstanceTab };

export interface GlobalDestination {
  id: GlobalPage;
  label: string;
}

const DEVELOPMENT_ONLY_PAGES: readonly GlobalPage[] = ["developer"];

export function isGlobalPage(value: string): value is GlobalPage {
  return (GLOBAL_PAGES as readonly string[]).includes(value);
}

export function isInstanceTab(value: string): value is InstanceTab {
  return (INSTANCE_TABS as readonly string[]).includes(value);
}

/**
 * The sidebar's global destinations. The Developer destination exists only
 * in development builds; it is stripped from the production bundle together
 * with its page.
 */
export function globalDestinations(development: boolean): GlobalDestination[] {
  const labels: Record<GlobalPage, string> = {
    home: "Home",
    instances: "Instances",
    accounts: "Accounts",
    settings: "Settings",
    about: "About",
    developer: "Developer",
  };

  return GLOBAL_PAGES.filter(
    (page) => development || !DEVELOPMENT_ONLY_PAGES.includes(page),
  ).map((page) => ({ id: page, label: labels[page] }));
}

/**
 * Navigates to a global destination, leaving any open instance workspace.
 * The page must be a known global page; unknown strings are ignored so a
 * stray caller can never invent navigation state.
 */
export function goToGlobal(
  state: NavigationState,
  page: GlobalPage,
): NavigationState {
  if (!isGlobalPage(page)) return state;
  return { kind: "global", page };
}

/**
 * Opens one instance's workspace. A blank id changes nothing (there is no
 * instance to show); an existing workspace for the same instance keeps its
 * tab unless the caller asks for one explicitly.
 */
export function openInstance(
  state: NavigationState,
  instanceId: string,
  tab?: InstanceTab,
): NavigationState {
  const trimmed = instanceId.trim();
  if (trimmed === "") return state;
  if (
    state.kind === "instance" &&
    state.instanceId === trimmed &&
    tab === undefined
  ) {
    return state;
  }
  return { kind: "instance", instanceId: trimmed, tab: tab ?? "overview" };
}

/**
 * Switches the workspace's local tab. Global navigation state is untouched,
 * and switching tabs never changes which instance is open (or selected).
 */
export function selectInstanceTab(
  state: NavigationState,
  tab: InstanceTab,
): NavigationState {
  if (state.kind !== "instance" || !isInstanceTab(tab)) return state;
  if (state.tab === tab) return state;
  return { kind: "instance", instanceId: state.instanceId, tab };
}

/** Leaves the workspace the way the breadcrumb advertises: back to Instances. */
export function backToInstances(): NavigationState {
  return { kind: "global", page: "instances" };
}

/**
 * The sidebar destination that owns the current state. An open instance
 * workspace belongs to the Instances destination; its own sidebar entry
 * carries the page-level marker.
 */
export function activeGlobalPage(state: NavigationState): GlobalPage {
  return state.kind === "instance" ? "instances" : state.page;
}

/**
 * Resolves the workspace against the known instance list. The navigation
 * state may name an instance the registry no longer has (state refreshes are
 * not navigation events), so the view resolves existence explicitly instead
 * of the navigation store guessing.
 */
export type WorkspaceResolution =
  | { status: "loading" }
  | { status: "missing" }
  | { status: "open"; instanceId: string; tab: InstanceTab };

export function resolveWorkspace(
  state: NavigationState,
  knownInstanceIds: readonly string[] | null,
): WorkspaceResolution {
  if (state.kind !== "instance") return { status: "loading" };
  if (knownInstanceIds === null) return { status: "loading" };
  if (!knownInstanceIds.includes(state.instanceId)) {
    return { status: "missing" };
  }
  return { status: "open", instanceId: state.instanceId, tab: state.tab };
}
