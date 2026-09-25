<script lang="ts">
  import {
    getModrinthProject, installModrinth, previewModrinthInstall, searchModrinth,
    LauncherBackendError,
    type ContentType, type ModrinthPreviewResponse,
    type ModrinthProjectDetails, type ModrinthSearchPage,
  } from "$lib/backend";

  let {
    instanceId, instanceName, minecraftVersion, kind, onInstalled,
  }: {
    instanceId: string;
    instanceName: string;
    minecraftVersion: string;
    kind: ContentType;
    onInstalled: () => Promise<void>;
  } = $props();

  let query = $state("");
  let page = $state<ModrinthSearchPage | null>(null);
  let project = $state<ModrinthProjectDetails | null>(null);
  let versionId = $state("");
  let preview = $state<ModrinthPreviewResponse | null>(null);
  let busy = $state<"search" | "details" | "preview" | "install" | null>(null);
  let error = $state<{ code: string; message: string } | null>(null);
  let requestSerial = 0;

  function showError(reason: unknown): void {
    error = reason instanceof LauncherBackendError
      ? { code: reason.code, message: reason.message }
      : { code: "provider_network_error", message: "Modrinth is unavailable. Try again later." };
  }

  async function search(offset = 0): Promise<void> {
    const serial = ++requestSerial;
    busy = "search";
    error = null;
    if (offset === 0) { page = null; project = null; preview = null; }
    try {
      const result = await searchModrinth(instanceId, kind, query, offset);
      if (serial !== requestSerial) return;
      page = offset === 0 || !page
        ? result
        : { ...result, hits: [...page.hits, ...result.hits] };
    } catch (reason) {
      if (serial === requestSerial) showError(reason);
    } finally {
      if (serial === requestSerial) busy = null;
    }
  }

  async function openProject(id: string): Promise<void> {
    const serial = ++requestSerial;
    busy = "details";
    error = null;
    preview = null;
    try {
      const result = await getModrinthProject(instanceId, kind, id);
      if (serial !== requestSerial) return;
      project = result;
      versionId = result.defaultVersionId ?? "";
    } catch (reason) {
      if (serial === requestSerial) showError(reason);
    } finally {
      if (serial === requestSerial) busy = null;
    }
  }

  async function inspectInstall(): Promise<void> {
    if (!project || !versionId) return;
    busy = "preview";
    error = null;
    preview = null;
    try {
      preview = await previewModrinthInstall(instanceId, kind, project.projectId, versionId);
    } catch (reason) {
      showError(reason);
    } finally {
      busy = null;
    }
  }

  async function confirmInstall(): Promise<void> {
    if (!project || !preview) return;
    busy = "install";
    error = null;
    try {
      await installModrinth(
        instanceId, kind, project.projectId,
        preview.preview.versionId, preview.previewFingerprint,
      );
      await onInstalled();
      preview = null;
      project = null;
    } catch (reason) {
      showError(reason);
    } finally {
      busy = null;
    }
  }

  const previewItems = $derived(preview?.preview.items ?? []);
  const installCount = $derived(previewItems.filter((item) => !item.alreadyInstalled).length);
</script>

