<script lang="ts">
  import { onMount } from "svelte";
  import { launcher } from "$lib/launcher/store.svelte";
  import { navigation } from "$lib/launcher/navigation.svelte";
  import AppShell from "$lib/shell/AppShell.svelte";
  import HomePage from "$lib/pages/HomePage.svelte";
  import InstancesPage from "$lib/pages/InstancesPage.svelte";
  import AccountsPage from "$lib/pages/AccountsPage.svelte";
  import SettingsPage from "$lib/pages/SettingsPage.svelte";
  import AboutPage from "$lib/pages/AboutPage.svelte";
  import DeveloperPage from "$lib/pages/DeveloperPage.svelte";
  import InstanceWorkspace from "$lib/instances/InstanceWorkspace.svelte";

  // Development-only pipeline proofs; the destination is stripped from
  // production builds together with its page.
  const developerDestination = import.meta.env.DEV;

  const state = $derived(navigation.state);

  onMount(() => {
    launcher.initialize();
    return () => launcher.dispose();
  });
</script>

<svelte:head>
  <title>Aurora Launcher</title>
</svelte:head>

<AppShell>
  {#if state.kind === "instance"}
    <InstanceWorkspace instanceId={state.instanceId} tab={state.tab} />
  {:else if state.page === "home"}
    <HomePage />
  {:else if state.page === "instances"}
    <InstancesPage />
  {:else if state.page === "accounts"}
    <AccountsPage />
  {:else if state.page === "settings"}
    <SettingsPage />
  {:else if state.page === "about"}
    <AboutPage />
  {:else if state.page === "developer" && developerDestination}
    <DeveloperPage />
  {/if}
</AppShell>
