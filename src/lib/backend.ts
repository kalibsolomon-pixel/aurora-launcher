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

/** One selectable built-in launcher theme. */
export interface ThemeOption {
  id: string;
  label: string;
  description: string;
}

/** One curated accent preset (the hex is the swatch preview color). */
export interface AccentOption {
  id: string;
  label: string;
  hex: string;
}

/** The accent selection as persisted: a curated preset or a custom color. */
export type AccentSelection =
  | { type: "preset"; id: string }
  | { type: "custom"; hex: string };

/** The accent-family CSS token values Rust derived for the current accent. */
export interface AccentPalette {
  accent: string;
  accentStrong: string;
  accentHover: string;
  accentPressed: string;
  accentContrast: string;
  accentSoft: string;
  accentOutline: string;
}

/** Launcher-wide appearance state plus the catalogs Settings renders. */
export interface AppearanceState {
  theme: string;
  accent: AccentSelection;
  palette: AccentPalette;
  themes: ThemeOption[];
  accents: AccentOption[];
}

export async function getAppearance(): Promise<AppearanceState> {
  try {
    return await invoke<AppearanceState>("get_appearance");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The launcher appearance could not be loaded.",
    );
  }
}

export async function setAppearance(
  theme: string,
  accent: AccentSelection,
): Promise<AppearanceState> {
  try {
    return await invoke<AppearanceState>("set_appearance", {
      request: { theme, accent },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The launcher appearance could not be saved.",
    );
  }
}

/** Live Windows shortcut status for one slot, as Settings renders it. */
export interface ShortcutStatus {
  state: "present" | "absent" | "conflict" | "unknown";
  managedBy: "aurora" | "installer";
}

/**
 * Live Windows desktop-integration state. Every field is queried from the
 * operating system on demand — shortcut status is never persisted, so it
 * always reflects reality when Settings is opened or refreshed.
 */
export interface DesktopIntegrationState {
  supported: boolean;
  manageable: boolean;
  desktopShortcut: ShortcutStatus;
  startMenuShortcut: ShortcutStatus;
}

export async function getDesktopIntegration(): Promise<DesktopIntegrationState> {
  try {
    return await invoke<DesktopIntegrationState>("get_desktop_integration");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "Windows shortcut status could not be read.",
    );
  }
}

export async function createDesktopShortcut(): Promise<DesktopIntegrationState> {
  try {
    return await invoke<DesktopIntegrationState>("create_desktop_shortcut");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The desktop shortcut could not be created.",
    );
  }
}

export async function removeDesktopShortcut(): Promise<DesktopIntegrationState> {
  try {
    return await invoke<DesktopIntegrationState>("remove_desktop_shortcut");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The desktop shortcut could not be removed.",
    );
  }
}

export type AcquisitionOrigin = "downloaded" | "cacheHit";

export interface AcquireArtifactRequest {
  url: string;
  sha256: string;
  sizeBytes: number | null;
}

export interface AcquiredArtifact {
  path: string;
  sha256: string;
  bytes: number;
  origin: AcquisitionOrigin;
}

export async function acquireArtifact(
  request: AcquireArtifactRequest,
): Promise<AcquiredArtifact> {
  try {
    return await invoke<AcquiredArtifact>("acquire_artifact", { request });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The artifact acquisition could not be completed.",
    );
  }
}

export interface PlanMinecraftInstallRequest {
  version: string;
}

/** Concise summary of a resolved Minecraft installation plan. */
export interface MinecraftPlanSummary {
  minecraftVersion: string;
  versionType: string;
  javaComponent: string;
  javaMajorVersion: number;
  clientSha1: string;
  clientSizeBytes: number;
  assetIndexId: string;
  libraryCount: number;
  nativeLibraryCount: number;
  mainClass: string;
  gameArgumentCount: number;
  jvmArgumentCount: number;
}

export async function planMinecraftInstall(
  request: PlanMinecraftInstallRequest,
): Promise<MinecraftPlanSummary> {
  try {
    return await invoke<MinecraftPlanSummary>("plan_minecraft_install", { request });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The Minecraft installation plan could not be resolved.",
    );
  }
}

export interface PlanFabricInstallRequest {
  minecraftVersion: string;
  loaderVersion: string;
}

/** Concise summary of a composed Minecraft + Fabric Loader game plan. */
export interface FabricPlanSummary {
  minecraftVersion: string;
  loaderVersion: string;
  vanillaLibraryCount: number;
  fabricLibraryCount: number;
  finalLibraryCount: number;
  fabricDigestedLibraryCount: number;
  javaComponent: string;
  javaMajorVersion: number;
  javaRaisedByLoader: boolean;
  finalMainClass: string;
}

