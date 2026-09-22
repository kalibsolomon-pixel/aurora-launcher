<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
</script>

<!--
  Existing account surface, intentionally not redesigned in the shell/Home
  pilot: the internal content keeps its established presentation and every
  control until its own design phase.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">Accounts</h2>
      <p class="page-subtitle">Microsoft accounts used to launch Minecraft.</p>
    </div>
  </header>

  <section class="status-card" aria-labelledby="accounts-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Microsoft &amp; Minecraft authentication</p>
        <h3 id="accounts-title">Account</h3>
      </div>

      {#if launcher.signInBusy}
        <span class="badge loading"><span aria-hidden="true"></span
          >{launcher.signInProgress ? launcher.signInProgress.phase : "Waiting for Microsoft"}</span
        >
      {:else if launcher.accountsState && launcher.accountsState.accounts.length > 0}
        <span class="badge ready"><span aria-hidden="true"></span>Signed in</span>
      {:else if launcher.accountsError}
        <span class="badge error"><span aria-hidden="true"></span>Unavailable</span>
      {:else}
        <span class="badge loading"><span aria-hidden="true"></span>Not signed in</span>
      {/if}
    </div>

    {#if launcher.accountsError}
      <div class="error-message" role="alert">
        <p>{launcher.accountsError.message}</p>
        <code>{launcher.accountsError.code}</code>
      </div>
    {/if}

    {#if launcher.signInBusy}
      <dl>
        <div>
          <dt>Sign-in</dt>
          <dd>
            Complete the Microsoft sign-in in your browser, then return here.
            {#if launcher.signInProgress}Current step: {launcher.signInProgress.phase}{/if}
          </dd>
        </div>
      </dl>
      <form class="acquire-form" onsubmit={(event) => launcher.runCancelSignIn(event)}>
        <button type="submit">Cancel sign-in</button>
      </form>
    {:else}
      <form class="acquire-form" onsubmit={(event) => launcher.runSignIn(event)}>
        <button type="submit" disabled={launcher.accountsState === null}>
          {launcher.accountsState && launcher.accountsState.accounts.length > 0
            ? "Add another account"
            : "Sign in with Microsoft"}
        </button>
      </form>
    {/if}

    {#if launcher.signInError}
      <div class="error-message" role="alert">
        <p>{launcher.signInError.message}</p>
        <code>{launcher.signInError.code}</code>
      </div>
    {/if}

    {#if launcher.accountError}
      <div class="error-message" role="alert">
        <p>{launcher.accountError.message}</p>
        <code>{launcher.accountError.code}</code>
      </div>
    {/if}

    {#if launcher.accountsState && launcher.accountsState.accounts.length === 0 && !launcher.signInBusy}
      <p class="footnote">Not signed in.</p>
    {/if}

    {#if launcher.accountsState}
      {#each launcher.accountsState.accounts as account (account.accountId)}
      <div class="instance-row">
        <div class="instance-main">
          <div class="instance-title">
            <strong>{account.minecraftName}</strong>
            {#if launcher.accountsState.selectedAccountId === account.accountId}
              <span class="badge ready"><span aria-hidden="true"></span>Selected</span>
            {/if}
            {#if account.status === "reauthenticationRequired"}
              <span class="badge error"><span aria-hidden="true"></span>Sign-in required</span>
            {:else}
              <span class="badge ready"><span aria-hidden="true"></span>Signed in</span>
            {/if}
          </div>
          {#if launcher.accountSessions[account.accountId]}
            <div class="instance-meta validation-line">
              Session: ready
            </div>
          {/if}
        </div>

        <div class="instance-actions">
          {#if launcher.accountsState.selectedAccountId !== account.accountId}
            <button
              type="button"
              onclick={() => launcher.runSelectAccount(account.accountId)}
              disabled={launcher.accountBusy === account.accountId || launcher.signInBusy}
            >
              Select
            </button>
          {/if}
          <button
            type="button"
            onclick={() => launcher.runRefreshAccountSession(account.accountId)}
            disabled={launcher.accountBusy === account.accountId || launcher.signInBusy}
          >
            Check session
          </button>
          <button
            type="button"
            onclick={() => launcher.runRemoveAccount(account.accountId)}
            disabled={launcher.accountBusy === account.accountId || launcher.signInBusy}
          >
            Remove account
          </button>
        </div>
      </div>
      {/each}
    {/if}

    <p class="footnote">
      Sign-in uses your system browser, and only the Microsoft refresh credential is stored —
      in the operating system's credential store, never in plain files. Accounts are separate
      from instances and the selected account is supplied to Play without exposing its token.
    </p>
  </section>
</div>

<style>
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

  .eyebrow {
    margin: 0 0 0.45rem;
    color: #a99dff;
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  h3 {
    margin: 0;
    font-size: 1.25rem;
    letter-spacing: -0.02em;
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
  .error-message {
    margin: 0;
    padding: 1rem 1.5rem;
    color: #818ca4;
    font-size: 0.82rem;
  }

  .acquire-form {
    display: grid;
    gap: 0.9rem;
    padding: 1.25rem 1.5rem 0.5rem;
  }

  .acquire-form button {
    justify-self: start;
    padding: 0.55rem 1.1rem;
    border: none;
    border-radius: 8px;
    background: #6f5df2;
    color: #ffffff;
    font: inherit;
    font-size: 0.88rem;
    font-weight: 700;
    cursor: pointer;
  }

  .acquire-form button:disabled {
    opacity: 0.6;
    cursor: progress;
  }

  .instance-row {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 1rem;
    padding: 1rem 1.5rem;
    border-bottom: 1px solid #20263a;
  }

  .instance-main {
    min-width: 0;
    flex: 1 1 18rem;
  }

  .instance-title {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
  }

  .instance-meta {
    margin-top: 0.35rem;
    color: #818ca4;
    font-size: 0.84rem;
    overflow-wrap: anywhere;
  }

  .instance-actions {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .instance-actions button {
    padding: 0.4rem 0.85rem;
    border: 1px solid #2a3149;
    border-radius: 8px;
    background: #1a2032;
    color: #e7ecf7;
    font: inherit;
    font-size: 0.82rem;
    font-weight: 600;
    cursor: pointer;
  }

  .instance-actions button:hover:not(:disabled) {
    border-color: #6f5df2;
  }

  .instance-actions button:disabled {
    opacity: 0.55;
    cursor: progress;
  }

  .error-message code {
    display: inline-block;
    margin-top: 0.7rem;
    color: #ff9a9a;
  }
</style>
