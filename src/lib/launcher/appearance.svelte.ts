import {
  getAppearance,
  setAppearance,
  LauncherBackendError,
  type AccentSelection,
  type AppearanceState,
} from "$lib/backend";

function backendError(cause: unknown, fallback: string): LauncherBackendError {
  return cause instanceof LauncherBackendError
    ? cause
    : new LauncherBackendError("unknown_error", fallback);
}

/** The accent-family CSS custom properties owned by the appearance system. */
const ACCENT_PROPERTIES = [
  "--color-accent",
  "--color-accent-strong",
  "--color-accent-hover",
  "--color-accent-pressed",
  "--color-accent-contrast",
  "--color-accent-soft",
  "--color-accent-outline",
] as const;

/**
 * The frontend owner of launcher-wide appearance. Rust validates and derives
 * every value; this store only applies the returned theme attribute and
 * accent tokens to the document root and persists user choices.
 *
 * Changes apply live (no restart), and the initial load runs before the app
 * renders (see +layout.ts) so the persisted look is in place for the first
 * paint of the shell.
 */
class AppearanceStore {
  state = $state<AppearanceState | null>(null);
  error = $state<LauncherBackendError | null>(null);
  busy = $state(false);

  private initialized = false;

  get theme(): string {
    return this.state?.theme ?? "aurora-dark";
  }

  get accent(): AccentSelection {
    return this.state?.accent ?? { type: "preset", id: "violet" };
  }

  /** Loads and applies the persisted appearance; safe to call once at boot. */
  async initialize(): Promise<void> {
    if (this.initialized) return;
    this.initialized = true;

    try {
      this.apply(await getAppearance());
    } catch (cause: unknown) {
      // Appearance is cosmetic: a failure never blocks the launcher. The
      // default look stays applied and the Settings page reports the error.
      this.error = backendError(cause, "The launcher appearance could not be loaded.");
    }
  }

  /** Puts a Rust-derived appearance state onto the document root. */
  apply(state: AppearanceState): void {
    this.state = state;
    this.error = null;

    const root = document.documentElement;
    root.dataset.theme = state.theme;

    if (state.accent.type === "preset") {
      delete root.dataset.accentCustom;
      for (const property of ACCENT_PROPERTIES) {
        root.style.removeProperty(property);
      }
      root.dataset.accent = state.accent.id;
    } else {
      delete root.dataset.accent;
      root.dataset.accentCustom = "true";
      const palette = state.palette;
      const values: Record<(typeof ACCENT_PROPERTIES)[number], string> = {
        "--color-accent": palette.accent,
        "--color-accent-strong": palette.accentStrong,
        "--color-accent-hover": palette.accentHover,
        "--color-accent-pressed": palette.accentPressed,
        "--color-accent-contrast": palette.accentContrast,
        "--color-accent-soft": palette.accentSoft,
        "--color-accent-outline": palette.accentOutline,
      };
      for (const property of ACCENT_PROPERTIES) {
        root.style.setProperty(property, values[property]);
      }
    }
  }

  /** Switches the built-in theme: applied live, persisted by Rust. */
  async setTheme(theme: string): Promise<void> {
    if (this.busy || theme === this.theme) return;
    this.busy = true;
    try {
      this.apply(await setAppearance(theme, this.accent));
    } catch (cause: unknown) {
      this.error = backendError(cause, "The theme could not be saved.");
    } finally {
      this.busy = false;
    }
  }

  /** Switches the accent: Rust derives and validates, then this applies it. */
  async setAccent(accent: AccentSelection): Promise<void> {
    if (this.busy) return;
    this.busy = true;
    try {
      this.apply(await setAppearance(this.theme, accent));
    } catch (cause: unknown) {
      this.error = backendError(cause, "The accent could not be saved.");
    } finally {
      this.busy = false;
    }
  }
}

export const appearance = new AppearanceStore();
