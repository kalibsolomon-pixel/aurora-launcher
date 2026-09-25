<script lang="ts">
  import { tick } from "svelte";
  import { launcher } from "$lib/launcher/store.svelte";
  import {
    beginRemoval,
    confirmedRemovalId,
    formatModSize,
    visibleMods,
    type ModFilter,
    type ModSort,
    type RemovalCandidate,
  } from "$lib/instances/mods";
  import type { InstanceSummary, ModEntry } from "$lib/backend";
  import ModrinthBrowse from "./ModrinthBrowse.svelte";

  let { instance }: { instance: InstanceSummary } = $props();
  let query = $state("");
  let filter = $state<ModFilter>("all");
  let sort = $state<ModSort>("name");
  let loadedInstance = $state("");
  let removal = $state<RemovalCandidate | null>(null);
  let confirmButton: HTMLButtonElement | null = $state(null);
  let view = $state<"installed" | "browse">("installed");

  const inventory = $derived(launcher.modInventories[instance.id] ?? null);
  const entries = $derived(inventory?.entries ?? []);
  const shown = $derived(visibleMods(entries, query, filter, sort));
  const running = $derived(
    launcher.playProcess?.instanceId === instance.id &&
      launcher.playProcess.status === "running",
  );

  $effect(() => {
    if (loadedInstance !== instance.id) {
      loadedInstance = instance.id;
      if (launcher.modInventories[instance.id] === undefined) {
        void launcher.runLoadMods(instance.id);
      }
    }
  });

  async function askRemove(entry: ModEntry): Promise<void> {
    removal = beginRemoval(entry);
    await tick();
    confirmButton?.focus();
  }

  async function confirmRemove(): Promise<void> {
    const entryId = confirmedRemovalId(removal, entries);
    if (!entryId) {
      removal = null;
      return;
    }
    await launcher.runRemoveMod(instance.id, entryId);
    removal = null;
  }

  function cancelRemove(): void {
    const triggerId = removal?.entryId;
    removal = null;
    void tick().then(() => document.getElementById(`actions-${triggerId}`)?.focus());
  }

  function onConfirmationKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      cancelRemove();
    }
  }

  function stateLabel(entry: ModEntry): string {
    if (entry.ownership === "launcherManagedRequired") return "Required";
    if (entry.ownership === "providerManaged") return "Managed";
    if (entry.fileType === "enabledJar") return "Enabled";
    if (entry.fileType === "disabledJar") return "Disabled";
    if (entry.fileType === "link") return "Link";
    if (entry.fileType === "directory") return "Folder";
    return "Unclassified";
  }
</script>

