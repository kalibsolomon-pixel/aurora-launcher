<script lang="ts">
  import { onMount } from "svelte";
  import { appearance } from "$lib/launcher/appearance.svelte";
  import { desktopIntegration } from "$lib/launcher/desktopIntegration.svelte";
  import type { AccentSelection, ShortcutStatus } from "$lib/backend";

  const appearanceState = $derived(appearance.state);
  const themes = $derived(appearanceState?.themes ?? []);
  const accents = $derived(appearanceState?.accents ?? []);
  const isCustomAccent = $derived(appearance.accent.type === "custom");
  const integrationState = $derived(desktopIntegration.state);

  // The custom picker's resting value: the active custom color, or a
  // reasonable starting point when a preset is active.
  let customHex = $state("#8b80ff");
  $effect(() => {
    const accent = appearance.accent;
    if (accent.type === "custom") customHex = accent.hex;
  });

  // Shortcut status is live OS state: query it whenever Settings is shown so
  // a shortcut deleted outside Aurora is reflected immediately.
  onMount(() => {
    void desktopIntegration.refresh();
  });

  function selectTheme(event: Event): void {
    const value = (event.currentTarget as HTMLInputElement).value;
    void appearance.setTheme(value);
  }

  function selectPresetAccent(event: Event): void {
    const value = (event.currentTarget as HTMLInputElement).value;
    void appearance.setAccent({ type: "preset", id: value });
  }

  function selectCustomAccent(event: Event): void {
    const value = (event.currentTarget as HTMLInputElement).value;
    void appearance.setAccent({ type: "custom", hex: value });
  }

  const desktopStatusText = $derived.by(() => {
    const status = integrationState?.desktopShortcut;
    if (!status || status.state === "unknown") {
      return "Status unavailable right now.";
    }
    switch (status.state) {
      case "present":
        return "Aurora's shortcut is on the desktop.";
      case "absent":
        return "No Aurora shortcut on the desktop.";
      case "conflict":
        return "Another item is already using this name, so Aurora left it untouched.";
    }
  });

  const startMenuStatusText = $derived.by(() => {
    const status = integrationState?.startMenuShortcut;
    if (!status || status.state === "unknown") {
      return "Status unavailable right now.";
    }
    switch (status.state) {
      case "present":
        return "Present — created and removed by the Aurora installer.";
      case "absent":
        return "Created when Aurora is installed with its setup.";
      case "conflict":
        return "An item is using the Aurora name, but Aurora did not create it.";
    }
  });

  function shortcutStatusLabel(status: ShortcutStatus | undefined): string {
    if (!status || status.state === "unknown") return "Unknown";
    switch (status.state) {
      case "present":
        return "Present";
      case "absent":
        return "Not present";
      case "conflict":
        return "Name in use";
    }
  }
</script>

