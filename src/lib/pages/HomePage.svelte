<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";

  let { onNavigate }: { onNavigate: (page: string) => void } = $props();

  const instance = $derived(launcher.selectedInstance);
  const account = $derived(launcher.selectedAccount);
  const readiness = $derived(
    launcher.playReadiness && launcher.playReadiness.instanceId === instance?.id
      ? launcher.playReadiness
      : null,
  );
  const process = $derived(
    launcher.playProcess && launcher.playProcess.instanceId === instance?.id
      ? launcher.playProcess
      : null,
  );
  const validation = $derived(instance ? launcher.instanceValidations[instance.id] : undefined);
  const runtimeForInstance = $derived(
    launcher.runtimeStatus && launcher.runtimeStatus.instanceId === instance?.id
      ? launcher.runtimeStatus
      : null,
  );

  const playLabel = $derived(
    process?.status === "running"
      ? "Running"
      : launcher.playBusy
        ? "Starting…"
        : "Play",
  );
  const playDisabled = $derived(
    launcher.playBusy || launcher.playReadinessBusy || !readiness?.ready,
  );

  const contentStatus = $derived.by(() => {
    if (!instance) return null;
    if (instance.state === "installing") {
      return {
        tone: "status-working" as const,
        label: "Installing",
        detail: launcher.createProgress ? launcher.createProgress.phase : null,
      };
    }
    if (validation?.status === "damaged") {
      return {
        tone: "status-error" as const,
        label: "Damaged",
        detail: validation.problems[0]
          ? `${validation.problems[0].component}: ${validation.problems[0].reason}`
          : "Deep validation found problems.",
      };
    }
    if (validation?.status === "ready") {
      return {
        tone: "status-success" as const,
        label: "Ready",
        detail: "Deep validation passed.",
      };
    }
    return {
      tone: "status-success" as const,
      label: "Ready",
      detail: "Installed and complete.",
    };
  });

  const javaStatus = $derived.by(() => {
    if (launcher.runtimeBusy) {
      return {
        tone: "status-working" as const,
        label: "Checking…",
        detail: launcher.runtimeProgress
          ? `${launcher.runtimeProgress.phase} ${launcher.runtimeProgress.completedItems}/${launcher.runtimeProgress.totalItems}`
          : null,
      };
    }
    if (launcher.runtimeError) {
      return {
        tone: "status-error" as const,
        label: "Status failed",
        detail: launcher.runtimeError.message,
      };
    }
    if (!runtimeForInstance) {
      return { tone: "status-muted" as const, label: "Not checked", detail: null };
    }
    if (runtimeForInstance.status === "ready") {
      return {
        tone: "status-success" as const,
        label: "Ready",
        detail:
          `${runtimeForInstance.component} · Java ${runtimeForInstance.requiredMajorVersion}` +
          (runtimeForInstance.runtimeVersion ? ` · ${runtimeForInstance.runtimeVersion}` : "") +
          (runtimeForInstance.reused === true ? " · reused verified runtime" : ""),
      };
    }
    if (runtimeForInstance.status === "damaged") {
      return {
        tone: "status-error" as const,
        label: "Damaged",
        detail: runtimeForInstance.problems[0] ?? "The managed runtime failed validation.",
      };
    }
    return {
      tone: "status-warning" as const,
      label: "Not installed",
      detail: `${runtimeForInstance.component} · Java ${runtimeForInstance.requiredMajorVersion}`,
    };
  });

  const accountStatus = $derived.by(() => {
    if (!account) {
      return { tone: "status-muted" as const, label: "Not signed in", detail: null };
    }
    if (account.status === "reauthenticationRequired") {
      return {
        tone: "status-warning" as const,
        label: "Sign-in required",
        detail: account.minecraftName,
      };
    }
    return {
      tone: "status-success" as const,
      label: "Signed in",
      detail: account.minecraftName,
    };
  });

  const processStatus = $derived.by(() => {
    if (launcher.playBusy && !process) {
      return {
        tone: "status-working" as const,
        label: launcher.playProgress ? launcher.playProgress.phase : "Preparing…",
        detail: null,
      };
    }
    if (!process) return { tone: "status-muted" as const, label: "Not running", detail: null };
    if (process.status === "starting") {
      return {
        tone: "status-working" as const,
        label: "Starting…",
        detail: launcher.playProgress ? launcher.playProgress.phase : null,
      };
    }
    if (process.status === "running") {
      return {
        tone: "status-success" as const,
        label: "Running",
        detail:
          process.startedAtUnixSeconds !== null
            ? `Process ${process.processId ?? "?"} · started ${new Date(
                process.startedAtUnixSeconds * 1000,
              ).toLocaleTimeString()}`
            : `Process ${process.processId ?? "?"}`,
      };
    }
    if (process.status === "failed") {
      return {
        tone: "status-error" as const,
        label: "Failed",
        detail: process.message ?? "Minecraft exited unsuccessfully.",
      };
    }
    return {
      tone: "status-muted" as const,
      label: "Exited",
      detail: process.exitCode !== null ? `Exit code ${process.exitCode}` : null,
    };
  });

  const blockers = $derived(readiness && !readiness.ready ? readiness.blockers : []);
</script>

