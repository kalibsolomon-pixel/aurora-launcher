<script lang="ts">
  import {
    applyModrinthUpdate, applyProviderRemoval, checkModrinthUpdate,
    previewModrinthUpdate, previewProviderRemoval,
    type ContentType, type ModrinthVersionChoice, type ProviderLifecycleEntry,
    type ProviderRemovalPreview, type ProviderUpdatePreview,
  } from "$lib/backend";

  let { instanceId, kind, title, lifecycle, onChanged }: {
    instanceId: string;
    kind: ContentType;
    title: string;
    lifecycle: ProviderLifecycleEntry;
    onChanged: () => Promise<void>;
  } = $props();

  let busy = $state<"check" | "preview" | "update" | "remove" | null>(null);
  let candidate = $state<ModrinthVersionChoice | null>(null);
  let checked = $state(false);
  let updatePreview = $state<ProviderUpdatePreview | null>(null);
  let removalPreview = $state<ProviderRemovalPreview | null>(null);
  let error = $state("");
  let success = $state("");
  const record = $derived(lifecycle.record);
  const requiredBy = $derived(lifecycle.requiredBy);

  async function check(): Promise<void> {
    busy = "check"; error = ""; success = ""; candidate = null; checked = false; updatePreview = null;
    try {
      candidate = await checkModrinthUpdate(instanceId, kind, record.projectId);
      checked = true;
    } catch (reason) {
      error = reason instanceof Error ? reason.message : "Update check failed.";
    } finally { busy = null; }
  }

  async function previewUpdate(): Promise<void> {
    busy = "preview"; error = ""; success = ""; removalPreview = null;
    try {
      updatePreview = await previewModrinthUpdate(instanceId, kind, record.projectId);
    } catch (reason) {
      error = reason instanceof Error ? reason.message : "Update preview failed.";
    } finally { busy = null; }
  }

  async function update(): Promise<void> {
    if (!updatePreview) return;
    busy = "update"; error = "";
    try {
      await applyModrinthUpdate(instanceId, kind, record.projectId, updatePreview.previewFingerprint);
      updatePreview = null; candidate = null; checked = false;
      await onChanged();
      success = `${title} updated.`;
    } catch (reason) {
      error = reason instanceof Error ? reason.message : "Update failed. The previous installation was retained.";
    } finally { busy = null; }
  }

  async function previewRemove(): Promise<void> {
    busy = "preview"; error = ""; success = ""; updatePreview = null;
    try {
      removalPreview = await previewProviderRemoval(instanceId, kind, record.projectId);
    } catch (reason) {
      error = reason instanceof Error ? reason.message : "Removal preview failed.";
    } finally { busy = null; }
  }

  async function remove(): Promise<void> {
    if (!removalPreview) return;
    busy = "remove"; error = "";
    try {
      await applyProviderRemoval(instanceId, kind, record.projectId, removalPreview.previewFingerprint);
      removalPreview = null; updatePreview = null;
      await onChanged();
      success = `${title} removal completed.`;
    } catch (reason) {
      error = reason instanceof Error ? reason.message : "Removal failed. Installed content was retained.";
    } finally { busy = null; }
  }
</script>