<!--
  Launcher-wide preferences. Appearance and Windows desktop integration are
  the implemented preferences; the page stays sparse and purposeful rather
  than inventing settings; future launcher-wide preferences belong here.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">Settings</h2>
      <p class="page-subtitle">Launcher-wide preferences.</p>
    </div>
  </header>

  <section class="group" aria-labelledby="appearance-title">
    <div class="group-heading">
      <div>
        <h3 class="group-title" id="appearance-title">Appearance</h3>
        <p class="group-subtitle">
          Applies immediately and is remembered across restarts.
        </p>
      </div>
    </div>

    {#if appearanceState === null && appearance.error === null}
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <span class="group-row-detail">Loading appearance…</span>
      </div>
    {:else}
      <div class="group-row appearance-row">
        <div class="group-row-main">
          <span class="group-row-title">Theme</span>
          <span class="group-row-detail">
            Every theme is dark; they differ through their surface palette.
          </span>
        </div>
        <fieldset class="theme-picker" role="radiogroup" aria-label="Theme">
          {#each themes as theme (theme.id)}
            <label class="theme-option">
              <input
                type="radio"
                name="theme"
                value={theme.id}
                checked={appearance.theme === theme.id}
                onchange={selectTheme}
                disabled={appearance.busy}
              />
              <span class="theme-option-text">
                <span class="theme-option-name">{theme.label}</span>
                <span class="theme-option-description">{theme.description}</span>
              </span>
            </label>
          {/each}
        </fieldset>
      </div>

      <div class="group-row appearance-row">
        <div class="group-row-main">
          <span class="group-row-title">Accent</span>
          <span class="group-row-detail">
            Colors selection, focus, and primary actions — status colors stay
            semantic.
          </span>
        </div>
        <div class="accent-picker" role="radiogroup" aria-label="Accent color">
          {#each accents as accent (accent.id)}
            {@const selected =
              appearance.accent.type === "preset" && appearance.accent.id === accent.id}
            <label class="accent-option">
              <input
                type="radio"
                name="accent"
                value={accent.id}
                checked={selected}
                onchange={selectPresetAccent}
                disabled={appearance.busy}
              />
              <span
                class="accent-swatch"
                class:accent-swatch-selected={selected}
                style={`background: ${accent.hex}`}
                aria-hidden="true"
              ></span>
              <span class="accent-option-name">
                {selected ? "✓ " : ""}{accent.label}
              </span>
            </label>
          {/each}
          <label class="accent-option accent-option-custom">
            <input
              type="radio"
              name="accent"
              value="custom"
              checked={isCustomAccent}
              onchange={() => void appearance.setAccent({ type: "custom", hex: customHex })}
              disabled={appearance.busy}
            />
            <span class="accent-custom-controls">
              <input
                type="color"
                class="accent-color-input"
                value={customHex}
                onchange={selectCustomAccent}
                disabled={appearance.busy}
                aria-label="Custom accent color"
              />
            </span>
            <span class="accent-option-name">
              {isCustomAccent ? "✓ " : ""}Custom
            </span>
          </label>
        </div>
      </div>
    {/if}

    {#if appearance.error}
      <p class="inline-message inline-message-error group-row" role="alert">
        {appearance.error.message}
      </p>
    {/if}

    <p class="group-footer">
      The accent affects selected navigation, focus rings, selected states, and
      primary buttons. Success, warning, and error colors keep their meaning in
      every theme.
    </p>
  </section>

  <section class="group" aria-labelledby="desktop-integration-title">
    <div class="group-heading">
      <div>
        <h3 class="group-title" id="desktop-integration-title">
          Desktop integration
        </h3>
        <p class="group-subtitle">
          Windows shortcuts for launching Aurora, reflecting the system as it
          is right now.
        </p>
      </div>
    </div>

    {#if integrationState === null && desktopIntegration.error === null}
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <span class="group-row-detail">Reading shortcut status…</span>
      </div>
    {:else if integrationState !== null && !integrationState.supported}
      <div class="group-row">
        <div class="group-row-main">
          <span class="group-row-title">Windows shortcuts</span>
          <span class="group-row-detail">
            Shortcut integration is not available on this platform.
          </span>
        </div>
      </div>
    {:else if integrationState !== null}
      <div class="group-row integration-row">
        <div class="group-row-main">
          <span class="group-row-title">Desktop shortcut</span>
          <span class="group-row-detail" role="status">
            {desktopStatusText}
          </span>
          {#if integrationState.desktopShortcut.state === "conflict"}
            <span class="status-badge status-warning">
              {shortcutStatusLabel(integrationState.desktopShortcut)}
            </span>
          {/if}
        </div>
        <div class="group-row-actions">
          {#if !integrationState.manageable}
            <span class="integration-note">Requires an installed production build.</span>
          {:else if integrationState.desktopShortcut.state === "absent"}
            <button
              type="button"
              class="btn"
              onclick={() => void desktopIntegration.create()}
              disabled={desktopIntegration.busy}
            >
              Create desktop shortcut
            </button>
          {:else if integrationState.desktopShortcut.state === "present"}
            <button
              type="button"
              class="btn btn-danger"
              onclick={() => void desktopIntegration.remove()}
              disabled={desktopIntegration.busy}
            >
              Remove desktop shortcut
            </button>
          {/if}
        </div>
      </div>

      <div class="group-row integration-row">
        <div class="group-row-main">
          <span class="group-row-title">Start menu</span>
          <span class="group-row-detail" role="status">
            {startMenuStatusText}
          </span>
        </div>
        <div class="group-row-actions">
          {#if integrationState.startMenuShortcut.state === "present"}
            <span class="integration-note">Managed by the installer.</span>
          {/if}
        </div>
      </div>
    {/if}

    {#if desktopIntegration.error}
      <p class="inline-message inline-message-error group-row" role="alert">
        {desktopIntegration.error.message}
      </p>
    {/if}

    <p class="group-footer">
      Aurora only manages shortcuts it created; anything else with the same
      name is left alone. Pinning Aurora to the taskbar stays a Windows choice.
    </p>
  </section>
</div>

<style>
  .appearance-row {
    align-items: flex-start;
  }

  /* Desktop-integration rows: the status text wraps under the title on
     narrow widths while actions keep their own line. */
  .integration-row {
    align-items: center;
  }

  .integration-note {
    font-size: var(--text-metadata);
    color: var(--color-text-muted);
  }

  /* Theme choices: one stacked radio per built-in theme. */
  .theme-picker {
    display: grid;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    border: none;
    min-width: 0;
    flex: none;
  }

  .theme-option {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    cursor: pointer;
    transition: background-color var(--motion-fast) var(--motion-ease);
  }

  .theme-option:hover {
    background: var(--color-surface-raised);
  }

  .theme-option input {
    margin: 0;
    margin-top: 3px;
    flex: none;
  }

  .theme-option-text {
    display: grid;
    gap: 1px;
    min-width: 0;
  }

  .theme-option-name {
    font-size: var(--text-body);
    font-weight: 500;
    color: var(--color-text);
  }

  .theme-option-description {
    font-size: var(--text-secondary);
    color: var(--color-text-secondary);
  }

  /* Accent choices: a wrapped row of swatch radios with text labels, so the
     selection is never communicated by color alone (checked radio, ✓ in the
     label, and a ring on the swatch). */
  .accent-picker {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-3) var(--space-4);
    min-width: 0;
    flex: none;
    max-width: 100%;
  }

  .accent-option {
    display: grid;
    justify-items: center;
    gap: var(--space-1);
    width: 76px;
    cursor: pointer;
  }

  /* Keep the native radio reachable and visible for focus, but let the
     swatch carry the visual weight. */
  .accent-option input[type="radio"] {
    margin: 0;
  }

  .accent-swatch {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    border: 1px solid var(--color-border-strong);
    /* An offset gap keeps the ring readable on any swatch color. */
    box-shadow: 0 0 0 2px var(--color-surface);
  }

  .accent-swatch-selected {
    box-shadow:
      0 0 0 2px var(--color-surface),
      0 0 0 4px var(--color-accent);
  }

  .accent-option-name {
    font-size: var(--text-metadata);
    color: var(--color-text-secondary);
    text-align: center;
    overflow-wrap: anywhere;
  }

  .accent-custom-controls {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
  }

  .accent-color-input {
    width: 26px;
    height: 26px;
    padding: 0;
    border: 1px solid var(--color-border-strong);
    border-radius: 50%;
    background: var(--color-surface-sunken);
    cursor: pointer;
  }

  .accent-color-input::-webkit-color-swatch-wrapper {
    padding: 2px;
  }

  .accent-color-input::-webkit-color-swatch {
    border: none;
    border-radius: 50%;
  }

  @media (max-width: 800px) {
    .appearance-row {
      flex-direction: column;
      align-items: stretch;
    }

    .theme-picker,
    .accent-picker {
      width: 100%;
    }

    .integration-row {
      flex-direction: column;
      align-items: flex-start;
      gap: var(--space-2);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .theme-option {
      transition: none;
    }
  }
</style>
