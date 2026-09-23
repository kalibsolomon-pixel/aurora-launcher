<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
  import { navigation } from "$lib/launcher/navigation.svelte";
  import { activeGlobalPage, globalDestinations } from "$lib/launcher/navigation";

  let { children }: { children: import("svelte").Snippet } = $props();

  // The Developer destination is stripped from production builds together
  // with its page; the list is static per bundle.
  const destinations = globalDestinations(import.meta.env.DEV);

  /** How many instances the sidebar lists before pointing at Instances. */
  const SIDEBAR_INSTANCE_LIMIT = 4;

  const state = $derived(navigation.state);
  const inWorkspace = $derived(state.kind === "instance");
  const workspaceInstanceId = $derived(
    state.kind === "instance" ? state.instanceId : null,
  );
  const currentPage = $derived(activeGlobalPage(state));
  const instances = $derived(launcher.launcherState?.instances ?? []);
  const sidebarInstances = $derived(instances.slice(0, SIDEBAR_INSTANCE_LIMIT));
  const selectedId = $derived(launcher.launcherState?.config.selectedInstanceId ?? null);
  const account = $derived(launcher.selectedAccount);
</script>

<div class="app-frame">
  <nav class="sidebar" aria-label="Aurora Launcher">
    <div class="brand">
      <img class="brand-icon" src="/aurora-icon.png" alt="" aria-hidden="true" />
      <h1 class="brand-name">Aurora</h1>
    </div>

    <div class="nav">
      {#each destinations as destination (destination.id)}
        <button
          type="button"
          class="nav-item"
          aria-current={!inWorkspace && currentPage === destination.id ? "page" : undefined}
          onclick={() => navigation.goTo(destination.id)}
        >
          {destination.label}
        </button>
      {/each}
    </div>

    {#if instances.length > 0}
      <!--
        A restrained instance shortcut list. The registry records no
        last-played or last-opened timestamps, so entries appear in registry
        (creation) order under an honest "Instances" label — never a
        "Recent" claim the data cannot support.
      -->
      <div class="sidebar-instances">
        <p class="sidebar-label" id="sidebar-instances-label">Instances</p>
        <div class="nav">
          {#each sidebarInstances as instance (instance.id)}
            <button
              type="button"
              class="nav-item sidebar-instance-item"
              aria-current={workspaceInstanceId === instance.id ? "page" : undefined}
              title={instance.displayName}
              onclick={() => navigation.openInstance(instance.id)}
            >
              <span class="sidebar-instance-name">{instance.displayName}</span>
              {#if selectedId === instance.id}
                <span class="sidebar-instance-marker">Selected</span>
              {/if}
            </button>
          {/each}
          {#if instances.length > SIDEBAR_INSTANCE_LIMIT}
            <button
              type="button"
              class="nav-item sidebar-all-instances"
              aria-current={!inWorkspace && currentPage === "instances" ? "page" : undefined}
              onclick={() => navigation.goTo("instances")}
            >
              All instances
            </button>
          {/if}
        </div>
      </div>
    {/if}

    <div class="sidebar-spacer"></div>

    <button
      type="button"
      class="account-chip"
      onclick={() => navigation.goTo("accounts")}
      aria-label={account
        ? `Accounts — signed in as ${account.minecraftName}`
        : "Accounts — not signed in"}
    >
      {#if account}
        <span
          class="status-dot"
          class:status-success={account.status === "signedIn"}
          class:status-warning={account.status === "reauthenticationRequired"}
          aria-hidden="true"
        ></span>
        <span class="account-chip-name" title={account.minecraftName}>
          {account.minecraftName}
        </span>
      {:else}
        <span class="status-dot status-muted" aria-hidden="true"></span>
        <span class="account-chip-name">Not signed in</span>
      {/if}
    </button>

    <p class="sidebar-version">
      {launcher.status ? `Aurora Launcher ${launcher.status.launcherVersion}` : "Aurora Launcher"}
    </p>
  </nav>

  <main class="content">
    {@render children()}
  </main>
</div>
