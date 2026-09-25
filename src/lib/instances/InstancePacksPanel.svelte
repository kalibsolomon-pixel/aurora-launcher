<script lang="ts">
  import { tick } from "svelte";
  import type { ContentEntry, InstanceSummary } from "$lib/backend";
  import { launcher } from "$lib/launcher/store.svelte";
  import { formatModSize } from "./mods";
  import ModrinthBrowse from "./ModrinthBrowse.svelte";

  let { instance, kind }: { instance: InstanceSummary; kind: "resourcePack" | "shaderPack" } = $props();
  let query = $state("");
  let filter = $state<"all" | "warnings" | "managed">("all");
  let removal = $state<ContentEntry | null>(null);
  let confirmButton: HTMLButtonElement | null = $state(null);
  let loadedKey = $state("");
  let view = $state<"installed" | "browse">("installed");
  const key = $derived(`${instance.id}:${kind}`);
  const title = $derived(kind === "resourcePack" ? "Resource Packs" : "Shaders");
  const directoryName = $derived(kind === "resourcePack" ? "resourcepacks" : "shaderpacks");
  const inventory = $derived(launcher.contentInventories[key] ?? null);
  const entries = $derived(inventory?.entries ?? []);
  const shown = $derived(entries.filter((entry) => {
    if (filter === "warnings" && !entry.warnings.length) return false;
    if (filter === "managed" && entry.ownership !== "providerManaged") return false;
    const needle = query.trim().toLocaleLowerCase();
    return !needle || [entry.displayName, entry.fileName, entry.provenance?.provider ?? ""].join("\n").toLocaleLowerCase().includes(needle);
  }));
  const running = $derived(launcher.playProcess?.instanceId === instance.id && launcher.playProcess.status === "running");

  $effect(() => {
    if (loadedKey !== key) {
      loadedKey = key;
      if (!launcher.contentInventories[key]) void launcher.runLoadContent(instance.id, kind);
    }
  });

  async function askRemove(entry: ContentEntry): Promise<void> {
    if (!entry.canRemove) return;
    removal = entry;
    await tick();
    confirmButton?.focus();
  }

  async function confirmRemove(): Promise<void> {
    const candidate = removal;
    if (!candidate || !entries.some((entry) => entry.entryId === candidate.entryId && entry.canRemove)) {
      removal = null;
      return;
    }
    await launcher.runRemoveContent(instance.id, kind, candidate.entryId);
    removal = null;
  }

  function owner(entry: ContentEntry): string {
    if (entry.ownership === "providerManaged") return `Managed · ${entry.provenance?.provider}`;
    if (entry.ownership === "userManaged") return "Local";
    return "Unknown";
  }
</script>

