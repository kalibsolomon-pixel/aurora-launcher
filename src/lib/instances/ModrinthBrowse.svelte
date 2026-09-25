<script module lang="ts">
  import { DefaultBrowseCache } from "./modrinthBrowse";
  const browseDefaultPages = new DefaultBrowseCache();
  const failedIconUrls = new Set<string>();
</script>

<script lang="ts">
  import { onDestroy } from "svelte";
  import { appendBrowsePage, TransientNotice } from "./modrinthBrowse";
  import {
    getModrinthProject, installModrinth, previewModrinthInstall, quickInstallModrinth, searchModrinth,
    LauncherBackendError,
    type ContentType, type ModrinthPreviewResponse,
    type ModrinthProjectDetails, type ModrinthSearchPage,
  } from "$lib/backend";

  let {
    instanceId, instanceName, minecraftVersion, kind, installedProjectIds, onInstalled,
  }: {
    instanceId: string;
    instanceName: string;
    minecraftVersion: string;
    kind: ContentType;
    installedProjectIds: string[];
    onInstalled: (targetInstanceId: string, targetKind: ContentType) => Promise<void>;
  } = $props();

  let query = $state("");
  let page = $state<ModrinthSearchPage | null>(null);
  let project = $state<ModrinthProjectDetails | null>(null);
  let versionId = $state("");
  let preview = $state<ModrinthPreviewResponse | null>(null);
  let busy = $state<"search" | "details" | "preview" | "install" | null>(null);
  let error = $state<{ code: string; message: string } | null>(null);
  let nextOffset = $state(0);
  let quickBusyProjectId = $state<string | null>(null);
  let notice = $state("");
  let noticeExiting = $state(false);
  let failedIcons = $state<Record<string, true>>({});
  let requestSerial = 0;

  // A brief in-memory reuse avoids fetching the same default page on a quick
  // Installed → Browse round trip. Instance and domain are part of the key.
  const defaultPages = browseDefaultPages;
  const notifications = new TransientNotice((message, exiting) => { notice = message; noticeExiting = exiting; });

  onDestroy(() => {
    requestSerial++;
    notifications.dispose();
  });

  function showError(reason: unknown): void {
    error = reason instanceof LauncherBackendError
      ? { code: reason.code, message: reason.message }
      : { code: "provider_network_error", message: "Modrinth is unavailable. Try again later." };
  }

  async function search(offset = 0, term = query): Promise<void> {
    const serial = ++requestSerial;
    busy = "search";
    error = null;
    if (offset === 0) { page = null; nextOffset = 0; project = null; preview = null; }
    try {
      const result = await searchModrinth(instanceId, kind, term, offset);
      if (serial !== requestSerial) return;
      page = appendBrowsePage(page, result, offset);
      nextOffset = result.offset + 20;
      if (offset === 0 && !term.trim()) defaultPages.put(instanceId, kind, minecraftVersion, result);
    } catch (reason) {
      if (serial === requestSerial) showError(reason);
    } finally {
      if (serial === requestSerial) busy = null;
    }
  }

  $effect(() => {
    ++requestSerial;
    query = "";
    page = null;
    project = null;
    preview = null;
    error = null;
    const cached = defaultPages.get(instanceId, kind, minecraftVersion);
    if (cached) {
      page = cached;
      nextOffset = 20;
      busy = null;
    } else {
      void search(0, "");
    }
  });

  function quickMessage(reason: unknown): string {
    if (!(reason instanceof LauncherBackendError)) return "Modrinth is unavailable. Try again later.";
    switch (reason.code) {
      case "provider_no_compatible_version": return "No compatible version is available for this instance.";
      case "provider_content_collision": return "This project conflicts with installed or local content. Review Details.";
      case "provider_rate_limited": return "Modrinth's rate limit was reached. Try again shortly.";
      case "provider_network_error": return "Modrinth is unavailable. Try again later.";
      case "provider_dependency_unresolved": return "A required dependency has no compatible version.";
      case "provider_dependency_cycle": return "This project's required dependencies contain a cycle.";
      case "provider_integrity_failure": return "The download failed verification. Nothing was installed.";
      default: return reason.message;
    }
  }

  async function quickInstall(projectId: string, title: string): Promise<void> {
    if (quickBusyProjectId || installedProjectIds.includes(projectId)) return;
    const targetInstanceId = instanceId;
    const targetKind = kind;
    quickBusyProjectId = projectId;
    try {
      const installed = await quickInstallModrinth(targetInstanceId, targetKind, projectId);
      if (installed.length) {
        await onInstalled(targetInstanceId, targetKind);
        if (instanceId === targetInstanceId && kind === targetKind) notifications.show(`${title} installed. View it under Installed.`);
      } else {
        if (instanceId === targetInstanceId && kind === targetKind) notifications.show(`${title} is already installed.`);
      }
    } catch (reason) {
      if (instanceId === targetInstanceId && kind === targetKind) notifications.show(quickMessage(reason));
    } finally {
      quickBusyProjectId = null;
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
    const targetInstanceId = instanceId;
    const targetKind = kind;
    busy = "install";
    error = null;
    try {
      await installModrinth(
        targetInstanceId, targetKind, project.projectId,
        preview.preview.versionId, preview.previewFingerprint,
      );
      await onInstalled(targetInstanceId, targetKind);
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
      <input type="search" bind:value={query} oninput={(event) => { if (!event.currentTarget.value.trim()) void search(0, ""); }} maxlength="160" placeholder="Search Modrinth" />
    </label>
    <button type="submit" class="btn" disabled={busy !== null}>{busy === "search" ? "Searching…" : "Search"}</button>
  </form>

  {#if error}
    <p class="inline-message inline-message-error" role="alert">{error.message} <code>{error.code}</code></p>
  {/if}
  {#if busy && busy !== "install"}<p class="browse-status" class:browse-loading={busy === "search" && !page} role="status"><span class="spinner" aria-hidden="true"></span> {busy === "search" ? "Loading Modrinth projects…" : busy === "details" ? "Loading compatible versions…" : "Resolving dependencies…"}</p>{/if}
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
            <div class="browse-glyph" aria-hidden="true">
              {#if hit.iconUrl && !failedIconUrls.has(hit.iconUrl) && !failedIcons[hit.projectId]}
                <img src={hit.iconUrl} alt="" loading="lazy" onerror={() => { if (hit.iconUrl) failedIconUrls.add(hit.iconUrl); failedIcons = { ...failedIcons, [hit.projectId]: true }; }} />
              {:else}
                {kind === "mod" ? "M" : kind === "resourcePack" ? "R" : "S"}
              {/if}
            </div>
            <div class="browse-copy">
              <h4>{hit.title}</h4>
              <p>{hit.summary}</p>
              <span class="browse-meta">By {hit.author} · {hit.downloads.toLocaleString()} downloads</span>
            </div>
            <div class="browse-actions">
              <button type="button" class="btn btn-quiet install-action" title={installedProjectIds.includes(hit.projectId) ? `${hit.title} is installed` : `Install latest compatible version of ${hit.title}`} aria-label={installedProjectIds.includes(hit.projectId) ? `${hit.title} is installed` : quickBusyProjectId === hit.projectId ? `Installing ${hit.title}` : `Install latest compatible version of ${hit.title}`} disabled={quickBusyProjectId !== null || installedProjectIds.includes(hit.projectId)} onclick={() => quickInstall(hit.projectId, hit.title)}>
                {#if quickBusyProjectId === hit.projectId}<span class="spinner" aria-hidden="true"></span><span class="action-state">Installing…</span>{:else if installedProjectIds.includes(hit.projectId)}<span class="action-state">Installed</span>{:else}<span aria-hidden="true">↓</span>{/if}
              </button>
              <button type="button" class="btn btn-quiet" disabled={busy !== null} onclick={() => openProject(hit.projectId)}>Details</button>
            </div>
          </article>
        {/each}
      </div>
    {:else}
      <p class="browse-note">{page.totalHits > 0 ? "No compatible projects on this page." : "No matching projects for this instance."}</p>
    {/if}
    {#if nextOffset < page.totalHits}
      <button type="button" class="btn btn-quiet more" disabled={busy !== null} onclick={() => search(nextOffset)}>Load more</button>
    {/if}
  {/if}

  {#if notice}<div class="browse-notice" class:exiting={noticeExiting} role="status" aria-live="polite">{notice}</div>{/if}

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
  .browse-loading { min-height: 96px; }
  .browse-list { border: 1px solid var(--color-surface-edge); border-radius: var(--radius-lg); background: var(--color-surface); }
  .browse-row { display: flex; align-items: center; gap: var(--space-3); padding: var(--space-3) var(--space-4); border-bottom: 1px solid var(--color-border); }
  .browse-row:last-child { border-bottom: 0; }
  .browse-glyph { display: grid; place-items: center; width: 34px; height: 34px; flex: none; overflow: hidden; border-radius: var(--radius-sm); background: var(--color-surface-raised); color: var(--color-text-secondary); font-weight: 700; }
  .browse-glyph img { display: block; width: 100%; height: 100%; object-fit: cover; }
  .browse-actions { display: flex; align-items: center; gap: var(--space-2); flex: none; }
  .install-action { display: flex; align-items: center; justify-content: center; gap: var(--space-1); min-width: 34px; min-height: 32px; font-size: 19px; line-height: 1; }
  .action-state { font-size: var(--text-metadata); }
  .browse-notice { position: fixed; z-index: 20; right: var(--space-4); bottom: var(--space-4); max-width: min(360px, calc(100vw - 32px)); padding: var(--space-3) var(--space-4); border: 1px solid var(--color-surface-edge); border-radius: var(--radius-md); background: var(--color-surface-raised); box-shadow: var(--shadow-group); color: var(--color-text); font-size: var(--text-secondary); opacity: 1; transition: opacity 300ms ease-out; }
  .browse-notice.exiting { opacity: 0; }
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
  @media (prefers-reduced-motion: reduce) { .browse-notice { transition: none; } }
</style>