<section class="browse" aria-label="Browse Modrinth">
  <div class="browse-heading">
    <div>
      <h3 class="group-title">Browse Modrinth</h3>
      <p class="group-subtitle">Results are filtered for this instance's Minecraft version{kind === "mod" ? " and Fabric" : ""}. Version compatibility is checked before installation.</p>
    </div>
    <span class="source">Source: Modrinth</span>
  </div>

  <form class="browse-search" onsubmit={(event) => { event.preventDefault(); void search(); }}>
    <label>
      <span class="field-label">Find {kind === "mod" ? "mods" : kind === "resourcePack" ? "resource packs" : "shaders"}</span>
      <input type="search" bind:value={query} maxlength="160" placeholder="Search Modrinth" />
    </label>
    <button type="submit" class="btn" disabled={busy !== null}>{busy === "search" ? "Searching…" : "Search"}</button>
  </form>

  {#if error}
    <p class="inline-message inline-message-error" role="alert">{error.message} <code>{error.code}</code></p>
  {/if}
  {#if busy && busy !== "install"}<p class="browse-status" role="status"><span class="spinner" aria-hidden="true"></span> {busy === "search" ? "Searching Modrinth…" : busy === "details" ? "Loading compatible versions…" : "Resolving dependencies…"}</p>{/if}
  {#if busy === "install"}<p class="browse-status" role="status"><span class="spinner" aria-hidden="true"></span> Downloading and verifying content…</p>{/if}

  {#if project}
    <div class="project">
      <button type="button" class="btn btn-quiet" onclick={() => { project = null; preview = null; }}>← Results</button>
      <h4>{project.title}</h4>
      <p>{project.summary}</p>
      <p class="browse-meta">License {project.license} · Modrinth project {project.projectId}</p>
      <p class="browse-meta">Minecraft {minecraftVersion} · {project.loaders.join(", ") || "No loader listed"}</p>
      {#if kind === "shaderPack"}<p class="browse-note">The file can be installed. This instance may need a compatible shader loader before Minecraft can use it.</p>{/if}
      {#if project.versions.length}
        <label class="version-choice"><span class="field-label">Compatible version</span>
          <select bind:value={versionId} onchange={() => preview = null}>
            {#each project.versions as version}
              <option value={version.id}>{version.versionNumber} · {version.versionType} · {version.name}</option>
            {/each}
          </select>
        </label>
        <button type="button" class="btn" disabled={busy !== null || !versionId} onclick={inspectInstall}>Review installation</button>
      {:else}
        <p class="browse-note">No compatible version is available for this instance.</p>
      {/if}
    </div>
  {:else if page}
    {#if page.hits.length}
      <div class="browse-list">
        {#each page.hits as hit (hit.projectId)}
          <article class="browse-row">
            <div class="browse-glyph" aria-hidden="true">{kind === "mod" ? "M" : kind === "resourcePack" ? "R" : "S"}</div>
            <div class="browse-copy">
              <h4>{hit.title}</h4>
              <p>{hit.summary}</p>
              <span class="browse-meta">By {hit.author} · {hit.downloads.toLocaleString()} downloads</span>
            </div>
            <button type="button" class="btn btn-quiet" disabled={busy !== null} onclick={() => openProject(hit.projectId)}>Details</button>
          </article>
        {/each}
      </div>
      {#if page.offset + 20 < page.totalHits}
        <button type="button" class="btn btn-quiet more" disabled={busy !== null} onclick={() => search(page ? page.offset + 20 : 0)}>Load more</button>
      {/if}
    {:else}
      <p class="browse-note">No matching projects for this instance.</p>
    {/if}
  {:else if !busy}
    <p class="browse-note">Search Modrinth to find content for this instance.</p>
  {/if}

  {#if preview}
    <div class="preview" role="group" aria-label="Installation preview">
      <h4>Install into {instanceName}</h4>
      <p>{installCount} file{installCount === 1 ? "" : "s"} will be installed. Required dependencies appear below.</p>
      <ul>
        {#each previewItems as item}
          <li><strong>{item.title}</strong> {item.versionNumber} · {item.fileName}{item.alreadyInstalled ? " · already installed" : ""}</li>
        {/each}
      </ul>
      {#each preview.preview.warnings as warning}<p class="browse-note">⚠ {warning}</p>{/each}
      <div class="preview-actions">
        <button type="button" class="btn btn-quiet" disabled={busy !== null} onclick={() => preview = null}>Cancel</button>
        <button type="button" class="btn" disabled={busy !== null || installCount === 0} onclick={confirmInstall}>Install {installCount} file{installCount === 1 ? "" : "s"}</button>
      </div>
    </div>
  {/if}
</section>

<style>
  .browse { min-width: 0; }
  .browse-heading, .browse-search, .browse-row, .preview-actions { display: flex; gap: var(--space-3); align-items: center; }
  .browse-heading { justify-content: space-between; margin-bottom: var(--space-4); }
  .browse-heading .group-subtitle { max-width: 50ch; }
  .source, .browse-meta { font-size: var(--text-metadata); color: var(--color-text-muted); }
  .source { white-space: nowrap; }
  .browse-search { align-items: end; margin-bottom: var(--space-4); }
  .browse-search label, .version-choice { display: grid; gap: var(--space-1); flex: 1; }
  .browse input, .browse select { width: 100%; padding: var(--space-2) var(--space-3); border: 1px solid var(--color-border-strong); border-radius: var(--radius-sm); background: var(--color-surface-sunken); color: var(--color-text); font: inherit; }
  .browse-status { display: flex; align-items: center; gap: var(--space-2); }
  .browse-list { border: 1px solid var(--color-surface-edge); border-radius: var(--radius-lg); background: var(--color-surface); }
  .browse-row { padding: var(--space-3) var(--space-4); border-bottom: 1px solid var(--color-border); }
  .browse-row:last-child { border-bottom: 0; }
  .browse-glyph { display: grid; place-items: center; width: 34px; height: 34px; flex: none; border-radius: var(--radius-sm); background: var(--color-surface-raised); color: var(--color-text-secondary); font-weight: 700; }
  .browse-copy { min-width: 0; flex: 1; }
  .browse-copy h4, .project h4, .preview h4 { margin: 0; }
  .browse-copy p, .project p { margin: 2px 0; font-size: var(--text-metadata); color: var(--color-text-secondary); }
  .browse-note { color: var(--color-text-secondary); font-size: var(--text-metadata); }
  .project, .preview { display: grid; gap: var(--space-3); padding: var(--space-4); border: 1px solid var(--color-surface-edge); border-radius: var(--radius-lg); background: var(--color-surface); }
  .project > .btn, .preview-actions .btn { justify-self: start; }
  .preview { margin-top: var(--space-4); }
  .preview ul { margin: 0; padding-left: var(--space-4); }
  .preview li { margin: var(--space-1) 0; }
  .more { margin-top: var(--space-3); }
  @media (max-width: 760px) { .browse-heading, .browse-row { align-items: flex-start; } .browse-heading { flex-wrap: wrap; } }
</style>
