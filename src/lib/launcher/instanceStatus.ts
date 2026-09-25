import type {
  InstanceConfiguration,
  InstanceSummary,
  InstanceValidationDto,
  RuntimeStatusDto,
} from "$lib/backend";

/**
 * Pure presentation derivations over Rust-owned instance state, shared by
 * Home, the Instances list, and the instance workspace so every surface
 * makes the SAME decision for the SAME underlying state. No readiness is
 * reconstructed here beyond what these DTOs already carry: deep validation
 * outcomes come from `validate_instance`, and the configuration-versus-pin
 * comparison mirrors the backend's stale gate for display.
 *
 * Svelte-free and backend-import-free at runtime (erased `import type`
 * only), so the derivations are deterministically testable with the plain
 * Node test runner.
 */

export type InstanceStatusTone =
  | "status-success"
  | "status-warning"
  | "status-error"
  | "status-working"
  | "status-muted";

export interface InstanceStatusDecision {
  tone: InstanceStatusTone;
  label: string;
  detail: string | null;
}

/**
 * True when the saved desired configuration differs from the installed
 * release pin — the display half of the backend's stale gate. Mirrors the
 * comparison `validate_instance` performs; it decides emphasis and the
 * "install new configuration" action, never launchability (that is
 * `get_play_readiness`'s decision alone).
 */
export function configurationRequiresInstall(instance: InstanceSummary): boolean {
  if (instance.state !== "ready") return false;
  return (
    instance.configuration.minecraftVersion !== instance.minecraftVersion ||
    (instance.configuration.loader.policy.type === "pinned" &&
      instance.configuration.loader.policy.version !== instance.fabricLoaderVersion)
  );
}

/**
 * The one content-status decision every surface shows for an instance.
 * Inputs are Rust-owned state only; the installing phase is the live native
 * progress phase when one is running.
 */
export function instanceContentStatus(
  instance: InstanceSummary,
  validation: InstanceValidationDto | undefined,
  installingPhase: string | null,
): InstanceStatusDecision {
  if (instance.state === "installing") {
    return {
      tone: "status-working",
      label: "Installing",
      detail: installingPhase,
    };
  }
  if (validation?.status === "damaged") {
    return {
      tone: "status-error",
      label: "Damaged",
      detail: validation.problems[0]
        ? `${validation.problems[0].component}: ${validation.problems[0].reason}`
        : "Deep validation found problems.",
    };
  }
  if (validation?.status === "stale" || configurationRequiresInstall(instance)) {
    return {
      tone: "status-warning",
      label: "Needs install",
      detail:
        "The saved configuration differs from the installed content — install the new configuration from the instance's Settings tab.",
    };
  }
  if (validation?.status === "ready") {
    return {
      tone: "status-success",
      label: "Ready",
      detail: "Deep validation passed.",
    };
  }
  if (validation?.status === "notInstalled") {
    return {
      tone: "status-muted",
      label: "Not installed",
      detail: "No installed game content was found for this instance.",
    };
  }
  return {
    tone: "status-success",
    label: "Ready",
    detail: "Installed and complete.",
  };
}

/** Quiet one-line description of an instance's desired configuration. */
export function configurationLabel(instance: InstanceSummary): string {
  const loader =
    instance.configuration.loader.policy.type === "pinned"
      ? `Fabric ${instance.configuration.loader.policy.version}`
      : "Fabric (release version)";
  return `Minecraft ${instance.configuration.minecraftVersion} · ${loader}`;
}

/**
 * True when the draft differs from the instance's saved desired
 * configuration. Deterministic structural comparison; the dirty flag can
 * therefore be derived instead of tracked, and it can never disagree with
 * what Save would actually change. Both sides serialize through the same
 * DTO key order, including through Svelte reactive proxies.
 */
export function draftIsDirty(
  draft: InstanceConfiguration,
  configuration: InstanceConfiguration,
): boolean {
  return JSON.stringify(draft) !== JSON.stringify(configuration);
}

/**
 * The managed Java status decision shown on Home and the workspace
 * Overview. The runtime status DTO belongs to one instance; callers pass
 * null when it belongs to a different instance or was never checked.
 */
export function javaRuntimeStatus(
  runtime: RuntimeStatusDto | null,
  busy: boolean,
  error: string | null,
  progressPhase: string | null,
): InstanceStatusDecision {
  if (busy) {
    return {
      tone: "status-working",
      label: "Checking…",
      detail: progressPhase,
    };
  }
  if (error) {
    return { tone: "status-error", label: "Status failed", detail: error };
  }
  if (!runtime) {
    return { tone: "status-muted", label: "Not checked", detail: null };
  }
  if (runtime.status === "ready") {
    return {
      tone: "status-success",
      label: "Ready",
      detail:
        `${runtime.component} · Java ${runtime.requiredMajorVersion}` +
        (runtime.runtimeVersion ? ` · ${runtime.runtimeVersion}` : "") +
        (runtime.reused === true ? " · reused verified runtime" : ""),
    };
  }
  if (runtime.status === "damaged") {
    return {
      tone: "status-error",
      label: "Damaged",
      detail: runtime.problems[0] ?? "The managed runtime failed validation.",
    };
  }
  return {
    tone: "status-warning",
    label: "Not installed",
    detail: `${runtime.component} · Java ${runtime.requiredMajorVersion}`,
  };
}
