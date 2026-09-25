import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  type GlobalPage,
  type NavigationState,
  backToInstances,
  activeGlobalPage,
  globalDestinations,
  goToGlobal,
  isGlobalPage,
  openInstance,
  resolveWorkspace,
  selectInstanceTab,
} from "./navigation.ts";

function globalState(page: GlobalPage): NavigationState {
  return { kind: "global", page };
}

describe("global navigation", () => {
  it("navigates between global destinations and leaves any workspace", () => {
    const workspace = openInstance(globalState("home"), "abc123");
    assert.deepEqual(goToGlobal(workspace, "settings"), globalState("settings"));
    assert.deepEqual(goToGlobal(globalState("home"), "about"), globalState("about"));
  });

  it("ignores unknown pages instead of inventing navigation state", () => {
    const state = globalState("instances");
    assert.deepEqual(
      goToGlobal(state, "nonsense" as never),
      state,
    );
    assert.equal(isGlobalPage("nonsense"), false);
    assert.equal(isGlobalPage("instances"), true);
  });

  it("hides the developer destination outside development builds", () => {
    const production = globalDestinations(false).map((destination) => destination.id);
    assert.deepEqual(production, [
      "home",
      "instances",
      "accounts",
      "settings",
      "about",
    ]);
    assert.ok(!production.includes("developer"));

    const development = globalDestinations(true).map((destination) => destination.id);
    assert.deepEqual(development, [
      "home",
      "instances",
      "accounts",
      "settings",
      "about",
      "developer",
    ]);
  });

  it("reports the workspace as owned by the Instances destination", () => {
    assert.equal(activeGlobalPage(globalState("accounts")), "accounts");
    assert.equal(
      activeGlobalPage(openInstance(globalState("home"), "abc123")),
      "instances",
    );
  });
});

describe("instance workspace navigation", () => {
  it("opens a workspace on the Overview tab by default", () => {
    assert.deepEqual(openInstance(globalState("home"), "abc123"), {
      kind: "instance",
      instanceId: "abc123",
      tab: "overview",
    });
  });

  it("keeps the current tab when reopening the same instance", () => {
    const state = openInstance(globalState("instances"), "abc123", "settings");
    assert.deepEqual(openInstance(state, "abc123"), {
      kind: "instance",
      instanceId: "abc123",
      tab: "settings",
    });
  });

  it("switches instances deliberately and never changes selection semantics", () => {
    const first = openInstance(globalState("instances"), "first", "settings");
    const second = openInstance(first, "second");
    assert.deepEqual(second, {
      kind: "instance",
      instanceId: "second",
      tab: "overview",
    });
  });

  it("ignores blank instance ids", () => {
    const state = globalState("home");
    assert.deepEqual(openInstance(state, "  "), state);
  });

  it("switches workspace tabs without touching global state", () => {
    const state = openInstance(globalState("instances"), "abc123");
    const switched = selectInstanceTab(state, "settings");
    assert.deepEqual(switched, {
      kind: "instance",
      instanceId: "abc123",
      tab: "settings",
    });
    assert.deepEqual(selectInstanceTab(switched, "settings"), switched);
    assert.deepEqual(selectInstanceTab(globalState("instances"), "overview"), globalState("instances"));
  });

  it("places the implemented Mods tab between Overview and Settings", () => {
    const state = openInstance(globalState("instances"), "abc123");
    assert.deepEqual(selectInstanceTab(state, "mods"), {
      kind: "instance",
      instanceId: "abc123",
      tab: "mods",
    });
  });

  it("navigates back to the Instances page from a workspace", () => {
    const state = openInstance(globalState("home"), "abc123", "settings");
    assert.deepEqual(backToInstances(), globalState("instances"));
    assert.notEqual(state.kind, "global");
  });
});

describe("workspace resolution against known instances", () => {
  it("is loading until the registry state is known", () => {
    assert.deepEqual(resolveWorkspace(openInstance(globalState("instances"), "abc123"), null), {
      status: "loading",
    });
    assert.deepEqual(resolveWorkspace(globalState("instances"), ["abc123"]), {
      status: "loading",
    });
  });

  it("reports a deleted or unknown instance explicitly", () => {
    assert.deepEqual(resolveWorkspace(openInstance(globalState("instances"), "gone"), ["abc123"]), {
      status: "missing",
    });
  });

  it("opens with the active instance identity and tab", () => {
    const state = openInstance(globalState("instances"), "abc123", "settings");
    assert.deepEqual(resolveWorkspace(state, ["abc123", "other"]), {
      status: "open",
      instanceId: "abc123",
      tab: "settings",
    });
  });
});