export async function planFabricInstall(
  request: PlanFabricInstallRequest,
): Promise<FabricPlanSummary> {
  try {
    return await invoke<FabricPlanSummary>("plan_fabric_install", { request });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The Fabric installation plan could not be resolved.",
    );
  }
}

export interface InstallGameRequest {
  instanceId: string;
  minecraftVersion: string;
  loaderVersion: string;
}

/** One native installation-progress event; the frontend only displays it. */
export interface InstallProgressEvent {
  phase:
    | "acquiring"
    | "materializing"
    | "extractingNatives"
    | "validating"
    | "committing";
  completedItems: number;
  totalItems: number;
  currentItem: string | null;
}

/** Concise summary of one committed installation. */
export interface InstalledGameSummary {
  minecraftVersion: string;
  loaderVersion: string;
  installationId: string;
  fileCount: number;
  totalBytes: number;
  verifiedSha1Files: number;
  verifiedSha256Files: number;
  transportObservedFiles: number;
  nativesDirectory: string;
  gameDirectory: string;
}

export async function installGame(
  request: InstallGameRequest,
): Promise<InstalledGameSummary> {
  try {
    return await invoke<InstalledGameSummary>("install_game", { request });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The game installation could not be completed.",
    );
  }
}

export interface ValidateInstalledGameRequest {
  instanceId: string;
}

export type InstalledGameStatus = "valid" | "damaged" | "notInstalled";

export interface ValidationProblem {
  path: string;
  reason: string;
}

/** Read-only validation outcome of one installed game. */
export interface InstalledGameValidation {
  status: InstalledGameStatus;
  minecraftVersion: string | null;
  loaderVersion: string | null;
  installationId: string | null;
  checkedFiles: number;
  verifiedBytes: number;
  problems: ValidationProblem[];
}

export async function validateInstalledGame(
  request: ValidateInstalledGameRequest,
): Promise<InstalledGameValidation> {
  try {
    return await invoke<InstalledGameValidation>("validate_installed_game", { request });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The installed game could not be validated.",
    );
  }
}

export type AuroraChannel = "stable" | "beta" | "nightly";

/** One Aurora release offered for instance creation. */
export interface AuroraReleaseSummary {
  source: string;
  auroraVersion: string;
  channel: AuroraChannel;
  minecraftVersion: string;
  fabricLoaderVersion: string;
  javaMajorVersion: number;
}

export async function listAuroraReleases(): Promise<AuroraReleaseSummary[]> {
  try {
    return await invoke<AuroraReleaseSummary[]>("list_aurora_releases");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The Aurora release list could not be loaded.",
    );
  }
}

export interface InstanceLoaderPolicy {
  type: "automatic" | "pinned";
  version?: string;
}

/** The desired loader kind and version policy of one instance. */
export interface InstanceLoader {
  kind: "fabric";
  policy: InstanceLoaderPolicy;
}

/** A custom windowed resolution, passed to Minecraft's own launch arguments. */
export interface InstanceWindow {
  width: number;
  height: number;
}

/** The desired configuration of one instance: what the user wants it to be. */
export interface InstanceConfiguration {
  minecraftVersion: string;
  loader: InstanceLoader;
  memoryMib: number;
  additionalJvmArguments: string;
  window: InstanceWindow | null;
}

export interface InstanceSummary {
  id: string;
  displayName: string;
  state: "installing" | "ready";
  channel: AuroraChannel;
  auroraVersion: string;
  minecraftVersion: string;
  fabricLoaderVersion: string;
  configuration: InstanceConfiguration;
}

export interface CreateInstanceRequest {
  displayName: string;
  minecraftVersion: string;
  loaderPolicy: InstanceLoaderPolicy;
}

/** One lifecycle progress event; game item progress is embedded verbatim. */
export interface InstanceProgressEvent {
  phase:
    | "resolvingRelease"
    | "resolvingGame"
    | "installingGame"
    | "installingAurora"
    | "validating"
    | "completing";
  game: InstallProgressEvent | null;
}

export async function createInstance(
  request: CreateInstanceRequest,
): Promise<InstanceSummary> {
  try {
    return await invoke<InstanceSummary>("create_instance", { request });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The instance could not be created.",
    );
  }
}

export interface UpdateInstanceConfigurationRequest {
  instanceId: string;
  configuration: InstanceConfiguration;
}

