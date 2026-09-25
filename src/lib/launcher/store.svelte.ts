import { listen } from "@tauri-apps/api/event";
import {
  acquireArtifact,
  beginMicrosoftLogin,
  cancelMicrosoftLogin,
  createInstance,
  ensureInstanceRuntime,
  getApplicationStatus,
  getAccounts,
  getInstanceRuntimeStatus,
  getLauncherState,
  getInstanceMods,
  getPlayReadiness,
  installGame,
  installInstanceConfiguration,
  listAuroraReleases,
  listFabricLoaderVersions,
  listMinecraftVersions,
  openInstanceFolder,
  openInstanceModsFolder,
  planFabricInstall,
  planMinecraftInstall,
  playInstance,
  refreshAccountSession,
  removeAccount,
  removeInstanceMod,
  renameInstance,
  retryInstanceInstall,
  selectAccount,
  selectInstance,
  setInstanceModEnabled,
  updateInstanceConfiguration,
  validateInstance,
  validateInstalledGame,
  LauncherBackendError,
  type AcquiredArtifact,
  type AccountSession,
  type AccountSummary,
  type AccountsState,
  type ApplicationStatus,
  type AuroraReleaseSummary,
  type AuthProgressEvent,
  type FabricLoaderVersion,
  type FabricPlanSummary,
  type InstanceConfiguration,
  type InstalledGameSummary,
  type InstalledGameValidation,
  type InstanceProgressEvent,
  type InstanceSummary,
  type InstanceValidationDto,
  type InstallProgressEvent,
  type LauncherState,
  type LaunchProcess,
  type LaunchProgressEvent,
  type MinecraftPlanSummary,
  type MinecraftVersion,
  type ModInventory,
  type PlayReadiness,
  type RuntimeProgressEvent,
  type RuntimeStatusDto,
} from "$lib/backend";

function backendError(cause: unknown, fallback: string): LauncherBackendError {
  return cause instanceof LauncherBackendError
    ? cause
    : new LauncherBackendError("unknown_error", fallback);
}

/**
 * The one frontend owner of launcher presentation state. Every value here is
 * real Rust-owned state or a native progress event; nothing is mocked.
 */
class LauncherStore {
  // Application and persisted state (About / shell).
  status = $state<ApplicationStatus | null>(null);
  statusError = $state<LauncherBackendError | null>(null);
  launcherState = $state<LauncherState | null>(null);
  stateError = $state<LauncherBackendError | null>(null);

  // Aurora releases (instance creation source; development fixture today).
  releases = $state<AuroraReleaseSummary[]>([]);
  releasesError = $state<LauncherBackendError | null>(null);

  // Instance management. All state comes from Rust.
  createDisplayName = $state("");
  createMinecraftVersion = $state("");
  createLoaderPolicy = $state<{ type: "automatic" | "pinned"; version?: string }>({
    type: "automatic",
  });
  createIncludeSnapshots = $state(false);
  createBusy = $state(false);
  createProgress = $state<InstanceProgressEvent | null>(null);
  createError = $state<LauncherBackendError | null>(null);
  minecraftVersions = $state<MinecraftVersion[] | null>(null);
  minecraftVersionsBusy = $state(false);
  minecraftVersionsError = $state<LauncherBackendError | null>(null);
  loaderVersions = $state<FabricLoaderVersion[] | null>(null);
  loaderVersionsBusy = $state(false);
  loaderVersionsError = $state<LauncherBackendError | null>(null);
  renaming = $state<{ id: string; name: string } | null>(null);
  instanceBusy = $state<string | null>(null);
  instanceValidations = $state<Record<string, InstanceValidationDto>>({});
  instanceError = $state<LauncherBackendError | null>(null);

  // Per-instance configuration editing (the workspace Settings tab). Each
  // draft is a copy of that instance's desired configuration; Save sends the
  // whole proposed configuration atomically. Drafts live per instance id, so
  // opening another instance or leaving the workspace never discards unsaved
  // edits silently.
  detailDrafts = $state<Record<string, InstanceConfiguration>>({});
  detailBusy = $state<string | null>(null);
  detailError = $state<LauncherBackendError | null>(null);
  detailInstallBusy = $state<string | null>(null);

  // Instance folder opening (the workspace header's contextual action).
  folderBusy = $state<string | null>(null);
  folderError = $state<LauncherBackendError | null>(null);

