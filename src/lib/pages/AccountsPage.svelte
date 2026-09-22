<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
  import { authErrorMessage, authPhaseLabel } from "$lib/authMessages";

  const accounts = $derived(launcher.accountsState?.accounts ?? []);
  const signedIn = $derived(accounts.length > 0);

  function sessionVerified(id: string): boolean {
    return launcher.accountSessions[id] !== undefined;
  }
</script>

<!--
  Account management surface: one obvious primary task when signed out,
  restrained selected/unselected rows when signed in. The UI speaks in
  actionable language — internal error codes never appear here (see
  authMessages.ts), and tokens never leave Rust in the first place.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">Accounts</h2>
      <p class="page-subtitle">Microsoft accounts used to launch Minecraft.</p>
    </div>
    {#if signedIn && !launcher.signInBusy}
      <div class="page-header-actions">
        <button
          type="button"
          class="btn btn-primary"
          onclick={() => launcher.runSignIn()}
          disabled={launcher.accountsState === null || launcher.accountBusy !== null}
        >
          Add account
        </button>
      </div>
    {/if}
  </header>

  {#if launcher.accountsError}
    <section class="group" aria-live="polite">
      <div class="group-heading">
        <div>
          <h3 class="group-title">Accounts could not be loaded</h3>
          <p class="group-subtitle">The persisted account list could not be read.</p>
        </div>
      </div>
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.accountsError.message}
      </p>
    </section>
  {/if}

  {#if launcher.signInBusy}
    <section class="group" aria-live="polite">
      <div class="group-heading">
        <div>
          <h3 class="group-title">Signing in with Microsoft</h3>
          <p class="group-subtitle">
            {launcher.signInProgress
              ? authPhaseLabel(launcher.signInProgress.phase)
              : "Opening the Microsoft sign-in in your browser…"}
          </p>
        </div>
        <span class="status-badge status-working">In progress</span>
      </div>
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <span class="group-row-detail">
          Finish the sign-in in your browser, then return here. The sign-in happens entirely in
          your browser — Aurora never asks for your password.
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-detail">You can cancel if you changed your mind.</span>
        <div class="group-row-actions">
          <button type="button" class="btn" onclick={() => launcher.runCancelSignIn()}>
            Cancel sign-in
          </button>
        </div>
      </div>
      {#if launcher.signInError}
        <p class="inline-message inline-message-error group-row" role="alert">
          {authErrorMessage(launcher.signInError)}
        </p>
      {/if}
    </section>
  {:else if launcher.accountsState === null && !launcher.accountsError}
    <section class="group" aria-live="polite">
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <span class="group-row-detail">Loading accounts…</span>
      </div>
    </section>
  {:else if !signedIn}
    <section class="empty-state" aria-live="polite">
      <h3 class="empty-title">No account signed in</h3>
      <p class="empty-detail">
        Sign in with your Microsoft account to launch Minecraft. Aurora opens the sign-in in your
        system browser and only stores the refresh credential, in the operating system's credential
        store.
      </p>
      <button
        type="button"
        class="btn btn-primary"
        onclick={() => launcher.runSignIn()}
        disabled={launcher.accountsState === null}
      >
        Sign in with Microsoft
      </button>
      {#if launcher.signInError}
        <p class="inline-message inline-message-error" role="alert">
          {authErrorMessage(launcher.signInError)}
        </p>
      {/if}
      {#if launcher.accountError}
        <p class="inline-message inline-message-error" role="alert">
          {authErrorMessage(launcher.accountError)}
        </p>
      {/if}
    </section>
  {:else}
    <section class="group" aria-live="polite">
      <div class="group-heading">
        <div>
          <h3 class="group-title">Your accounts</h3>
          <p class="group-subtitle">The selected account is the one Play launches with.</p>
        </div>
      </div>

      {#each accounts as account (account.accountId)}
        {@const selected = launcher.accountsState?.selectedAccountId === account.accountId}
        {@const busy = launcher.accountBusy === account.accountId}
        <div class="group-row" class:group-row-selected={selected}>
          <div class="group-row-main">
            <span class="account-name-line">
              <span class="group-row-title">{account.minecraftName}</span>
              {#if selected}<span class="row-marker">Selected</span>{/if}
            </span>
            {#if account.status === "reauthenticationRequired"}
              <span class="group-row-detail">
                This account's stored credential is no longer valid — sign in again to use it.
              </span>
            {:else if sessionVerified(account.accountId)}
              <span class="group-row-detail">Session verified — ready to launch.</span>
            {/if}
          </div>
          <div class="group-row-actions">
            {#if account.status === "reauthenticationRequired"}
              <span class="status-badge status-warning">Sign-in required</span>
            {/if}
            {#if !selected}
              <button
                type="button"
                class="btn"
                onclick={() => launcher.runSelectAccount(account.accountId)}
                disabled={busy || launcher.signInBusy}
              >
                Select
              </button>
            {/if}
            <button
              type="button"
              class="btn"
              onclick={() => launcher.runRefreshAccountSession(account.accountId)}
              disabled={busy || launcher.signInBusy}
            >
              {busy ? "Checking…" : "Check session"}
            </button>
            <button
              type="button"
              class="btn btn-danger"
              onclick={() => launcher.runRemoveAccount(account.accountId)}
              disabled={busy || launcher.signInBusy}
            >
              Remove
            </button>
          </div>
        </div>
      {/each}

      {#if launcher.signInError}
        <p class="inline-message inline-message-error group-row" role="alert">
          {authErrorMessage(launcher.signInError)}
        </p>
      {/if}
      {#if launcher.accountError}
        <p class="inline-message inline-message-error group-row" role="alert">
          {authErrorMessage(launcher.accountError)}
        </p>
      {/if}

      <p class="group-footer">
        Sign-in uses your system browser. Aurora stores only the Microsoft refresh credential, in
        the operating system's credential store — never in plain files — and removing an account
        here only removes it from Aurora.
      </p>
    </section>
  {/if}
</div>

<style>
  .account-name-line {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    min-width: 0;
    flex-wrap: wrap;
  }
</style>
