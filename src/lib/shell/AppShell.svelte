<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";

  interface NavDestination {
    id: string;
    label: string;
  }

  let {
    page,
    destinations,
    onNavigate,
    children,
  }: {
    page: string;
    destinations: NavDestination[];
    onNavigate: (id: string) => void;
    children: import("svelte").Snippet;
  } = $props();

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
          aria-current={page === destination.id ? "page" : undefined}
          onclick={() => onNavigate(destination.id)}
        >
          {destination.label}
        </button>
      {/each}
    </div>

    <div class="sidebar-spacer"></div>

    <button type="button" class="account-chip" onclick={() => onNavigate("accounts")}>
      {#if account}
        <span
          class="status-dot"
          class:status-success={account.status === "signedIn"}
          class:status-warning={account.status === "reauthenticationRequired"}
          aria-hidden="true"
        ></span>
        <span class="account-chip-name">{account.minecraftName}</span>
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