  // Filesystem-authoritative local Mods snapshots. Search/filter/sort are
  // presentation-only over these snapshots; only explicit refresh/mutation
  // calls cross the native boundary.
  modInventories = $state<Record<string, ModInventory>>({});
  modInventoryBusy = $state<string | null>(null);
  modMutationBusy = $state<string | null>(null);
  modFolderBusy = $state<string | null>(null);
  modError = $state<LauncherBackendError | null>(null);

  // Managed Java runtime for the selected instance.
  runtimeStatus = $state<RuntimeStatusDto | null>(null);
  runtimeBusy = $state(false);
  runtimeProgress = $state<RuntimeProgressEvent | null>(null);
  runtimeError = $state<LauncherBackendError | null>(null);

  // Account surface. All state comes from Rust; the UI never sees tokens.
  accountsState = $state<AccountsState | null>(null);
  accountsError = $state<LauncherBackendError | null>(null);
  signInBusy = $state(false);
  signInProgress = $state<AuthProgressEvent | null>(null);
  signInError = $state<LauncherBackendError | null>(null);
  accountBusy = $state<string | null>(null);
  accountError = $state<LauncherBackendError | null>(null);
  accountSessions = $state<Record<string, AccountSession>>({});

  // Play surface. Rust owns the readiness decision and process state; the
  // frontend only renders the non-secret DTOs.
  playReadiness = $state<PlayReadiness | null>(null);
  playProcess = $state<LaunchProcess | null>(null);
  playProgress = $state<LaunchProgressEvent | null>(null);
  playBusy = $state(false);
  playReadinessBusy = $state(false);
  playError = $state<LauncherBackendError | null>(null);

  // Development proofs of the native pipelines; stripped from production.
  artifactUrl = $state("");
  artifactSha256 = $state("");
  artifactSize = $state("");
  acquisitionBusy = $state(false);
  acquisition = $state<AcquiredArtifact | null>(null);
  acquisitionError = $state<LauncherBackendError | null>(null);
  minecraftVersion = $state("");
  planningBusy = $state(false);
  planSummary = $state<MinecraftPlanSummary | null>(null);
  planningError = $state<LauncherBackendError | null>(null);
  fabricMinecraftVersion = $state("");
  fabricLoaderVersion = $state("");
  fabricPlanningBusy = $state(false);
  fabricPlanSummary = $state<FabricPlanSummary | null>(null);
  fabricPlanningError = $state<LauncherBackendError | null>(null);
  installInstanceId = $state("");
  installMinecraftVersion = $state("");
  installLoaderVersion = $state("");
  installBusy = $state(false);
  installProgress = $state<InstallProgressEvent | null>(null);
  installSummary = $state<InstalledGameSummary | null>(null);
  installError = $state<LauncherBackendError | null>(null);
  validationBusy = $state(false);
  validation = $state<InstalledGameValidation | null>(null);
  validationError = $state<LauncherBackendError | null>(null);

  private initialized = false;
  private unsubscribers: (() => void)[] = [];

  get selectedInstance(): InstanceSummary | null {
    const selectedId = this.launcherState?.config.selectedInstanceId;
    if (!selectedId) return null;
    return this.launcherState?.instances.find((instance) => instance.id === selectedId) ?? null;
  }

  get selectedAccount(): AccountSummary | null {
    const selectedId = this.accountsState?.selectedAccountId;
    if (!selectedId) return null;
    return this.accountsState?.accounts.find((account) => account.accountId === selectedId) ?? null;
  }

  /** Initial load and native event wiring; call once from the root page. */
  initialize(): void {
    if (this.initialized) return;
    this.initialized = true;

    void this.loadInitialStatus();
    void this.subscribeToEvents();
  }

  dispose(): void {
    for (const stop of this.unsubscribers) stop();
    this.unsubscribers = [];
    this.initialized = false;
  }

  private async loadInitialStatus(): Promise<void> {
    try {
      this.status = await getApplicationStatus();
    } catch (cause: unknown) {
      this.statusError = backendError(cause, "The launcher status could not be loaded.");
    }

    try {
      this.launcherState = await getLauncherState();
      const selected = this.selectedInstance;
      if (selected?.state === "ready") void this.runRuntimeStatus(selected.id);
    } catch (cause: unknown) {
      this.stateError = backendError(cause, "The launcher state could not be loaded.");
    }

    try {
      this.releases = await listAuroraReleases();
    } catch (cause: unknown) {
      this.releasesError = backendError(cause, "The Aurora release list failed.");
    }

    try {
      this.accountsState = await getAccounts();
      void this.refreshPlayReadiness();
    } catch (cause: unknown) {
      this.accountsError = backendError(cause, "The account list could not be loaded.");
    }
  }