<div class="provider-lifecycle" aria-label={`Modrinth lifecycle for ${title}`}>
  <div class="lifecycle-summary">
    <span>{record.explicitlyRetained ? "Managed" : "Dependency"}</span>
    <span>· {record.displayVersion ?? record.versionId}</span>
    {#if requiredBy.length}<span>· Required by {requiredBy.map((parent) => parent.fileName).join(", ")}</span>{/if}
  </div>
  <details class="relationships">
    <summary>Dependency details</summary>
    <p>{record.explicitlyRetained ? "Explicitly retained" : "Installed as a required dependency"} via Modrinth.</p>
    <p>Requires: {lifecycle.requires.length ? lifecycle.requires.map((item) => item.fileName).join(", ") : "No provider-managed dependencies"}</p>
    <p>Required by: {requiredBy.length ? requiredBy.map((parent) => parent.fileName).join(", ") : "No installed provider content"}</p>
  </details>
  <div class="lifecycle-actions">
    {#if record.explicitlyRetained}
      <button type="button" class="btn btn-quiet" disabled={busy !== null} onclick={check}>{busy === "check" ? "Checking…" : "Check for updates"}</button>
      {#if checked}<span role="status">{candidate ? `Update available: ${candidate.versionNumber} (${candidate.versionType})` : "Up to date"}</span>{/if}
      {#if candidate}<button type="button" class="btn btn-quiet" disabled={busy !== null} onclick={previewUpdate}>Update…</button>{/if}
    {/if}
    {#if record.explicitlyRetained || !requiredBy.length}
      <button type="button" class="btn btn-quiet" disabled={busy !== null} onclick={previewRemove}>Remove…</button>
    {/if}
  </div>
  {#if error}<p class="lifecycle-error" role="alert">{error}</p>{/if}
  {#if success}<p role="status">{success}</p>{/if}

  {#if updatePreview}
    <div class="lifecycle-preview" role="group" aria-label={`Update ${title} preview`}>
      <strong>{title}: {updatePreview.current.displayVersion ?? "installed"} → {updatePreview.candidate.versionNumber}</strong>
      {#if updatePreview.delta.willInstall.length}<p>Will install: {updatePreview.delta.willInstall.map((item) => item.fileName).join(", ")}</p>{/if}
      {#if updatePreview.delta.newRequirements.length}<p>New dependencies: {updatePreview.delta.newRequirements.map((item) => item.fileName).join(", ")}</p>{/if}
      {#if updatePreview.delta.removedRequirements.length}<p>No longer required: {updatePreview.delta.removedRequirements.map((item) => item.fileName).join(", ")}</p>{/if}
      {#if !updatePreview.delta.newRequirements.length && !updatePreview.delta.removedRequirements.length}<p>Required dependencies: unchanged.</p>{/if}
      {#if updatePreview.delta.willRemove.length}<p>Will remove: {updatePreview.delta.willRemove.map((item) => item.fileName).join(", ")}</p>{/if}
      {#if updatePreview.delta.willRetain.length}<p>Retained: {updatePreview.delta.willRetain.map((item) => item.fileName).join(", ")}</p>{/if}
      {#if updatePreview.warnings.length}<p>Notes: {updatePreview.warnings.join(" ")}</p>{/if}
      <div class="preview-actions">
        <button type="button" class="btn btn-quiet" disabled={busy !== null} onclick={() => updatePreview = null}>Cancel</button>
        <button type="button" class="btn" disabled={busy !== null} onclick={update}>{busy === "update" ? "Updating…" : "Approve update"}</button>
      </div>
    </div>
  {/if}
  {#if removalPreview}
    <div class="lifecycle-preview" role="group" aria-label={`Remove ${title} preview`}>
      <strong>{removalPreview.delta.willRemove.length ? `Remove ${title}?` : `Stop explicitly retaining ${title}?`}</strong>
      {#if !removalPreview.delta.willRemove.length && requiredBy.length}<p>The file stays installed because {requiredBy.map((parent) => parent.fileName).join(", ")} still requires it.</p>{/if}
      {#if removalPreview.delta.willRemove.length}<p>Will remove: {removalPreview.delta.willRemove.map((item) => item.fileName).join(", ")}</p>{/if}
      {#if removalPreview.delta.willRetain.length}<p>Retained because still required or explicitly installed: {removalPreview.delta.willRetain.map((item) => item.fileName).join(", ")}</p>{/if}
      <div class="preview-actions">
        <button type="button" class="btn btn-quiet" disabled={busy !== null} onclick={() => removalPreview = null}>Cancel</button>
        <button type="button" class="btn btn-danger" disabled={busy !== null} onclick={remove}>{busy === "remove" ? "Removing…" : "Approve removal"}</button>
      </div>
    </div>
  {/if}
</div>

<style>
  .provider-lifecycle { display: grid; gap: var(--space-2); margin-top: var(--space-2); font-size: var(--text-metadata); color: var(--color-text-secondary); }
  .lifecycle-summary, .lifecycle-actions, .preview-actions { display: flex; align-items: center; gap: var(--space-2); flex-wrap: wrap; }
  .relationships summary { cursor: pointer; color: var(--color-text-secondary); }
  .relationships p, .lifecycle-preview p { margin: var(--space-1) 0; }
  .lifecycle-preview { padding: var(--space-3); border: 1px solid var(--color-border); border-radius: var(--radius-sm); background: var(--color-surface-raised); }
  .preview-actions { margin-top: var(--space-2); }
  .lifecycle-error { color: var(--color-error); }
</style>
