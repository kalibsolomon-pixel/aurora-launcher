import { invoke } from "@tauri-apps/api/core";

export type BackendStatus = "ready";

export interface PlatformInfo {
  os: string;
  architecture: string;
}

export interface ApplicationStatus {
  launcherVersion: string;
  platform: PlatformInfo;
  managedDataRoot: string;
  backendStatus: BackendStatus;
}

interface BackendCommandError {
  code: string;
  message: string;
}

export class LauncherBackendError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = "LauncherBackendError";
    this.code = code;
  }
}

function isBackendCommandError(value: unknown): value is BackendCommandError {
  if (typeof value !== "object" || value === null) return false;

  const candidate = value as Record<string, unknown>;
  return typeof candidate.code === "string" && typeof candidate.message === "string";
}

export async function getApplicationStatus(): Promise<ApplicationStatus> {
  try {
    return await invoke<ApplicationStatus>("get_application_status");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "Aurora's native backend did not return a usable status.",
    );
  }
}

export type ReleaseChannel = "stable" | "beta" | "nightly";

export interface LauncherConfigSummary {
  schemaVersion: number;
  selectedInstanceId: string | null;
}

export interface InstanceSummary {
  id: string;
  displayName: string;
  channel: ReleaseChannel;
  auroraVersion: string | null;
}

export interface LauncherState {
  config: LauncherConfigSummary;
  instances: InstanceSummary[];
}

export async function getLauncherState(): Promise<LauncherState> {
  try {
    return await invoke<LauncherState>("get_launcher_state");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "Aurora's persisted launcher state could not be loaded.",
    );
  }
}