  private async subscribeToEvents(): Promise<void> {
    // Native progress events drive the installation displays.
    listen<InstallProgressEvent>("install-progress", (event) => {
      this.installProgress = event.payload;
    }).then((stop) => {
      this.unsubscribers.push(stop);
    });
    listen<InstanceProgressEvent>("instance-progress", (event) => {
      this.createProgress = event.payload;
    }).then((stop) => {
      this.unsubscribers.push(stop);
    });
    listen<RuntimeProgressEvent>("runtime-progress", (event) => {
      this.runtimeProgress = event.payload;
    }).then((stop) => {
      this.unsubscribers.push(stop);
    });
    listen<AuthProgressEvent>("auth-progress", (event) => {
      this.signInProgress = event.payload;
    }).then((stop) => {
      this.unsubscribers.push(stop);
    });
    listen<LaunchProgressEvent>("launch-progress", (event) => {
      this.playProgress = event.payload;
    }).then((stop) => {
      this.unsubscribers.push(stop);
    });
    listen<LaunchProcess>("launch-state", (event) => {
      this.playProcess = event.payload;
      if (event.payload.status === "exited" || event.payload.status === "failed") {
        this.playBusy = false;
        this.playProgress = null;
        void this.refreshPlayReadiness();
      }
    }).then((stop) => {
      this.unsubscribers.push(stop);
    });
  }

  async refreshState(): Promise<void> {
    try {
      this.launcherState = await getLauncherState();
    } catch {
      // State refresh is best-effort after mutations; load errors surface
      // through the dedicated state card.
    }
  }

  async refreshPlayReadiness(): Promise<void> {
    const instanceId = this.launcherState?.config.selectedInstanceId;
    if (!instanceId) {
      this.playReadiness = null;
      return;
    }
    this.playReadinessBusy = true;
    try {
      this.playReadiness = await getPlayReadiness(
        instanceId,
        this.accountsState?.selectedAccountId ?? null,
      );
      this.playError = null;
    } catch (cause: unknown) {
      this.playReadiness = null;
      this.playError = backendError(cause, "Play readiness could not be loaded.");
    } finally {
      this.playReadinessBusy = false;
    }
  }

  async runPlay(instanceId: string): Promise<void> {
    const accountId = this.accountsState?.selectedAccountId;
    if (!accountId || !this.playReadiness?.ready) return;
    this.playBusy = true;
    this.playProgress = { phase: "checkingPreconditions" };
    this.playError = null;
    try {
      this.playProcess = await playInstance(instanceId, accountId);
    } catch (cause: unknown) {
      this.playError = backendError(cause, "Minecraft could not be started.");
      this.playBusy = false;
      this.playProgress = null;
      void this.refreshPlayReadiness();
    }
  }

  async loadMinecraftVersions(includeSnapshots: boolean): Promise<void> {
    this.minecraftVersionsBusy = true;
    this.minecraftVersionsError = null;
    try {
      this.minecraftVersions = await listMinecraftVersions(includeSnapshots);
    } catch (cause: unknown) {
      this.minecraftVersionsError = backendError(
        cause,
        "The Minecraft version list could not be loaded.",
      );
    } finally {
      this.minecraftVersionsBusy = false;
    }
  }

  async loadLoaderVersions(minecraftVersion: string): Promise<void> {
    if (minecraftVersion.trim() === "") {
      this.loaderVersions = null;
      return;
    }
    this.loaderVersionsBusy = true;
    this.loaderVersionsError = null;
    try {
      this.loaderVersions = await listFabricLoaderVersions(minecraftVersion.trim());
    } catch (cause: unknown) {
      this.loaderVersions = null;
      this.loaderVersionsError = backendError(
        cause,
        "The Fabric Loader versions could not be loaded.",
      );
    } finally {
      this.loaderVersionsBusy = false;
    }
  }