export async function updateInstanceConfiguration(
  instanceId: string,
  configuration: InstanceConfiguration,
): Promise<InstanceSummary> {
  try {
    return await invoke<InstanceSummary>("update_instance_configuration", {
      request: { instanceId, configuration },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The configuration could not be saved.",
    );
  }
}

export async function installInstanceConfiguration(
  instanceId: string,
): Promise<InstanceSummary> {
  try {
    return await invoke<InstanceSummary>("install_instance_configuration", {
      request: { instanceId },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The new configuration could not be installed.",
    );
  }
}

/** One Minecraft version from the official Mojang manifest. */
export interface MinecraftVersion {
  id: string;
  versionType: "release" | "snapshot";
}

export async function listMinecraftVersions(
  includeSnapshots: boolean,
): Promise<MinecraftVersion[]> {
  try {
    return await invoke<MinecraftVersion[]>("list_minecraft_versions", {
      includeSnapshots,
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The Minecraft version list could not be loaded.",
    );
  }
}

/** One Fabric Loader version available for a Minecraft version. */
export interface FabricLoaderVersion {
  version: string;
  stable: boolean;
}

export async function listFabricLoaderVersions(
  minecraftVersion: string,
): Promise<FabricLoaderVersion[]> {
  try {
    return await invoke<FabricLoaderVersion[]>("list_fabric_loader_versions", {
      minecraftVersion,
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The Fabric Loader versions could not be loaded.",
    );
  }
}

export async function retryInstanceInstall(instanceId: string): Promise<InstanceSummary> {
  try {
    return await invoke<InstanceSummary>("retry_instance_install", {
      request: { instanceId },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The installation could not be retried.",
    );
  }
}

export async function renameInstance(
  instanceId: string,
  newDisplayName: string,
): Promise<InstanceSummary> {
  try {
    return await invoke<InstanceSummary>("rename_instance", {
      request: { instanceId, newDisplayName },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The instance could not be renamed.",
    );
  }
}

export async function selectInstance(instanceId: string): Promise<void> {
  try {
    await invoke<void>("select_instance", { request: { instanceId } });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The instance could not be selected.",
    );
  }
}

/**
 * Opens one instance's managed root folder in the operating system's file
 * browser. Rust derives the folder from the validated instance id and the
 * managed paths — no frontend-supplied path exists on this surface.
 */
export async function openInstanceFolder(instanceId: string): Promise<void> {
  try {
    await invoke<void>("open_instance_folder", { request: { instanceId } });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The instance folder could not be opened.",
    );
  }
}

export type ModFileType =
  | "enabledJar"
  | "disabledJar"
  | "unexpectedFile"
  | "directory"
  | "link";

export type ModOwnership =
  | "launcherManagedRequired"
  | "providerManaged"
  | "userManaged"
  | "unknown";

export interface ModRelation {
  modId: string;
  requirement: string;
}

export interface FabricModMetadata {
  id: string;
  name: string | null;
  version: string | null;
  description: string | null;
  authors: string[];
  environment: string | null;
  depends: ModRelation[];
  recommends: ModRelation[];
  suggests: ModRelation[];
  conflicts: ModRelation[];
  breaks: ModRelation[];
  hasDeclaredIcon: boolean;
}

export interface ModWarning {
  code: string;
  message: string;
}

/** One direct child of the authoritative instance mods directory. */
export interface ModEntry {
  /** Opaque scan identity; never a filesystem path. */
  entryId: string;
  fileName: string;
  displayName: string;
  enabled: boolean;
  fileType: ModFileType;
  sizeBytes: number | null;
  modifiedUnixMillis: number | null;
  ownership: ModOwnership;
  sha256: string | null;
  provenance: ProviderRecord | null;
  metadata: FabricModMetadata | null;
  warnings: ModWarning[];
  canToggle: boolean;
  canRemove: boolean;
  actionBlockedReason: string | null;
}

export interface ModInventory {
  instanceId: string;
  entries: ModEntry[];
  missingManaged: ProviderRecord[];
}

export type ContentType = "mod" | "resourcePack" | "shaderPack";
export type ContentOwnership = ModOwnership;
export type DependencyKind = "required" | "optional" | "incompatible";

export interface ProviderDependency {
  kind: DependencyKind;
  provider: string;
  projectId: string;
  versionId: string | null;
}

export interface ContentCompatibility {
  minecraftVersions: string[];
  loader: string | null;
  environment: string | null;
}

export interface ProviderRecord {
  contentType: ContentType;
  provider: string;
  projectId: string;
  versionId: string;
  fileId: string;
  fileName: string;
  sha256: string;
  displayVersion: string | null;
  compatibility: ContentCompatibility;
  dependencies: ProviderDependency[];
}

export interface ContentEntry {
  entryId: string;
  contentType: ContentType;
  fileName: string;
  displayName: string;
  fileType: "zip" | "directory" | "link" | "unexpectedFile" | "unreadable";
  sizeBytes: number | null;
  modifiedUnixMillis: number | null;
  ownership: ContentOwnership;
  sha256: string | null;
  provenance: ProviderRecord | null;
  description: string | null;
  packFormat: number | null;
  warnings: ModWarning[];
  canRemove: boolean;
}

export interface ContentInventory {
  instanceId: string;
  contentType: ContentType;
  entries: ContentEntry[];
  missingManaged: ProviderRecord[];
}

export interface InstanceContentContext {
  instanceId: string;
  minecraftVersion: string;
  loader: "fabric";
  loaderVersion: string;
  auroraVersion: string;
  environment: "client";
}

async function contentInvoke<T>(command: string, request: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, { request });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) throw new LauncherBackendError(error.code, error.message);
    throw new LauncherBackendError("backend_unavailable", "Instance content is unavailable.");
  }
}

export function getInstanceContentContext(instanceId: string): Promise<InstanceContentContext> {
  return contentInvoke("get_instance_content_context", { instanceId });
}

export function getInstanceContent(instanceId: string, contentType: ContentType): Promise<ContentInventory> {
  return contentInvoke("get_instance_content", { instanceId, contentType });
}

export function removeInstanceContent(instanceId: string, contentType: ContentType, entryId: string): Promise<ContentInventory> {
  return contentInvoke("remove_instance_content", { instanceId, contentType, entryId });
}

export function openInstanceContentFolder(instanceId: string, contentType: ContentType): Promise<void> {
  return contentInvoke("open_instance_content_folder", { instanceId, contentType });
}

export async function getInstanceMods(instanceId: string): Promise<ModInventory> {
  try {
    return await invoke<ModInventory>("get_instance_mods", { request: { instanceId } });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The local mod inventory could not be loaded.",
    );
  }
}

