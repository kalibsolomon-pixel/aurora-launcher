<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
</script>

<!--
  Existing launcher status and persisted-state surfaces, intentionally not
  redesigned in the shell/Home pilot: the internal content keeps its
  established presentation until its own design phase.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">About</h2>
      <p class="page-subtitle">Launcher version, platform, and persisted state.</p>
    </div>
  </header>

  <section class="status-card" aria-labelledby="status-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Native connection</p>
        <h3 id="status-title">Launcher status</h3>
      </div>

      {#if launcher.status}
        <span class="badge ready"><span aria-hidden="true"></span>Ready</span>
      {:else if launcher.statusError}
        <span class="badge error"><span aria-hidden="true"></span>Unavailable</span>
      {:else}
        <span class="badge loading"><span aria-hidden="true"></span>Connecting</span>
      {/if}
    </div>

    {#if launcher.status}
      <dl>
        <div>
          <dt>Launcher version</dt>
          <dd>{launcher.status.launcherVersion}</dd>
        </div>
        <div>
          <dt>Platform</dt>
          <dd>{launcher.status.platform.os} / {launcher.status.platform.architecture}</dd>
        </div>
        <div class="path-row">
          <dt>Managed data root</dt>
          <dd>{launcher.status.managedDataRoot}</dd>
        </div>
      </dl>
      <p class="footnote">No Minecraft installation data is accessed in this phase.</p>
    {:else if launcher.statusError}
      <div class="error-message" role="alert">
        <p>{launcher.statusError.message}</p>
        <code>{launcher.statusError.code}</code>
      </div>
    {:else}
      <div class="loading-message">
        <span class="spinner" aria-hidden="true"></span>
        <p>Requesting status from the native launcher core…</p>
      </div>
    {/if}
  </section>

  <section class="status-card" aria-labelledby="state-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Persisted model</p>
        <h3 id="state-title">Launcher state</h3>
      </div>

      {#if launcher.launcherState}
        <span class="badge ready"><span aria-hidden="true"></span>Loaded</span>
      {:else if launcher.stateError}
        <span class="badge error"><span aria-hidden="true"></span>Unavailable</span>
      {:else}
        <span class="badge loading"><span aria-hidden="true"></span>Loading</span>
      {/if}
    </div>

      {#if launcher.launcherState}
      <dl>
        <div>
          <dt>Config schema version</dt>
          <dd>{launcher.launcherState.config.schemaVersion}</dd>
        </div>
        <div>
          <dt>Selected instance</dt>
          <dd>{launcher.launcherState.config.selectedInstanceId ?? "None"}</dd>
        </div>
        <div>
          <dt>Known instances</dt>
          <dd>{launcher.launcherState.instances.length}</dd>
        </div>
      </dl>
      <p class="footnote">
        Instance management and Rust-owned Play readiness live in Instances and Home. Aurora
        starts only from validated content, the exact managed Java runtime, and a usable
        authenticated session.
      </p>
    {:else if launcher.stateError}
      <div class="error-message" role="alert">
        <p>{launcher.stateError.message}</p>
        <code>{launcher.stateError.code}</code>
      </div>
    {:else}
      <div class="loading-message">
        <span class="spinner" aria-hidden="true"></span>
        <p>Loading persisted launcher state…</p>
      </div>
    {/if}
  </section>
</div>

<style>
  .status-card {
    overflow: hidden;
    border: 1px solid #252b3f;
    border-radius: 16px;
    background: rgba(18, 22, 35, 0.84);
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.24);
  }

  .status-card + .status-card {
    margin-top: 1.5rem;
  }

  .status-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 1.4rem 1.5rem;
    border-bottom: 1px solid #252b3f;
  }

  .eyebrow {
    margin: 0 0 0.45rem;
    color: #a99dff;
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  h3 {
    margin: 0;
    font-size: 1.25rem;
    letter-spacing: -0.02em;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.42rem 0.7rem;
    border-radius: 999px;
    font-size: 0.8rem;
    font-weight: 700;
  }

  .badge span {
    width: 0.46rem;
    height: 0.46rem;
    border-radius: 50%;
    background: currentColor;
  }

  .ready {
    color: #78e6ba;
    background: rgba(50, 172, 125, 0.12);
  }

  .error {
    color: #ff9a9a;
    background: rgba(201, 68, 68, 0.13);
  }

  .loading {
    color: #b8c0d3;
    background: rgba(132, 143, 168, 0.11);
  }

  dl {
    margin: 0;
  }

  dl div {
    display: grid;
    grid-template-columns: minmax(9rem, 0.55fr) minmax(0, 1fr);
    gap: 1.5rem;
    padding: 1rem 1.5rem;
    border-bottom: 1px solid #20263a;
  }

  dt {
    color: #818ca4;
    font-size: 0.86rem;
  }

  dd {
    min-width: 0;
    margin: 0;
    color: #e7ecf7;
    font-size: 0.9rem;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .footnote,
  .loading-message,
  .error-message {
    margin: 0;
    padding: 1rem 1.5rem;
    color: #818ca4;
    font-size: 0.82rem;
  }

  .loading-message {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    min-height: 8rem;
  }

  .loading-message p,
  .error-message p {
    margin-bottom: 0;
  }

  .spinner {
    width: 1rem;
    height: 1rem;
    border: 2px solid #394158;
    border-top-color: #a99dff;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  .error-message code {
    display: inline-block;
    margin-top: 0.7rem;
    color: #ff9a9a;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (max-width: 620px) {
    dl div {
      grid-template-columns: 1fr;
      gap: 0.3rem;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
    }
  }
</style>
