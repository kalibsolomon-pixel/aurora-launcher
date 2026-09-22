import type { AuthProgressEvent, LauncherBackendError } from "$lib/backend";

/**
 * User-facing wording for the authentication surface. The normal Accounts
 * UI speaks in concise, actionable language; internal structured error codes
 * stay out of it (they remain visible to developer tooling and logs).
 */

/** Friendly text for one coarse native sign-in phase. */
export function authPhaseLabel(phase: AuthProgressEvent["phase"]): string {
  switch (phase) {
    case "waitingForMicrosoft":
      return "Waiting for you to finish signing in with Microsoft…";
    case "exchangingMicrosoftToken":
      return "Completing the Microsoft sign-in…";
    case "authenticatingWithXbox":
      return "Signing in to Xbox Live…";
    case "authorizingXsts":
      return "Authorizing with Xbox…";
    case "authenticatingMinecraft":
      return "Signing in to Minecraft services…";
    case "checkingEntitlement":
      return "Verifying Minecraft ownership…";
    case "fetchingProfile":
      return "Loading your Minecraft profile…";
    case "savingAccount":
      return "Saving the account…";
    case "restoringSession":
      return "Restoring the session…";
  }
}

const FRIENDLY_AUTH_ERRORS: Record<string, string> = {
  auth_configuration_missing:
    "Microsoft sign-in is not configured in this build of Aurora.",
  auth_login_in_progress:
    "A sign-in is already in progress. Finish or cancel it, then try again.",
  auth_login_cancelled: "Sign-in was cancelled.",
  auth_login_timeout:
    "Sign-in timed out waiting for the browser. Start again when you are ready.",
  auth_browser_open_failure:
    "The system browser could not be opened for sign-in.",
  auth_entitlement_missing:
    "This Microsoft account does not own Minecraft: Java Edition, so it cannot be used to play.",
  auth_profile_missing:
    "This account owns Minecraft but has no Minecraft profile yet. Create a profile in the official Minecraft launcher first.",
  auth_reauthentication_required: "This account must sign in again.",
};

/**
 * Concise actionable text for a known authentication failure; anything
 * unmapped keeps the backend's sanitized user-readable message.
 */
export function authErrorMessage(error: LauncherBackendError): string {
  return FRIENDLY_AUTH_ERRORS[error.code] ?? error.message;
}