export async function setInstanceModEnabled(
  instanceId: string,
  entryId: string,
  enabled: boolean,
): Promise<ModInventory> {
  try {
    return await invoke<ModInventory>("set_instance_mod_enabled", {
      request: { instanceId, entryId, enabled },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The local mod state could not be changed.",
    );
  }
}

export async function removeInstanceMod(
  instanceId: string,
  entryId: string,
): Promise<ModInventory> {
  try {
    return await invoke<ModInventory>("remove_instance_mod", {
      request: { instanceId, entryId },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The local mod could not be removed.",
    );
  }
}

export async function openInstanceModsFolder(instanceId: string): Promise<void> {
  try {
    await invoke<void>("open_instance_mods_folder", { request: { instanceId } });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The instance mods folder could not be opened.",
    );
  }
}

export type InstanceValidationStatus =
  | "ready"
  | "damaged"
  | "installing"
  | "stale"
  | "notInstalled";

export interface InstanceProblem {
  component: string;
  reason: string;
}

/** Complete read-only validation outcome of one instance. */
export interface InstanceValidationDto {
  instanceId: string;
  displayName: string;
  status: InstanceValidationStatus;
  problems: InstanceProblem[];
}

export async function validateInstance(instanceId: string): Promise<InstanceValidationDto> {
  try {
    return await invoke<InstanceValidationDto>("validate_instance", {
      request: { instanceId },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }

    throw new LauncherBackendError(
      "backend_unavailable",
      "The instance could not be validated.",
    );
  }
}

export type RuntimeStatus = "missing" | "ready" | "damaged";

export interface RuntimeStatusDto {
  instanceId: string;
  contentStatus: "ready";
  status: RuntimeStatus;
  component: string;
  requiredMajorVersion: number;
  runtimeVersion: string | null;
  runtimeRoot: string;
  launchExecutable: string | null;
  checkedFiles: number;
  verifiedBytes: number;
  reportedMajorVersion: number | null;
  diagnosticSummary: string | null;
  problems: string[];
  reused: boolean | null;
}

export interface RuntimeProgressEvent {
  phase: "acquiring" | "materializing" | "validating" | "executing" | "committing";
  completedItems: number;
  totalItems: number;
  currentItem: string | null;
}

export async function getInstanceRuntimeStatus(instanceId: string): Promise<RuntimeStatusDto> {
  try {
    return await invoke<RuntimeStatusDto>("get_instance_runtime_status", {
      request: { instanceId },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The managed Java status could not be loaded.",
    );
  }
}

export async function ensureInstanceRuntime(instanceId: string): Promise<RuntimeStatusDto> {
  try {
    return await invoke<RuntimeStatusDto>("ensure_instance_runtime", {
      request: { instanceId },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The managed Java runtime could not be installed.",
    );
  }
}

export type AccountStatus = "signedIn" | "reauthenticationRequired";

/** One non-secret account summary; tokens never cross this boundary. */
export interface AccountSummary {
  accountId: string;
  minecraftName: string;
  status: AccountStatus;
}

export interface AccountsState {
  accounts: AccountSummary[];
  selectedAccountId: string | null;
}

/** Coarse sign-in progress phase; never carries credentials. */
export interface AuthProgressEvent {
  phase:
    | "waitingForMicrosoft"
    | "exchangingMicrosoftToken"
    | "authenticatingWithXbox"
    | "authorizingXsts"
    | "authenticatingMinecraft"
    | "checkingEntitlement"
    | "fetchingProfile"
    | "savingAccount"
    | "restoringSession";
}

export async function getAccounts(): Promise<AccountsState> {
  try {
    return await invoke<AccountsState>("get_accounts");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The account list could not be loaded.",
    );
  }
}

export async function beginMicrosoftLogin(): Promise<AccountSummary> {
  try {
    return await invoke<AccountSummary>("begin_microsoft_login");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "Microsoft sign-in could not be started.",
    );
  }
}

export async function cancelMicrosoftLogin(): Promise<void> {
  try {
    await invoke<void>("cancel_microsoft_login");
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The sign-in could not be cancelled.",
    );
  }
}

export async function selectAccount(accountId: string): Promise<void> {
  try {
    await invoke<void>("select_account", { request: { accountId } });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The account could not be selected.",
    );
  }
}