  async runCreateInstance(): Promise<void> {
    this.createBusy = true;
    this.createProgress = null;
    this.createError = null;

    try {
      await createInstance({
        displayName: this.createDisplayName.trim(),
        minecraftVersion: this.createMinecraftVersion,
        loaderPolicy: this.createLoaderPolicy,
      });
      this.createDisplayName = "";
      await this.refreshState();
    } catch (cause: unknown) {
      this.createError = backendError(cause, "The instance creation failed.");
      await this.refreshState();
    } finally {
      this.createBusy = false;
    }
  }

  /**
   * Seeds one instance's settings draft from its current desired
   * configuration. An existing draft is left untouched, so unsaved edits
   * survive moving between instances and tabs.
   */
  openDetail(id: string): void {
    const instance = this.launcherState?.instances.find((entry) => entry.id === id);
    if (!instance) return;
    if (this.detailDrafts[id] === undefined) {
      this.detailDrafts[id] = $state.snapshot(instance.configuration);
    }
    this.detailError = null;
    if (this.loaderVersions === null) {
      void this.loadLoaderVersions(instance.configuration.minecraftVersion);
    }
  }

  /** The instance's live draft, or null before the editor has opened it. */
  draftFor(id: string): InstanceConfiguration | null {
    return this.detailDrafts[id] ?? null;
  }

  /** Drops one instance's unsaved draft by reseeding it from saved state. */
  discardDraft(id: string): void {
    const instance = this.launcherState?.instances.find((entry) => entry.id === id);
    if (!instance) {
      delete this.detailDrafts[id];
      return;
    }
    this.detailDrafts[id] = $state.snapshot(instance.configuration);
  }

  /** Saves one instance's whole proposed configuration atomically. */
  async runSaveConfiguration(id: string): Promise<void> {
    const draft = this.detailDrafts[id];
    if (!draft || this.detailBusy !== null) return;
    this.detailBusy = id;
    this.detailError = null;
    try {
      await updateInstanceConfiguration(id, draft);
      await this.refreshState();
      void this.refreshPlayReadiness();
      // Reflect the persisted draft back from Rust-owned state.
      const updated = this.launcherState?.instances.find((entry) => entry.id === id);
      if (updated) this.detailDrafts[id] = $state.snapshot(updated.configuration);
    } catch (cause: unknown) {
      this.detailError = backendError(cause, "The configuration could not be saved.");
    } finally {
      this.detailBusy = null;
    }
  }

  /**
   * Installs one instance's saved configuration (install-affecting changes).
   */
  async runInstallConfiguration(id: string): Promise<void> {
    if (this.detailInstallBusy !== null) return;
    this.detailInstallBusy = id;
    this.detailError = null;
    try {
      await installInstanceConfiguration(id);
      await this.refreshState();
      void this.refreshPlayReadiness();
      const updated = this.launcherState?.instances.find((entry) => entry.id === id);
      if (updated) this.detailDrafts[id] = $state.snapshot(updated.configuration);
    } catch (cause: unknown) {
      this.detailError = backendError(cause, "The new configuration could not be installed.");
      await this.refreshState();
    } finally {
      this.detailInstallBusy = null;
    }
  }

  /**
   * Opens one instance's managed folder in the OS file browser. The folder
   * is derived entirely by Rust from the validated instance id.
   */
  async runOpenFolder(id: string): Promise<void> {
    if (this.folderBusy !== null) return;
    this.folderBusy = id;
    this.folderError = null;
    try {
      await openInstanceFolder(id);
    } catch (cause: unknown) {
      this.folderError = backendError(cause, "The instance folder could not be opened.");
    } finally {
      this.folderBusy = null;
    }
  }

  async runLoadMods(id: string): Promise<void> {
    if (this.modInventoryBusy === id) return;
    this.modInventoryBusy = id;
    this.modError = null;
    try {
      this.modInventories[id] = await getInstanceMods(id);
    } catch (cause: unknown) {
      this.modError = backendError(cause, "The local mod inventory could not be loaded.");
    } finally {
      this.modInventoryBusy = null;
    }
  }

  async runSetModEnabled(id: string, entryId: string, enabled: boolean): Promise<void> {
    if (this.modMutationBusy !== null) return;
    this.modMutationBusy = entryId;
    this.modError = null;
    try {
      this.modInventories[id] = await setInstanceModEnabled(id, entryId, enabled);
    } catch (cause: unknown) {
      const error = backendError(cause, "The mod state could not be changed.");
      await this.runLoadMods(id);
      this.modError = error;
    } finally {
      this.modMutationBusy = null;
    }
  }