<section class="mods-panel" aria-labelledby="mods-title">
  <div class="content-view-tabs" role="group" aria-label="Mods view">
    <button type="button" class="btn btn-quiet" aria-pressed={view === "installed"} onclick={() => view = "installed"}>Installed</button>
    <button type="button" class="btn btn-quiet" aria-pressed={view === "browse"} onclick={() => view = "browse"}>Browse</button>
  </div>
  {#if view === "browse"}
    <ModrinthBrowse instanceId={instance.id} instanceName={instance.displayName} minecraftVersion={instance.minecraftVersion} kind="mod" installedProjectIds={entries.filter((entry) => entry.provenance?.provider === "modrinth").map((entry) => entry.provenance!.projectId)} onInstalled={async (targetId) => { await launcher.runLoadMods(targetId); }} />
  {:else}
  {#if inventory?.missingManaged.length}
    <p class="mods-notice" role="status">{inventory.missingManaged.length} managed mod file{inventory.missingManaged.length === 1 ? " is" : "s are"} missing. Refresh or inspect the instance folder; Aurora will not recreate files automatically.</p>
    <details class="missing-details"><summary>Missing managed files</summary><ul>{#each inventory.missingManaged as record}<li>{record.fileName} · {record.provider}</li>{/each}</ul></details>
  {/if}
  <div class="mods-heading">
    <div>
      <h3 id="mods-title" class="group-title">Mods</h3>
      <p class="group-subtitle">Local files in this instance. Use Browse to find compatible Modrinth mods.</p>
    </div>
    <div class="mods-heading-actions">
      <button
        type="button"
        class="btn btn-quiet"
        onclick={() => launcher.runLoadMods(instance.id)}
        disabled={launcher.modInventoryBusy === instance.id || launcher.modMutationBusy !== null}
      >
        {launcher.modInventoryBusy === instance.id ? "Refreshing…" : "Refresh"}
      </button>
      <button
        type="button"
        class="btn"
        onclick={() => launcher.runOpenModsFolder(instance.id)}
        disabled={launcher.modFolderBusy !== null}
      >
        {launcher.modFolderBusy === instance.id ? "Opening…" : "Open mods folder"}
      </button>
    </div>
  </div>

  {#if running}
    <p class="mods-notice" role="status">
      Minecraft is running. Local changes are for the next launch and do not hot-reload the current game.
    </p>
  {/if}

  {#if launcher.modError}
    <div class="mods-error" role="alert">
      <p>{launcher.modError.message}</p>
      <code>{launcher.modError.code}</code>
      <button type="button" class="btn btn-quiet" onclick={() => launcher.runLoadMods(instance.id)}>
        Retry
      </button>
    </div>
  {/if}

  {#if inventory}
    <div class="mods-toolbar">
      <label class="mods-search">
        <span class="field-label">Search mods</span>
        <input bind:value={query} type="search" placeholder="Name, ID, filename, or author" />
      </label>
      <label class="mods-control">
        <span class="field-label">Show</span>
        <select bind:value={filter} aria-label="Filter mods">
          <option value="all">All</option>
          <option value="enabled">Enabled</option>
          <option value="disabled">Disabled</option>
          <option value="warnings">Warnings</option>
        </select>
      </label>
      <label class="mods-control">
        <span class="field-label">Sort</span>
        <select bind:value={sort} aria-label="Sort mods">
          <option value="name">Name</option>
          <option value="state">Enabled first</option>
          <option value="warnings">Warnings first</option>
        </select>
      </label>
    </div>

    {#if entries.length === 0}
      <div class="empty-state mods-empty">
        <h4 class="empty-title">No local mods found</h4>
        <p class="empty-detail">This instance's mods directory is empty.</p>
        <button type="button" class="btn" onclick={() => launcher.runOpenModsFolder(instance.id)}>
          Open mods folder
        </button>
      </div>
    {:else if shown.length === 0}
      <div class="mods-no-results" role="status">
        No mods match the current search and filter.
      </div>
    {:else}
      <div class="mod-list" aria-label={`${shown.length} local mod entries`}>
        {#each shown as entry (entry.entryId)}
          <article class="mod-row" class:mod-row-disabled={!entry.enabled}>
            <div class="mod-row-main">
              <div class="mod-glyph" aria-hidden="true">{entry.metadata ? "M" : "J"}</div>
              <div class="mod-identity">
                <div class="mod-title-line">
                  <h4>{entry.displayName}</h4>
                  {#if entry.metadata?.version}
                    <span class="mod-version">{entry.metadata.version}</span>
                  {/if}
                </div>
                <p class="mod-meta">
                  {entry.metadata ? "Fabric" : "Metadata unavailable"}
                  {#if entry.metadata?.authors.length}
                    · By {entry.metadata.authors.slice(0, 2).join(", ")}{entry.metadata.authors.length > 2 ? "…" : ""}
                  {/if}
                </p>
                <p class="mod-file" title={entry.fileName}>
                  {entry.fileName}{formatModSize(entry.sizeBytes) ? ` · ${formatModSize(entry.sizeBytes)}` : ""}
                </p>
                {#if entry.warnings.length}
                  <p class="mod-warning">
                    <span aria-hidden="true">⚠</span>
                    {entry.warnings[0]?.message}
                    {#if entry.warnings.length > 1}
                      <span> (+{entry.warnings.length - 1} more)</span>
                    {/if}
                  </p>
                {/if}
              </div>
            </div>

            <div class="mod-row-actions">
              <span class="mod-state" class:mod-state-required={entry.ownership === "launcherManagedRequired"}>
                {launcher.modMutationBusy === entry.entryId ? "Changing…" : stateLabel(entry)}
              </span>
              {#if entry.canToggle}
                <button
                  type="button"
                  role="switch"
                  class="mod-switch"
                  aria-checked={entry.enabled}
                  aria-label={`${entry.enabled ? "Disable" : "Enable"} ${entry.displayName}`}
                  disabled={launcher.modMutationBusy !== null}
                  onclick={() => launcher.runSetModEnabled(instance.id, entry.entryId, !entry.enabled)}
                >
                  <span aria-hidden="true"></span>
                </button>
              {:else}
                <span class="protected-marker" title={entry.actionBlockedReason ?? undefined}>
                  {entry.ownership === "launcherManagedRequired" ? "Protected" : entry.ownership === "providerManaged" ? "No toggle" : "Unavailable"}
                </span>
              {/if}
              <details class="mod-actions-menu">
                <summary id="actions-{entry.entryId}" aria-label={`Actions for ${entry.displayName}`}>•••</summary>
                <div class="mod-actions-popover">
                  {#if entry.canRemove}
                    <button
                      type="button"
                      class="menu-action menu-action-danger"
                      disabled={launcher.modMutationBusy !== null}
                      onclick={(event) => {
                        const menu = event.currentTarget.closest("details") as HTMLDetailsElement | null;
                        if (menu) menu.open = false;
                        void askRemove(entry);
                      }}
                    >Remove…</button>
                  {:else}
                    <p>{entry.actionBlockedReason}</p>
                  {/if}
                </div>
              </details>
            </div>

            <details class="mod-details">
              <summary>Details</summary>
              <dl>
                <div><dt>File</dt><dd>{entry.fileName}</dd></div>
                <div><dt>Ownership</dt><dd>{entry.ownership === "launcherManagedRequired" ? "Managed by Aurora · required" : entry.ownership === "providerManaged" ? `Managed · ${entry.provenance?.provider}` : entry.ownership === "userManaged" ? "Local mod" : "Unclassified"}</dd></div>
                {#if entry.provenance}<div><dt>Provider</dt><dd>{entry.provenance.provider} · {entry.provenance.projectId} · {entry.provenance.displayVersion ?? entry.provenance.versionId}</dd></div>{/if}
                {#if entry.metadata}
                  <div><dt>Mod ID</dt><dd>{entry.metadata.id}</dd></div>
                  {#if entry.metadata.environment}<div><dt>Environment</dt><dd>{entry.metadata.environment}</dd></div>{/if}
                  {#if entry.metadata.depends.length}<div><dt>Requires</dt><dd>{entry.metadata.depends.map((item) => `${item.modId} ${item.requirement}`).join(", ")}</dd></div>{/if}
                {/if}
                {#if entry.warnings.length}
                  <div class="details-warnings"><dt>Metadata warnings</dt><dd><ul>{#each entry.warnings as warning}<li>{warning.message}</li>{/each}</ul></dd></div>
                {/if}
              </dl>
            </details>

            {#if removal?.entryId === entry.entryId}
              <div
                class="remove-confirmation"
                role="group"
                aria-labelledby="remove-title-{entry.entryId}"
              >
                <div>
                  <strong id="remove-title-{entry.entryId}">Remove {entry.displayName}?</strong>
                  <p>This permanently deletes <span>{entry.fileName}</span>. This cannot be undone.</p>
                </div>
                <div class="remove-actions">
                  <button type="button" class="btn btn-quiet" onclick={cancelRemove} onkeydown={onConfirmationKeydown}>Cancel</button>
                  <button
                    bind:this={confirmButton}
                    type="button"
                    class="btn btn-danger"
                    disabled={launcher.modMutationBusy !== null}
                    onclick={confirmRemove}
                    onkeydown={onConfirmationKeydown}
                  >
                    {launcher.modMutationBusy === entry.entryId ? "Removing…" : "Remove permanently"}
                  </button>
                </div>
              </div>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  {:else if launcher.modInventoryBusy === instance.id}
    <div class="mods-loading" aria-live="polite">
      <span class="spinner" aria-hidden="true"></span>
      <span>Inspecting local mod files…</span>
    </div>
  {/if}
  {/if}
</section>

<style>
  .content-view-tabs { display: flex; gap: var(--space-2); margin-bottom: var(--space-4); }
  .content-view-tabs [aria-pressed="true"] { color: var(--color-text); background: var(--color-surface-raised); }
  .mods-panel { min-width: 0; }
  .mods-heading, .mods-heading-actions, .mods-toolbar, .mod-title-line, .mod-row-actions, .remove-actions {
    display: flex;
    align-items: center;
  }
  .mods-heading { justify-content: space-between; gap: var(--space-4); margin-bottom: var(--space-4); }
  .mods-heading-actions, .remove-actions { gap: var(--space-2); flex-wrap: wrap; }
  .mods-notice { margin: 0 0 var(--space-3); padding: var(--space-2) var(--space-3); border-radius: var(--radius-sm); background: var(--color-working-soft); color: var(--color-working); font-size: var(--text-secondary); }
  .missing-details { margin: 0 0 var(--space-4); font-size: var(--text-metadata); color: var(--color-text-secondary); }
  .mods-error { display: flex; align-items: center; gap: var(--space-2); margin-bottom: var(--space-3); padding: var(--space-3); border-radius: var(--radius-sm); background: var(--color-error-soft); color: var(--color-error); font-size: var(--text-secondary); flex-wrap: wrap; }
  .mods-error p { margin: 0; flex: 1; }
  .mods-error code { color: var(--color-text-muted); font-size: var(--text-metadata); }
  .mods-toolbar { align-items: end; gap: var(--space-3); margin-bottom: var(--space-4); }
  .mods-search, .mods-control { display: grid; gap: var(--space-1); }
  .mods-search { flex: 1; min-width: 180px; }
  .mods-control { width: 128px; }
  .mods-toolbar input, .mods-toolbar select { width: 100%; padding: var(--space-2) var(--space-3); border: 1px solid var(--color-border-strong); border-radius: var(--radius-sm); background: var(--color-surface-sunken); color: var(--color-text); font: inherit; font-size: var(--text-body); }
  .mod-list { overflow: visible; border: 1px solid var(--color-surface-edge); border-radius: var(--radius-lg); background: var(--color-surface); box-shadow: var(--shadow-group); }
  .mod-row { position: relative; display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: var(--space-2) var(--space-4); padding: var(--space-3) var(--space-4); border-bottom: 1px solid var(--color-border); min-width: 0; }
  .mod-row:last-child { border-bottom: none; }
  .mod-row-disabled .mod-identity { opacity: 0.7; }
  .mod-row-main { display: flex; gap: var(--space-3); min-width: 0; }
  .mod-glyph { display: grid; place-items: center; width: 34px; height: 34px; flex: none; border-radius: var(--radius-sm); background: var(--color-surface-raised); color: var(--color-text-secondary); font-size: var(--text-metadata); font-weight: 700; }
  .mod-identity { min-width: 0; }
  .mod-title-line { gap: var(--space-2); min-width: 0; }
  .mod-title-line h4 { margin: 0; overflow: hidden; color: var(--color-text); font-size: var(--text-body); font-weight: 600; text-overflow: ellipsis; white-space: nowrap; }
  .mod-version, .mod-file { color: var(--color-text-muted); font-size: var(--text-metadata); }
  .mod-meta, .mod-file, .mod-warning { margin: 2px 0 0; }
  .mod-meta { color: var(--color-text-secondary); font-size: var(--text-secondary); }
  .mod-file { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .mod-warning { color: var(--color-warning); font-size: var(--text-metadata); line-height: 1.4; }
  .mod-row-actions { align-self: start; justify-content: flex-end; gap: var(--space-2); }
  .mod-state, .protected-marker { color: var(--color-text-secondary); font-size: var(--text-metadata); font-weight: 500; white-space: nowrap; }
  .mod-state-required { color: var(--color-working); }
  .protected-marker { color: var(--color-text-muted); }
  .mod-switch { position: relative; width: 38px; height: 22px; padding: 2px; border: 1px solid var(--color-border-strong); border-radius: 999px; background: var(--color-surface-sunken); cursor: pointer; }
  .mod-switch span { display: block; width: 16px; height: 16px; border-radius: 50%; background: var(--color-text-muted); transition: transform var(--motion-fast) var(--motion-ease), background-color var(--motion-fast) var(--motion-ease); }
  .mod-switch[aria-checked="true"] { border-color: var(--color-accent); background: var(--color-accent-soft); }
  .mod-switch[aria-checked="true"] span { transform: translateX(16px); background: var(--color-accent); }
  .mod-switch:disabled { opacity: 0.5; cursor: progress; }
  .mod-actions-menu { position: relative; }
  .mod-actions-menu summary { display: grid; place-items: center; width: 32px; height: 28px; border-radius: var(--radius-sm); color: var(--color-text-secondary); cursor: pointer; list-style: none; }
  .mod-actions-menu summary::-webkit-details-marker { display: none; }
  .mod-actions-menu summary:hover { background: var(--color-surface-raised); color: var(--color-text); }
  .mod-actions-popover { position: absolute; z-index: 2; top: calc(100% + var(--space-1)); right: 0; min-width: 180px; padding: var(--space-2); border: 1px solid var(--color-border); border-radius: var(--radius-md); background: var(--color-surface-raised); box-shadow: var(--shadow-group); }
  .mod-actions-popover p { margin: 0; color: var(--color-text-muted); font-size: var(--text-metadata); line-height: 1.4; }
  .menu-action { width: 100%; padding: var(--space-2); border: none; border-radius: var(--radius-sm); background: transparent; color: var(--color-text); font: inherit; text-align: left; cursor: pointer; }
  .menu-action-danger { color: var(--color-error); }
  .menu-action:hover { background: var(--color-surface-hover); }
  .mod-details { grid-column: 1 / -1; margin-left: 46px; color: var(--color-text-secondary); font-size: var(--text-metadata); }
  .mod-details summary { width: fit-content; color: var(--color-text-muted); cursor: pointer; }
  .mod-details dl { display: grid; gap: var(--space-1); margin: var(--space-2) 0 0; }
  .mod-details dl > div { display: grid; grid-template-columns: 88px minmax(0, 1fr); gap: var(--space-2); }
  .mod-details dt { color: var(--color-text-muted); }
  .mod-details dd { margin: 0; overflow-wrap: anywhere; }
  .mod-details ul { margin: 0; padding-left: var(--space-4); }
  .remove-confirmation { grid-column: 1 / -1; display: flex; align-items: center; justify-content: space-between; gap: var(--space-4); margin-top: var(--space-2); padding: var(--space-3); border-radius: var(--radius-md); background: var(--color-error-soft); }
  .remove-confirmation strong { color: var(--color-text); font-size: var(--text-body); }
  .remove-confirmation p { margin: 2px 0 0; color: var(--color-text-secondary); font-size: var(--text-metadata); }
  .remove-confirmation p span { color: var(--color-text); }
  .mods-loading, .mods-no-results { display: flex; align-items: center; gap: var(--space-3); min-height: 96px; color: var(--color-text-secondary); font-size: var(--text-body); }
  .mods-no-results { justify-content: center; }
  .mods-empty { padding-top: var(--space-5); }

  @media (max-width: 800px) {
    .mods-heading { align-items: flex-start; flex-direction: column; }
    .mods-toolbar { align-items: stretch; flex-wrap: wrap; }
    .mods-search { flex-basis: 100%; }
    .mods-control { flex: 1; min-width: 120px; }
    .mod-row { grid-template-columns: minmax(0, 1fr); }
    .mod-row-actions { justify-content: flex-start; margin-left: 46px; }
    .remove-confirmation { align-items: flex-start; flex-direction: column; }
  }

  @media (prefers-reduced-motion: reduce) {
    .mod-switch span { transition: none; }
  }
</style>