export async function removeAccount(accountId: string): Promise<void> {
  try {
    await invoke<void>("remove_account", { request: { accountId } });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The account could not be removed.",
    );
  }
}

/** Outcome of an on-demand session restoration; the token stays in Rust. */
export interface AccountSession {
  accountId: string;
  minecraftName: string;
  status: "ready";
}

export async function refreshAccountSession(accountId: string): Promise<AccountSession> {
  try {
    return await invoke<AccountSession>("refresh_account_session", {
      request: { accountId },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "The account session could not be restored.",
    );
  }
}

export type LaunchInstanceStatus =
  | "ready"
  | "missing"
  | "installing"
  | "damaged"
  | "stale";
export type LaunchRuntimeStatus = "ready" | "missing" | "damaged" | "unresolved";
export type LaunchAccountStatus =
  | "ready"
  | "missing"
  | "reauthenticationRequired"
  | "configurationMissing";
export type LaunchProcessStatus = "stopped" | "starting" | "running" | "exited" | "failed";

export interface PlayBlocker {
  code: string;
  message: string;
}

/** Rust's complete non-secret decision about whether Play may proceed. */
export interface PlayReadiness {
  ready: boolean;
  instanceId: string;
  accountId: string | null;
  accountName: string | null;
  instanceStatus: LaunchInstanceStatus;
  runtimeStatus: LaunchRuntimeStatus;
  accountStatus: LaunchAccountStatus;
  processStatus: LaunchProcessStatus;
  blockers: PlayBlocker[];
}

/** Non-secret state of the exact child supervised by this launcher process. */
export interface LaunchProcess {
  instanceId: string;
  status: LaunchProcessStatus;
  processId: number | null;
  startedAtUnixSeconds: number | null;
  exitCode: number | null;
  message: string | null;
}

export interface LaunchProgressEvent {
  phase:
    | "checkingPreconditions"
    | "resolvingLaunch"
    | "restoringSession"
    | "assemblingArguments"
    | "startingProcess";
}

export async function getPlayReadiness(
  instanceId: string,
  accountId: string | null,
): Promise<PlayReadiness> {
  try {
    return await invoke<PlayReadiness>("get_play_readiness", {
      request: { instanceId, accountId },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError(
      "backend_unavailable",
      "Play readiness could not be determined.",
    );
  }
}

export async function playInstance(
  instanceId: string,
  accountId: string,
): Promise<LaunchProcess> {
  try {
    return await invoke<LaunchProcess>("play_instance", {
      request: { instanceId, accountId },
    });
  } catch (error: unknown) {
    if (isBackendCommandError(error)) {
      throw new LauncherBackendError(error.code, error.message);
    }
    throw new LauncherBackendError("backend_unavailable", "Minecraft could not be started.");
  }
}