<section class="packs-panel" aria-label={title}>
  <div class="content-view-tabs" role="group" aria-label={`${title} view`}>
    <button type="button" class="btn btn-quiet" aria-pressed={view === "installed"} onclick={() => view = "installed"}>Installed</button>
    <button type="button" class="btn btn-quiet" aria-pressed={view === "browse"} onclick={() => view = "browse"}>Browse</button>
  </div>
  {#if view === "browse"}
    <ModrinthBrowse instanceId={instance.id} instanceName={instance.displayName} minecraftVersion={instance.minecraftVersion} {kind} installedProjectIds={entries.filter((entry) => entry.provenance?.provider === "modrinth").map((entry) => entry.provenance!.projectId)} onInstalled={async (targetId, targetKind) => { if (targetKind !== "mod") await launcher.runLoadContent(targetId, targetKind); }} />
  {:else}
  <div class="packs-heading">
    <div>
      <h3 class="group-title">{title}</h3>
      <p class="group-subtitle">Local files in this instance's {directoryName} folder.</p>
    </div>
    <div class="packs-heading-actions">
      <button type="button" class="btn btn-quiet" onclick={() => launcher.runLoadContent(instance.id, kind)} disabled={launcher.contentBusy === key || launcher.contentMutationBusy !== null}>Refresh</button>
      <button type="button" class="btn" onclick={() => launcher.runOpenContentFolder(instance.id, kind)}>Open folder</button>
    </div>
  </div>

  {#if running}<p class="packs-note">Changes made while Minecraft is running apply on the next launch.</p>{/if}
  {#if launcher.contentError}<p class="inline-message inline-message-error" role="alert">{launcher.contentError.message} <code>{launcher.contentError.code}</code></p>{/if}
  {#if inventory?.missingManaged.length}
    <p class="packs-note" role="status">{inventory.missingManaged.length} managed {kind === "resourcePack" ? "resource pack" : "shader pack"} file{inventory.missingManaged.length === 1 ? " is" : "s are"} missing. Aurora will not recreate files automatically.</p>
    <details class="missing-details"><summary>Missing managed files</summary><ul>{#each inventory.missingManaged as record}<li>{record.fileName} · {record.provider}</li>{/each}</ul></details>
  {/if}

  {#if inventory}
    <div class="packs-toolbar">
      <label><span class="field-label">Search</span><input type="search" bind:value={query} placeholder="Name, filename, or provider" /></label>
      <label><span class="field-label">Show</span><select bind:value={filter} aria-label={`Filter ${title}`}><option value="all">All</option><option value="warnings">Warnings</option><option value="managed">Managed</option></select></label>
    </div>
    {#if entries.length === 0}
      <div class="empty-state"><h4 class="empty-title">No local {title.toLowerCase()} found</h4><p class="empty-detail">Add ZIP files or pack folders to this instance's {directoryName} folder, then Refresh.</p></div>
    {:else if shown.length === 0}
      <p role="status">No entries match the current search and filter.</p>
    {:else}
      <div class="packs-list">
        {#each shown as entry (entry.entryId)}
          <article class="pack-row">
            <div class="pack-main">
              <div class="pack-glyph" aria-hidden="true">{kind === "resourcePack" ? "R" : "S"}</div>
              <div class="pack-identity">
                <h4>{entry.displayName}</h4>
                <p class="pack-meta">{owner(entry)} · {entry.fileType === "directory" ? "Folder" : entry.fileType === "zip" ? "ZIP" : "Unclassified"}{formatModSize(entry.sizeBytes) ? ` · ${formatModSize(entry.sizeBytes)}` : ""}</p>
                {#if entry.description}<p class="pack-description">{entry.description}</p>{/if}
                {#if entry.warnings.length}<p class="pack-warning">⚠ {entry.warnings[0]?.message}{entry.warnings.length > 1 ? ` (+${entry.warnings.length - 1} more)` : ""}</p>{/if}
              </div>
            </div>
            <div class="pack-actions">
              {#if entry.canRemove}<button type="button" class="btn btn-quiet" disabled={launcher.contentMutationBusy !== null} onclick={() => askRemove(entry)}>Remove…</button>{:else}<span title="Folder packs and unsafe entries are left untouched">Unavailable</span>{/if}
            </div>
            <details class="pack-details"><summary>Details</summary>
              <dl>
                <div><dt>File</dt><dd>{entry.fileName}</dd></div>
                <div><dt>Ownership</dt><dd>{owner(entry)}</dd></div>
                {#if entry.packFormat !== null}<div><dt>Pack format</dt><dd>{entry.packFormat}</dd></div>{/if}
                {#if entry.provenance}<div><dt>Provider project</dt><dd>{entry.provenance.projectId}</dd></div><div><dt>Version</dt><dd>{entry.provenance.displayVersion ?? entry.provenance.versionId}</dd></div>{/if}
                {#if entry.warnings.length}<div><dt>Warnings</dt><dd><ul>{#each entry.warnings as warning}<li>{warning.message}</li>{/each}</ul></dd></div>{/if}
              </dl>
            </details>
            {#if removal?.entryId === entry.entryId}
              <div class="pack-confirm" role="group" aria-label={`Remove ${entry.displayName}?`}>
                <p>Remove {entry.fileName} permanently? This cannot be undone.</p>
                <button type="button" class="btn btn-quiet" onclick={() => removal = null}>Cancel</button>
                <button bind:this={confirmButton} type="button" class="btn btn-danger" disabled={launcher.contentMutationBusy !== null} onclick={confirmRemove}>Remove permanently</button>
              </div>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  {:else if launcher.contentBusy === key}
    <div class="group-row group-row-loading"><span class="spinner" aria-hidden="true"></span> Inspecting local {title.toLowerCase()}…</div>
  {/if}
  {/if}
</section>

<style>
  .content-view-tabs { display: flex; gap: var(--space-2); margin-bottom: var(--space-4); }
  .content-view-tabs [aria-pressed="true"] { color: var(--color-text); background: var(--color-surface-raised); }
  .packs-panel { min-width: 0; }
  .packs-heading, .packs-heading-actions, .packs-toolbar, .pack-main, .pack-actions, .pack-confirm { display: flex; align-items: center; gap: var(--space-3); }
  .packs-heading { justify-content: space-between; margin-bottom: var(--space-4); }
  .packs-heading-actions { flex-wrap: wrap; justify-content: flex-end; }
  .packs-note { padding: var(--space-2) var(--space-3); background: var(--color-working-soft); border-radius: var(--radius-sm); color: var(--color-working); }
  .packs-toolbar { align-items: end; margin-bottom: var(--space-4); flex-wrap: wrap; }
  .packs-toolbar label { display: grid; gap: var(--space-1); }
  .packs-toolbar label:first-child { flex: 1; min-width: 180px; }
  .packs-toolbar select { min-width: 128px; }
  .packs-toolbar input, .packs-toolbar select { padding: var(--space-2) var(--space-3); border: 1px solid var(--color-border-strong); border-radius: var(--radius-sm); background: var(--color-surface-sunken); color: var(--color-text); font: inherit; }
  .packs-list { border: 1px solid var(--color-surface-edge); border-radius: var(--radius-lg); background: var(--color-surface); box-shadow: var(--shadow-group); }
  .pack-row { display: grid; grid-template-columns: minmax(0,1fr) auto; gap: var(--space-2) var(--space-4); padding: var(--space-3) var(--space-4); border-bottom: 1px solid var(--color-border); }
  .pack-row:last-child { border-bottom: none; }
  .pack-main { min-width: 0; align-items: flex-start; }
  .pack-glyph { display: grid; place-items: center; width: 34px; height: 34px; flex: none; border-radius: var(--radius-sm); background: var(--color-surface-raised); color: var(--color-text-secondary); font-weight: 700; }
  .pack-identity { min-width: 0; }
  .pack-identity h4 { margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-body); }
  .pack-meta, .pack-description, .pack-warning { margin: 2px 0 0; font-size: var(--text-metadata); }
  .pack-meta { color: var(--color-text-secondary); }
  .pack-description { color: var(--color-text-muted); }
  .pack-warning { color: var(--color-warning); }
  .pack-actions span { color: var(--color-text-muted); font-size: var(--text-metadata); }
  .pack-details, .pack-confirm { grid-column: 1 / -1; font-size: var(--text-metadata); }
  .pack-details summary { cursor: pointer; color: var(--color-text-secondary); }
  .pack-details dl { margin: var(--space-2) 0; }
  .pack-details dl div { display: flex; gap: var(--space-3); padding: 2px 0; }
  .pack-details dt { min-width: 100px; color: var(--color-text-muted); }
  .pack-details dd { margin: 0; overflow-wrap: anywhere; }
  .pack-confirm { flex-wrap: wrap; padding: var(--space-3); background: var(--color-error-soft); border-radius: var(--radius-sm); }
  .pack-confirm p { margin: 0; flex: 1; }
  .missing-details { margin: 0 0 var(--space-4); font-size: var(--text-metadata); color: var(--color-text-secondary); }
  @media (max-width: 760px) { .packs-heading { align-items: flex-start; flex-wrap: wrap; } .pack-row { grid-template-columns: 1fr; } .pack-actions { justify-content: flex-start; } }
</style>
