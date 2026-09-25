<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
</script>

<!--
  Product information: what Aurora Launcher is, its version, the project
  behind it, and the legal disclaimers. Technical diagnostics (managed data
  root, persisted-state details) live on the development-only Developer page.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">About</h2>
      <p class="page-subtitle">Aurora Launcher — the desktop launcher for the Aurora client.</p>
    </div>
  </header>

  <section class="group" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title">Aurora Launcher</h3>
        <p class="group-subtitle">
          A standalone launcher for the Aurora client mod for Minecraft: Java Edition, built around
          isolated installations, transparent behavior, and user control.
        </p>
      </div>
    </div>

    {#if launcher.status}
      <div class="group-row">
        <span class="group-row-title">Version</span>
        <span class="group-row-value">{launcher.status.launcherVersion}</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Platform</span>
        <span class="group-row-value">
          {launcher.status.platform.os} · {launcher.status.platform.architecture}
        </span>
      </div>
    {:else if launcher.statusError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.statusError.message}
      </p>
    {:else}
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <span class="group-row-detail">Requesting status from the native launcher core…</span>
      </div>
    {/if}

    <p class="group-footer">
      Accounts sign in through Microsoft in the system browser, Minecraft and Aurora files are
      integrity-checked before they are used, and instances stay isolated from your normal
      .minecraft installation.
    </p>
  </section>

  <section class="group" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title">Project</h3>
        <p class="group-subtitle">Open source and in active development.</p>
      </div>
    </div>
    <div class="group-row">
      <span class="group-row-title">Source code</span>
      <span class="group-row-value value-mono">github.com/kalibsolomon-pixel/aurora-launcher</span>
    </div>
    <div class="group-row">
      <span class="group-row-title">License</span>
      <span class="group-row-value">MIT</span>
    </div>
    <div class="group-row">
      <div class="group-row-main">
        <span class="group-row-title">Development status</span>
        <span class="group-row-detail">
          The launcher supports installation, managed Java, authentication, and supervised launching.
          Reviewed Aurora Client releases are bundled with each launcher build; this build includes
          Aurora Client 2.1.2 for new production instances.
        </span>
      </div>
    </div>
  </section>

  <section class="group" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title">Legal</h3>
      </div>
    </div>
    <p class="group-footer legal-note">
      Aurora Launcher and Aurora are independent projects and are not affiliated with, endorsed
      by, or sponsored by Microsoft, Mojang Studios, or Fabric. Minecraft is a trademark of
      Microsoft Corporation. Aurora Launcher does not provide Minecraft accounts or bypass
      Minecraft ownership requirements; playing requires your own Microsoft account with a
      legitimate Minecraft: Java Edition entitlement.
    </p>
  </section>
</div>

<style>
  .legal-note {
    padding: var(--space-4);
    color: var(--color-text-secondary);
    font-size: var(--text-secondary);
    line-height: 1.5;
  }
</style>
