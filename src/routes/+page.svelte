<script lang="ts">
  import { onMount } from "svelte";
  import {
    getApplicationStatus,
    LauncherBackendError,
    type ApplicationStatus,
  } from "$lib/backend";

  let status = $state<ApplicationStatus | null>(null);
  let error = $state<LauncherBackendError | null>(null);

  onMount(async () => {
    try {
      status = await getApplicationStatus();
    } catch (cause: unknown) {
      error =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The launcher status could not be loaded.");
    }
  });
</script>

<svelte:head>
  <title>Aurora Launcher</title>
</svelte:head>

<main>
  <section class="hero" aria-labelledby="app-title">
    <p class="eyebrow">A lightweight home for Aurora</p>
    <h1 id="app-title">Aurora Launcher</h1>
    <p class="summary">
      A clean, isolated foundation for the Aurora client mod for Minecraft: Java Edition.
    </p>
  </section>

  <section class="status-card" aria-labelledby="status-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Native connection</p>
        <h2 id="status-title">Launcher status</h2>
      </div>

      {#if status}
        <span class="badge ready"><span aria-hidden="true"></span>Ready</span>
      {:else if error}
        <span class="badge error"><span aria-hidden="true"></span>Unavailable</span>
      {:else}
        <span class="badge loading"><span aria-hidden="true"></span>Connecting</span>
      {/if}
    </div>

    {#if status}
      <dl>
        <div>
          <dt>Launcher version</dt>
          <dd>{status.launcherVersion}</dd>
        </div>
        <div>
          <dt>Platform</dt>
          <dd>{status.platform.os} / {status.platform.architecture}</dd>
        </div>
        <div class="path-row">
          <dt>Managed data root</dt>
          <dd>{status.managedDataRoot}</dd>
        </div>
      </dl>
      <p class="footnote">No Minecraft installation or account data is accessed in this phase.</p>
    {:else if error}
      <div class="error-message" role="alert">
        <p>{error.message}</p>
        <code>{error.code}</code>
      </div>
    {:else}
      <div class="loading-message">
        <span class="spinner" aria-hidden="true"></span>
        <p>Requesting status from the native launcher core…</p>
      </div>
    {/if}
  </section>
</main>

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(html) {
    min-width: 320px;
    color: #eef4ff;
    background: #0b0e16;
    font-family:
      Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    font-synthesis: none;
    text-rendering: optimizeLegibility;
  }

  :global(body) {
    min-width: 320px;
    min-height: 100vh;
    margin: 0;
    background:
      radial-gradient(circle at 78% 8%, rgba(112, 93, 242, 0.16), transparent 30rem),
      linear-gradient(145deg, #0b0e16 0%, #111524 100%);
  }

  main {
    width: min(100% - 3rem, 860px);
    min-height: 100vh;
    margin: 0 auto;
    padding: clamp(3rem, 10vh, 6.5rem) 0 3rem;
  }

  .hero {
    max-width: 690px;
    margin-bottom: 2.25rem;
  }

  .eyebrow {
    margin: 0 0 0.45rem;
    color: #a99dff;
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  h1,
  h2,
  p {
    margin-top: 0;
  }

  h1 {
    margin-bottom: 0.7rem;
    font-size: clamp(2.6rem, 7vw, 4.6rem);
    line-height: 0.98;
    letter-spacing: -0.055em;
  }

  h2 {
    margin-bottom: 0;
    font-size: 1.25rem;
    letter-spacing: -0.02em;
  }

  .summary {
    margin-bottom: 0;
    color: #aab5ca;
    font-size: 1.05rem;
    line-height: 1.65;
  }

  .status-card {
    overflow: hidden;
    border: 1px solid #252b3f;
    border-radius: 16px;
    background: rgba(18, 22, 35, 0.84);
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.24);
  }

  .status-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 1.4rem 1.5rem;
    border-bottom: 1px solid #252b3f;
  }

  .status-heading .eyebrow {
    margin-bottom: 0.2rem;
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
    main {
      width: min(100% - 1.5rem, 860px);
      padding-top: 2.25rem;
    }

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
