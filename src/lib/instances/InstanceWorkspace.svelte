<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
  import { navigation } from "$lib/launcher/navigation.svelte";
  import { INSTANCE_TABS, resolveWorkspace } from "$lib/launcher/navigation";
  import { draftIsDirty, instanceContentStatus } from "$lib/launcher/instanceStatus";
  import InstanceOverviewPanel from "$lib/instances/InstanceOverviewPanel.svelte";
  import InstanceModsPanel from "$lib/instances/InstanceModsPanel.svelte";
  import InstancePacksPanel from "$lib/instances/InstancePacksPanel.svelte";
  import InstanceSettingsPanel from "$lib/instances/InstanceSettingsPanel.svelte";
  import type { InstanceTab } from "$lib/launcher/navigation";

  let { instanceId, tab }: { instanceId: string; tab: InstanceTab } = $props();

  const instances = $derived(launcher.launcherState?.instances ?? []);
  const knownIds = $derived(
    launcher.launcherState === null ? null : instances.map((instance) => instance.id),
  );
  const resolution = $derived(resolveWorkspace(navigation.state, knownIds));
  const instance = $derived(instances.find((entry) => entry.id === instanceId) ?? null);

  const validation = $derived(instance ? launcher.instanceValidations[instance.id] : undefined);
  const installingPhase = $derived(
    instance?.state === "installing" && launcher.createProgress
      ? launcher.createProgress.phase
      : null,
  );
  const contentBadge = $derived(
    instance ? instanceContentStatus(instance, validation, installingPhase) : null,
  );

  const isSelected = $derived(
    launcher.launcherState?.config.selectedInstanceId === instanceId,
  );
  const readiness = $derived(
    launcher.playReadiness && launcher.playReadiness.instanceId === instanceId
      ? launcher.playReadiness
      : null,
  );
  const process = $derived(
    launcher.playProcess && launcher.playProcess.instanceId === instanceId
      ? launcher.playProcess
      : null,
  );

  const draft = $derived(instance ? launcher.draftFor(instance.id) : null);
  const settingsDirty = $derived(
    instance && draft ? draftIsDirty(draft, instance.configuration) : false,
  );

  const tabLabels: Record<InstanceTab, string> = {
    overview: "Overview",
    mods: "Mods",
    resourcePacks: "Resource Packs",
    shaders: "Shaders",
    settings: "Settings",
  };

  const playLabel = $derived.by(() => {
    if (process?.status === "running") return "Running";
    if (launcher.instanceBusy === instanceId) return "Selecting…";
    if (launcher.playBusy) return "Starting…";
    return isSelected ? "Play" : "Select & play";
  });

  const playDisabled = $derived(
    process?.status === "running" ||
      launcher.playBusy ||
      launcher.playReadinessBusy ||
      launcher.instanceBusy === instanceId ||
      (isSelected && !readiness?.ready),
  );

  function onTabKeydown(event: KeyboardEvent): void {
    const currentIndex = INSTANCE_TABS.indexOf(tab);
    let nextIndex: number | null = null;
    switch (event.key) {
      case "ArrowRight":
        nextIndex = (currentIndex + 1) % INSTANCE_TABS.length;
        break;
      case "ArrowLeft":
        nextIndex = (currentIndex - 1 + INSTANCE_TABS.length) % INSTANCE_TABS.length;
        break;
      case "Home":
        nextIndex = 0;
        break;
      case "End":
        nextIndex = INSTANCE_TABS.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    const next = INSTANCE_TABS[nextIndex];
    if (next === undefined) return;
    navigation.setInstanceTab(next);
    document.getElementById(`instance-tab-${next}`)?.focus();
  }
</script>

<!--
  The contextual shell for one open instance: a compact header (breadcrumb
  back to Instances, identity, readiness badge, Open folder and Play), a
  local tab row, and the active tab's content. The shell stays mounted
  while tabs switch, so switching never rebuilds the page or loses state.
-->
<div class="page workspace-page">
  {#if launcher.stateError}
    <section class="group" aria-live="polite">
      <div class="group-heading">
        <div>
          <h3 class="group-title">Launcher state</h3>
          <p class="group-subtitle">Persisted launcher state could not be loaded.</p>
        </div>
        <span class="status-badge status-error">Error</span>
      </div>
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.stateError.message}
        <code>{launcher.stateError.code}</code>
      </p>
    </section>
  {:else if resolution.status === "loading"}
    <section class="group" aria-live="polite">
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <span class="group-row-detail">Loading persisted launcher state…</span>
      </div>
    </section>
  {:else if resolution.status === "missing" || !instance}
    <section class="empty-state" aria-live="polite">
      <h3 class="empty-title">Instance not found</h3>
      <p class="empty-detail">
        This instance no longer exists in the launcher's registry. It may have been
        removed while Aurora was open.
      </p>
      <button type="button" class="btn btn-primary" onclick={() => navigation.closeInstance()}>
        Back to Instances
      </button>
    </section>
  {:else}
    <header class="page-header workspace-header">
      <div class="workspace-identity">
        <nav class="breadcrumb" aria-label="Instance">
          <button
            type="button"
            class="breadcrumb-link"
            onclick={() => navigation.closeInstance()}
          >
            Instances
          </button>
          <span class="breadcrumb-separator" aria-hidden="true">›</span>
        </nav>
        <div class="workspace-title-line">
          <h2 class="page-title">{instance.displayName}</h2>
          {#if contentBadge}
            <span class="status-badge {contentBadge.tone}">{contentBadge.label}</span>
          {/if}
        </div>
        <p class="page-subtitle">
          Aurora {instance.auroraVersion} ({instance.channel}) · Minecraft
          {instance.minecraftVersion} · Fabric {instance.fabricLoaderVersion}
        </p>
      </div>
      <div class="page-header-actions">
        <button
          type="button"
          class="btn"
          onclick={() => launcher.runOpenFolder(instance.id)}
          disabled={launcher.folderBusy !== null}
        >
          {launcher.folderBusy === instance.id ? "Opening…" : "Open folder"}
        </button>
        <button
          type="button"
          class="btn btn-primary"
          onclick={() => launcher.runWorkspacePlay(instance.id)}
          disabled={playDisabled}
        >
          {playLabel}
        </button>
      </div>
    </header>

    {#if launcher.folderError}
      <p class="inline-message inline-message-error workspace-alert" role="alert">
        {launcher.folderError.message}
        <code>{launcher.folderError.code}</code>
      </p>
    {/if}

    <div class="workspace-tabs" role="tablist" aria-label="Instance sections">
      {#each INSTANCE_TABS as candidate (candidate)}
        <button
          type="button"
          role="tab"
          id="instance-tab-{candidate}"
          class="workspace-tab"
          class:workspace-tab-active={tab === candidate}
          aria-selected={tab === candidate}
          aria-controls="instance-tabpanel"
          tabindex={tab === candidate ? 0 : -1}
          onclick={() => navigation.setInstanceTab(candidate)}
          onkeydown={onTabKeydown}
        >
          {tabLabels[candidate]}
          {#if candidate === "settings" && settingsDirty}
            <span class="workspace-tab-marker">Unsaved</span>
          {/if}
        </button>
      {/each}
    </div>

    <div id="instance-tabpanel" role="tabpanel" aria-labelledby="instance-tab-{tab}">
      {#if tab === "overview"}
        <InstanceOverviewPanel {instance} />
      {:else if tab === "mods"}
        <InstanceModsPanel {instance} />
      {:else if tab === "resourcePacks"}
        <InstancePacksPanel {instance} kind="resourcePack" />
      {:else if tab === "shaders"}
        <InstancePacksPanel {instance} kind="shaderPack" />
      {:else}
        <InstanceSettingsPanel {instance} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .workspace-header {
    align-items: flex-start;
  }

  .workspace-identity {
    min-width: 0;
  }

  .breadcrumb {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    margin-bottom: var(--space-1);
  }

  .breadcrumb-link {
    padding: 0;
    border: none;
    background: none;
    color: var(--color-text-muted);
    font: inherit;
    font-size: var(--text-metadata);
    cursor: pointer;
  }

  .breadcrumb-link:hover {
    color: var(--color-text);
  }

  .breadcrumb-separator {
    color: var(--color-text-muted);
    font-size: var(--text-metadata);
  }

  .workspace-title-line {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    flex-wrap: wrap;
  }

  .workspace-title-line .page-title {
    overflow-wrap: anywhere;
  }

  .workspace-alert {
    margin: 0 0 var(--space-4);
  }

  /* Instance-local tab navigation: a compact horizontal row under the
     workspace header — a third navigation column is deliberately absent. */
  .workspace-tabs {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    border-bottom: 1px solid var(--color-border);
    margin-bottom: var(--space-5);
  }

  .workspace-tab {
    padding: var(--space-2) var(--space-3) calc(var(--space-2) + 1px);
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: var(--radius-sm) var(--radius-sm) 0 0;
    background: none;
    color: var(--color-text-secondary);
    font: inherit;
    font-size: var(--text-body);
    font-weight: 500;
    cursor: pointer;
    transition: color var(--motion-fast) var(--motion-ease),
      background-color var(--motion-fast) var(--motion-ease);
  }

  .workspace-tab:hover {
    color: var(--color-text);
    background: var(--color-surface-raised);
  }

  .workspace-tab-active {
    color: var(--color-text);
    font-weight: 600;
    /* The underline marks the active tab alongside aria-selected and the
       weight change, so selection is never color-alone. */
    border-bottom-color: var(--color-accent);
  }

  .workspace-tab-marker {
    margin-left: var(--space-2);
    color: var(--color-text-muted);
    font-size: var(--text-metadata);
    font-weight: 500;
  }

  @media (prefers-reduced-motion: reduce) {
    .workspace-tab {
      transition: none;
    }
  }
</style>