  async runRemoveMod(id: string, entryId: string): Promise<void> {
    if (this.modMutationBusy !== null) return;
    this.modMutationBusy = entryId;
    this.modError = null;
    try {
      this.modInventories[id] = await removeInstanceMod(id, entryId);
    } catch (cause: unknown) {
      const error = backendError(cause, "The mod could not be removed.");
      await this.runLoadMods(id);
      this.modError = error;
    } finally {
      this.modMutationBusy = null;
    }
  }

  async runOpenModsFolder(id: string): Promise<void> {
    if (this.modFolderBusy !== null) return;
    this.modFolderBusy = id;
    this.modError = null;
    try {
      await openInstanceModsFolder(id);
    } catch (cause: unknown) {
      this.modError = backendError(cause, "The instance mods folder could not be opened.");
    } finally {
      this.modFolderBusy = null;
    }
  }

  /**
   * The workspace header's contextual Play. It composes the same single
   * launch pipeline Home uses — select (when this instance is not the
   * selected one), refresh Rust's readiness decision, then launch only if
   * that decision says ready. There is no second readiness or launch path.
   */
  async runWorkspacePlay(instanceId: string): Promise<void> {
    if (instanceId !== this.launcherState?.config.selectedInstanceId) {
      await this.runSelect(instanceId);
    }
    await this.refreshPlayReadiness();
    if (this.playReadiness?.instanceId === instanceId && this.playReadiness.ready) {
      await this.runPlay(instanceId);
    }
  }

  async runSelect(id: string): Promise<void> {
    this.instanceBusy = id;
    this.instanceError = null;
    try {
      await selectInstance(id);
      await this.refreshState();
      this.runtimeStatus = null;
      void this.runRuntimeStatus(id);
      void this.refreshPlayReadiness();
    } catch (cause: unknown) {
      this.instanceError = backendError(cause, "The selection failed.");
    } finally {
      this.instanceBusy = null;
    }
  }

  async runRename(): Promise<void> {
    if (!this.renaming) return;
    this.instanceBusy = this.renaming.id;
    this.instanceError = null;
    try {
      await renameInstance(this.renaming.id, this.renaming.name.trim());
      this.renaming = null;
      await this.refreshState();
      void this.refreshPlayReadiness();
    } catch (cause: unknown) {
      this.instanceError = backendError(cause, "The rename failed.");
    } finally {
      this.instanceBusy = null;
    }
  }

  async runRetry(id: string): Promise<void> {
    this.instanceBusy = id;
    this.instanceError = null;
    this.createProgress = null;
    try {
      await retryInstanceInstall(id);
      await this.refreshState();
      void this.refreshPlayReadiness();
    } catch (cause: unknown) {
      this.instanceError = backendError(cause, "The retry failed.");
      await this.refreshState();
    } finally {
      this.instanceBusy = null;
    }
  }

  async runValidate(id: string): Promise<void> {
    this.instanceBusy = id;
    this.instanceError = null;
    try {
      this.instanceValidations[id] = await validateInstance(id);
      void this.refreshPlayReadiness();
    } catch (cause: unknown) {
      this.instanceError = backendError(cause, "The validation failed.");
    } finally {
      this.instanceBusy = null;
    }
  }

  async runRuntimeStatus(id: string): Promise<void> {
    this.runtimeBusy = true;
    this.runtimeError = null;
    this.runtimeProgress = null;
    try {
      this.runtimeStatus = await getInstanceRuntimeStatus(id);
      void this.refreshPlayReadiness();
    } catch (cause: unknown) {
      this.runtimeError = backendError(cause, "The managed Java status failed.");
    } finally {
      this.runtimeBusy = false;
    }
  }

  async runEnsureRuntime(id: string): Promise<void> {
    this.runtimeBusy = true;
    this.runtimeError = null;
    this.runtimeProgress = null;
    try {
      this.runtimeStatus = await ensureInstanceRuntime(id);
      void this.refreshPlayReadiness();
    } catch (cause: unknown) {
      this.runtimeError = backendError(cause, "Managed Java installation failed.");
    } finally {
      this.runtimeBusy = false;
    }
  }

  async refreshAccounts(): Promise<void> {
    try {
      this.accountsState = await getAccounts();
      void this.refreshPlayReadiness();
    } catch {
      // Account refresh is best-effort after mutations; load errors surface
      // through the dedicated account card.
    }
  }

