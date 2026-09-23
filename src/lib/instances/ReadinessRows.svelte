<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
  import { navigation } from "$lib/launcher/navigation.svelte";
  import {
    instanceContentStatus,
    javaRuntimeStatus,
  } from "$lib/launcher/instanceStatus";
  import type { InstanceSummary } from "$lib/backend";

  let { instance }: { instance: InstanceSummary } = $props();

  const account = $derived(launcher.selectedAccount);
  const validation = $derived(launcher.instanceValidations[instance.id]);
  const isSelected = $derived(
    launcher.launcherState?.config.selectedInstanceId === instance.id,
  );
  const runtimeForInstance = $derived(
    launcher.runtimeStatus && launcher.runtimeStatus.instanceId === instance.id
      ? launcher.runtimeStatus
      : null,
  );
  const process = $derived(
    launcher.playProcess && launcher.playProcess.instanceId === instance.id
      ? launcher.playProcess
      : null,
  );
  const readiness = $derived(
    launcher.playReadiness && launcher.playReadiness.instanceId === instance.id
      ? launcher.playReadiness
      : null,
  );

  const installingPhase = $derived(
    instance.state === "installing" && launcher.createProgress
      ? launcher.createProgress.phase
      : null,
  );

  const contentStatus = $derived(
    instanceContentStatus(instance, validation, installingPhase),
  );

  const javaStatus = $derived(
    javaRuntimeStatus(
      runtimeForInstance,
      launcher.runtimeBusy,
      launcher.runtimeError?.message ?? null,
      launcher.runtimeProgress
        ? `${launcher.runtimeProgress.phase} ${launcher.runtimeProgress.completedItems}/${launcher.runtimeProgress.totalItems}`
        : null,
    ),
  );

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
    if (launcher.playBusy && !process && isSelected) {
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

<!--
  The readiness rows shared by Home and the instance workspace Overview.
  One component over one derivation module, so both surfaces always make
  the same decision for the same underlying Rust-owned state.
-->
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
    {#if instance.state === "ready" && !launcher.runtimeBusy}
      {#if !runtimeForInstance}
        <button
          type="button"
          class="btn"
          onclick={() => launcher.runRuntimeStatus(instance.id)}
          disabled={launcher.createBusy}
        >
          Check Java
        </button>
      {:else if runtimeForInstance.status !== "ready"}
        <button
          type="button"
          class="btn"
          onclick={() => launcher.runEnsureRuntime(instance.id)}
          disabled={launcher.createBusy}
        >
          {runtimeForInstance.status === "damaged" ? "Repair Java" : "Install Java"}
        </button>
      {/if}
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
      <button type="button" class="btn btn-quiet" onclick={() => navigation.goTo("accounts")}>
        {account ? "Fix sign-in" : "Sign in"}
      </button>
    {/if}
  </div>
</div>

<div class="group-row">
  <div class="group-row-main">
    <span class="group-row-title">Minecraft</span>
    {#if processStatus.detail}
      <span class="group-row-detail" class:is-error={processStatus.tone === "status-error"}
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

{#if launcher.playError && isSelected}
  <p class="inline-message inline-message-error group-row" role="alert">
    {launcher.playError.message}
    <code>{launcher.playError.code}</code>
  </p>
{/if}

<style>
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
