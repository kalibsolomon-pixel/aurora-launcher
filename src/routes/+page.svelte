<script lang="ts">
  import { onMount } from "svelte";
  import { launcher } from "$lib/launcher/store.svelte";
  import AppShell from "$lib/shell/AppShell.svelte";
  import HomePage from "$lib/pages/HomePage.svelte";
  import InstancesPage from "$lib/pages/InstancesPage.svelte";
  import AccountsPage from "$lib/pages/AccountsPage.svelte";
  import AboutPage from "$lib/pages/AboutPage.svelte";
  import DeveloperPage from "$lib/pages/DeveloperPage.svelte";

  // Development-only pipeline proofs; the destination is stripped from
  // production builds together with its page.
  const developerDestination = import.meta.env.DEV;

  let page = $state("home");

  const destinations = $derived(
    developerDestination
      ? [
          { id: "home", label: "Home" },
          { id: "instances", label: "Instances" },
          { id: "accounts", label: "Accounts" },
          { id: "about", label: "About" },
          { id: "developer", label: "Developer" },
        ]
      : [
          { id: "home", label: "Home" },
          { id: "instances", label: "Instances" },
          { id: "accounts", label: "Accounts" },
          { id: "about", label: "About" },
        ],
  );

  function navigate(destination: string): void {
    page = destination;
  }

  onMount(() => {
    launcher.initialize();
    return () => launcher.dispose();
  });
</script>

<svelte:head>
  <title>Aurora Launcher</title>
</svelte:head>

<AppShell {page} {destinations} onNavigate={navigate}>
  {#if page === "home"}
    <HomePage onNavigate={navigate} />
  {:else if page === "instances"}
    <InstancesPage />
  {:else if page === "accounts"}
    <AccountsPage />
  {:else if page === "about"}
    <AboutPage />
  {:else if page === "developer" && developerDestination}
    <DeveloperPage />
  {/if}
</AppShell>