  async runSignIn(): Promise<void> {
    this.signInBusy = true;
    this.signInProgress = null;
    this.signInError = null;
    try {
      await beginMicrosoftLogin();
      await this.refreshAccounts();
    } catch (cause: unknown) {
      this.signInError = backendError(cause, "Microsoft sign-in failed.");
      await this.refreshAccounts();
    } finally {
      this.signInBusy = false;
      this.signInProgress = null;
    }
  }

  async runCancelSignIn(): Promise<void> {
    try {
      await cancelMicrosoftLogin();
    } catch {
      // Cancellation is best-effort; the flow itself reports its outcome.
    }
  }

  async runSelectAccount(id: string): Promise<void> {
    this.accountBusy = id;
    this.accountError = null;
    try {
      await selectAccount(id);
      await this.refreshAccounts();
    } catch (cause: unknown) {
      this.accountError = backendError(cause, "The account selection failed.");
    } finally {
      this.accountBusy = null;
    }
  }

  async runRemoveAccount(id: string): Promise<void> {
    this.accountBusy = id;
    this.accountError = null;
    try {
      await removeAccount(id);
      delete this.accountSessions[id];
      await this.refreshAccounts();
    } catch (cause: unknown) {
      this.accountError = backendError(cause, "Removing the account failed.");
    } finally {
      this.accountBusy = null;
    }
  }

  async runRefreshAccountSession(id: string): Promise<void> {
    this.accountBusy = id;
    this.accountError = null;
    try {
      this.accountSessions[id] = await refreshAccountSession(id);
      await this.refreshAccounts();
    } catch (cause: unknown) {
      this.accountError = backendError(cause, "Restoring the account session failed.");
      await this.refreshAccounts();
    } finally {
      this.accountBusy = null;
    }
  }

  async runAcquisition(): Promise<void> {
    this.acquisitionBusy = true;
    this.acquisition = null;
    this.acquisitionError = null;

    const parsedSize = this.artifactSize.trim() === "" ? null : Number(this.artifactSize);
    try {
      this.acquisition = await acquireArtifact({
        url: this.artifactUrl.trim(),
        sha256: this.artifactSha256.trim(),
        sizeBytes: parsedSize !== null && Number.isFinite(parsedSize) ? parsedSize : null,
      });
    } catch (cause: unknown) {
      this.acquisitionError = backendError(cause, "The artifact acquisition failed.");
    } finally {
      this.acquisitionBusy = false;
    }
  }

  async runPlanning(): Promise<void> {
    this.planningBusy = true;
    this.planSummary = null;
    this.planningError = null;

    try {
      this.planSummary = await planMinecraftInstall({ version: this.minecraftVersion.trim() });
    } catch (cause: unknown) {
      this.planningError = backendError(cause, "The installation plan failed.");
    } finally {
      this.planningBusy = false;
    }
  }

  async runFabricPlanning(): Promise<void> {
    this.fabricPlanningBusy = true;
    this.fabricPlanSummary = null;
    this.fabricPlanningError = null;

    try {
      this.fabricPlanSummary = await planFabricInstall({
        minecraftVersion: this.fabricMinecraftVersion.trim(),
        loaderVersion: this.fabricLoaderVersion.trim(),
      });
    } catch (cause: unknown) {
      this.fabricPlanningError = backendError(cause, "The composed plan failed.");
    } finally {
      this.fabricPlanningBusy = false;
    }
  }

  async runInstall(): Promise<void> {
    this.installBusy = true;
    this.installProgress = null;
    this.installSummary = null;
    this.installError = null;
    this.validation = null;
    this.validationError = null;

    try {
      this.installSummary = await installGame({
        instanceId: this.installInstanceId.trim(),
        minecraftVersion: this.installMinecraftVersion.trim(),
        loaderVersion: this.installLoaderVersion.trim(),
      });
    } catch (cause: unknown) {
      this.installError = backendError(cause, "The installation failed.");
    } finally {
      this.installBusy = false;
    }
  }

  async runValidation(): Promise<void> {
    this.validationBusy = true;
    this.validation = null;
    this.validationError = null;

    try {
      this.validation = await validateInstalledGame({
        instanceId: this.installInstanceId.trim(),
      });
    } catch (cause: unknown) {
      this.validationError = backendError(cause, "The validation failed.");
    } finally {
      this.validationBusy = false;
    }
  }
}

export const launcher = new LauncherStore();