<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">Home</h2>
      <p class="page-subtitle">Your selected instance and everything Play needs.</p>
    </div>
  </header>

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
  {:else if !launcher.launcherState}
    <section class="group" aria-live="polite">
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <p class="group-row-detail">Loading persisted launcher state…</p>
      </div>
    </section>
  {:else if !instance}
    {#if launcher.launcherState.instances.length === 0}
      <section class="empty-state" aria-live="polite">
        <h3 class="empty-title">No instances yet</h3>
        <p class="empty-detail">
          Create an isolated Minecraft installation to get started.
        </p>
        <button type="button" class="btn btn-primary" onclick={() => onNavigate("instances")}>
          Create instance
        </button>
      </section>
    {:else}
      <section class="empty-state" aria-live="polite">
        <h3 class="empty-title">No instance selected</h3>
        <p class="empty-detail">Choose an instance to launch from.</p>
        <button type="button" class="btn btn-primary" onclick={() => onNavigate("instances")}>
          Go to Instances
        </button>
      </section>
    {/if}
  {:else}
    <section class="group" aria-live="polite">
      <div class="group-heading">
        <div class="instance-heading">
          <h3 class="instance-name">{instance.displayName}</h3>
          <p class="instance-versions">
            Aurora {instance.auroraVersion} ({instance.channel}) · Minecraft
            {instance.minecraftVersion} · Fabric {instance.fabricLoaderVersion}
          </p>
        </div>
        <div class="play-actions">
          <button
            type="button"
            class="btn btn-primary"
            onclick={() => launcher.runPlay(instance.id)}
            disabled={playDisabled}
          >
            {playLabel}
          </button>
          <button
            type="button"
            class="btn btn-quiet"
            onclick={() => launcher.refreshPlayReadiness()}
            disabled={launcher.playBusy || launcher.playReadinessBusy}
          >
            {launcher.playReadinessBusy ? "Checking…" : "Check again"}
          </button>
        </div>
      </div>

      {#if contentStatus}
        <div class="group-row">
          <div class="group-row-main">
            <span class="group-row-title">Game content</span>
            {#if contentStatus.detail}
              <span class="group-row-detail">{contentStatus.detail}</span>
            {/if}
          </div>
          <span class="status-badge {contentStatus.tone}">{contentStatus.label}</span>
        </div>
      {/if}

      <div class="group-row">
        <div class="group-row-main">
          <span class="group-row-title">Java runtime</span>
          {#if javaStatus.detail}
            <span class="group-row-detail" class:is-error={javaStatus.tone === "status-error"}
              >{javaStatus.detail}</span
            >
          {/if}
        </div>
        <div class="group-row-actions">
          <span class="status-badge {javaStatus.tone}">{javaStatus.label}</span>
          {#if instance.state === "ready" && !launcher.runtimeBusy && runtimeForInstance && runtimeForInstance.status !== "ready"}
            <button
              type="button"
              class="btn"
              onclick={() => launcher.runEnsureRuntime(instance.id)}
            >
              {runtimeForInstance.status === "damaged" ? "Repair Java" : "Install Java"}
            </button>
          {/if}
        </div>
      </div>

      <div class="group-row">
        <div class="group-row-main">
          <span class="group-row-title">Account</span>
          {#if accountStatus.detail}
            <span class="group-row-detail">{accountStatus.detail}</span>
          {/if}
        </div>
        <div class="group-row-actions">
          <span class="status-badge {accountStatus.tone}">{accountStatus.label}</span>
          {#if !account || account.status === "reauthenticationRequired"}
            <button type="button" class="btn btn-quiet" onclick={() => onNavigate("accounts")}>
              {account ? "Fix sign-in" : "Sign in"}
            </button>
          {/if}
        </div>
      </div>

      <div class="group-row">
        <div class="group-row-main">
          <span class="group-row-title">Minecraft</span>
          {#if processStatus.detail}
            <span
              class="group-row-detail"
              class:is-error={processStatus.tone === "status-error"}
              >{processStatus.detail}</span
            >
          {/if}
        </div>
        <span class="status-badge {processStatus.tone}">{processStatus.label}</span>
      </div>

      {#if blockers.length > 0}
        {#each blockers as blocker (blocker.code)}
          <div class="group-row blocker-row">
            <div class="group-row-main">
              <span class="group-row-title blocker-text">{blocker.message}</span>
              <span class="group-row-detail">{blocker.code}</span>
            </div>
          </div>
        {/each}
      {/if}

      {#if launcher.playError}
        <p class="inline-message inline-message-error group-row" role="alert">
          {launcher.playError.message}
          <code>{launcher.playError.code}</code>
        </p>
      {/if}

      <p class="group-footer">
        Readiness is decided by Aurora from validated content, the exact managed Java runtime,
        and a usable authenticated session — the button reflects that decision.
      </p>
    </section>
  {/if}
</div>

<style>
  .instance-heading {
    min-width: 0;
  }

  .instance-name {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
    letter-spacing: -0.01em;
    overflow-wrap: anywhere;
  }

  .instance-versions {
    margin: var(--space-1) 0 0;
    color: var(--color-text-secondary);
    font-size: var(--text-metadata);
  }

  .play-actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .group-row-loading {
    justify-content: flex-start;
    gap: var(--space-3);
  }

  .group-row-detail.is-error {
    color: var(--color-error);
  }

  .blocker-row {
    background: var(--color-warning-soft);
  }

  .blocker-text {
    color: var(--color-warning);
  }
</style>
