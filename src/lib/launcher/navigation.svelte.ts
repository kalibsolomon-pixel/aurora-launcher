import {
  backToInstances,
  goToGlobal,
  openInstance,
  selectInstanceTab,
  type GlobalPage,
  type InstanceTab,
  type NavigationState,
} from "$lib/launcher/navigation";

/**
 * The frontend owner of navigation state. One typed value distinguishes the
 * global page from the instance workspace (instance id + local tab); the
 * transitions themselves are the pure functions in `navigation.ts`, so the
 * model stays testable outside Svelte.
 */
class NavigationStore {
  state = $state<NavigationState>({ kind: "global", page: "home" });

  goTo(page: GlobalPage): void {
    this.state = goToGlobal(this.state, page);
  }

  openInstance(instanceId: string, tab?: InstanceTab): void {
    this.state = openInstance(this.state, instanceId, tab);
  }

  setInstanceTab(tab: InstanceTab): void {
    this.state = selectInstanceTab(this.state, tab);
  }

  closeInstance(): void {
    this.state = backToInstances();
  }
}

export const navigation = new NavigationStore();
