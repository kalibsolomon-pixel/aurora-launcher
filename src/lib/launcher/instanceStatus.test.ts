import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  configurationLabel,
  configurationRequiresInstall,
  draftIsDirty,
  instanceContentStatus,
  javaRuntimeStatus,
} from "./instanceStatus.ts";
import type {
  InstanceConfiguration,
  InstanceSummary,
  InstanceValidationDto,
  RuntimeStatusDto,
} from "$lib/backend";

function configuration(
  overrides: Partial<InstanceConfiguration> = {},
): InstanceConfiguration {
  return {
    minecraftVersion: "26.2",
    loader: { kind: "fabric", policy: { type: "automatic" } },
    memoryMib: 2048,
    additionalJvmArguments: "",
    window: null,
    ...overrides,
  };
}

function instance(
  overrides: Partial<InstanceSummary> = {},
): InstanceSummary {
  return {
    id: "abc123",
    displayName: "Fixture Instance",
    state: "ready",
    channel: "stable",
    auroraVersion: "0.3.0",
    minecraftVersion: "26.2",
    fabricLoaderVersion: "0.19.5",
    configuration: configuration(),
    ...overrides,
  };
}

function validation(
  status: InstanceValidationDto["status"],
  problems: InstanceValidationDto["problems"] = [],
): InstanceValidationDto {
  return {
    instanceId: "abc123",
    displayName: "Fixture Instance",
    status,
    problems,
  };
}

function runtime(
  overrides: Partial<RuntimeStatusDto> = {},
): RuntimeStatusDto {
  return {
    instanceId: "abc123",
    contentStatus: "ready",
    status: "ready",
    component: "java-runtime-epsilon",
    requiredMajorVersion: 25,
    runtimeVersion: "25.0.1",
    runtimeRoot: "C:\\managed\\runtimes\\fixture",
    launchExecutable: "C:\\managed\\runtimes\\fixture\\bin\\javaw.exe",
    checkedFiles: 411,
    verifiedBytes: 105080003,
    reportedMajorVersion: 25,
    diagnosticSummary: "openjdk version \"25.0.1\"",
    problems: [],
    reused: null,
    ...overrides,
  };
}

describe("configuration-versus-pin comparison", () => {
  it("flags a changed Minecraft version on a ready instance", () => {
    const changed = instance({
      configuration: configuration({ minecraftVersion: "26.1" }),
    });
    assert.equal(configurationRequiresInstall(changed), true);
  });

  it("flags a changed pinned loader version", () => {
    const changed = instance({
      configuration: configuration({
        loader: { kind: "fabric", policy: { type: "pinned", version: "0.19.4" } },
      }),
    });
    assert.equal(configurationRequiresInstall(changed), true);
  });

  it("ignores the installed loader under the automatic policy", () => {
    const automatic = instance();
    assert.equal(configurationRequiresInstall(automatic), false);
  });

  it("never flags a non-ready instance", () => {
    const installing = instance({
      state: "installing",
      configuration: configuration({ minecraftVersion: "26.1" }),
    });
    assert.equal(configurationRequiresInstall(installing), false);
  });

  it("labels the desired configuration compactly", () => {
    assert.equal(
      configurationLabel(instance()),
      "Minecraft 26.2 · Fabric (latest compatible)",
    );
    assert.equal(
      configurationLabel(
        instance({
          configuration: configuration({
            loader: { kind: "fabric", policy: { type: "pinned", version: "0.19.5" } },
          }),
        }),
      ),
      "Minecraft 26.2 · Fabric 0.19.5",
    );
  });
});

describe("content status decisions", () => {
  it("shows an installing instance as working with the live phase", () => {
    const decision = instanceContentStatus(
      instance({ state: "installing" }),
      undefined,
      "installingGame",
    );
    assert.deepEqual(decision, {
      tone: "status-working",
      label: "Installing",
      detail: "installingGame",
    });
  });

  it("shows a damaged instance with its first concrete problem", () => {
    const decision = instanceContentStatus(
      instance(),
      validation("damaged", [{ component: "game", reason: "missing file" }]),
      null,
    );
    assert.equal(decision.tone, "status-error");
    assert.equal(decision.label, "Damaged");
    assert.equal(decision.detail, "game: missing file");
  });

  it("shows needs-install for both the stale gate and a differing configuration", () => {
    const viaValidation = instanceContentStatus(
      instance(),
      validation("stale"),
      null,
    );
    const viaConfiguration = instanceContentStatus(
      instance({ configuration: configuration({ minecraftVersion: "26.1" }) }),
      undefined,
      null,
    );
    for (const decision of [viaValidation, viaConfiguration]) {
      assert.equal(decision.tone, "status-warning");
      assert.equal(decision.label, "Needs install");
    }
  });

  it("shows ready for deep-validated and plain ready instances", () => {
    const validated = instanceContentStatus(instance(), validation("ready"), null);
    assert.equal(validated.label, "Ready");
    assert.equal(validated.detail, "Deep validation passed.");

    const unvalidated = instanceContentStatus(instance(), undefined, null);
    assert.equal(unvalidated.label, "Ready");
    assert.equal(unvalidated.detail, "Installed and complete.");
  });

  it("shows not installed when no game content exists", () => {
    const decision = instanceContentStatus(instance(), validation("notInstalled"), null);
    assert.equal(decision.tone, "status-muted");
    assert.equal(decision.label, "Not installed");
  });
});

describe("Home and workspace readiness consistency", () => {
  // The acceptance rule: the same underlying instance state must produce
  // the same UI decision on Home and the workspace Overview. Both surfaces
  // call this one derivation, so the matrix below is the deterministic
  // proof — one shared decision function, exercised across every state.
  const states: Array<[string, InstanceSummary, InstanceValidationDto | undefined]> = [
    ["ready-unvalidated", instance(), undefined],
    ["ready-validated", instance(), validation("ready")],
    [
      "stale-configuration",
      instance({ configuration: configuration({ minecraftVersion: "26.1" }) }),
      undefined,
    ],
    ["stale-validated", instance(), validation("stale")],
    [
      "damaged",
      instance(),
      validation("damaged", [{ component: "aurora", reason: "hash drift" }]),
    ],
    ["installing", instance({ state: "installing" }), undefined],
    ["not-installed", instance(), validation("notInstalled")],
  ];

  for (const [name, fixture, validationResult] of states) {
    it(`derives one decision for ${name} on every surface`, () => {
      const home = instanceContentStatus(fixture, validationResult, null);
      const overview = instanceContentStatus(fixture, validationResult, null);
      const sidebarBadge = {
        tone: instanceContentStatus(fixture, validationResult, null).tone,
        label: instanceContentStatus(fixture, validationResult, null).label,
      };
      assert.deepEqual(home, overview);
      assert.deepEqual(sidebarBadge, { tone: home.tone, label: home.label });
    });
  }
});

describe("draft dirty derivation", () => {
  it("is clean for an untouched draft", () => {
    const saved = configuration();
    assert.equal(draftIsDirty({ ...saved }, saved), false);
  });

  it("is dirty for changed memory, JVM arguments, and window size", () => {
    const saved = configuration();
    assert.equal(
      draftIsDirty(configuration({ memoryMib: 4096 }), saved),
      true,
    );
    assert.equal(
      draftIsDirty(configuration({ additionalJvmArguments: "-Dx=1" }), saved),
      true,
    );
    assert.equal(
      draftIsDirty(
        configuration({ window: { width: 854, height: 480 } }),
        saved,
      ),
      true,
    );
  });

  it("ignores equivalent deep-equal structures", () => {
    const saved = configuration({
      loader: { kind: "fabric", policy: { type: "pinned", version: "0.19.5" } },
      window: { width: 854, height: 480 },
    });
    const draft = configuration({
      loader: { kind: "fabric", policy: { type: "pinned", version: "0.19.5" } },
      window: { width: 854, height: 480 },
    });
    assert.equal(draftIsDirty(draft, saved), false);
  });
});

describe("managed Java status decisions", () => {
  it("is unchecked before any status query", () => {
    assert.deepEqual(javaRuntimeStatus(null, false, null, null), {
      tone: "status-muted",
      label: "Not checked",
      detail: null,
    });
  });

  it("shows ready with component and version detail", () => {
    const decision = javaRuntimeStatus(runtime(), false, null, null);
    assert.equal(decision.tone, "status-success");
    assert.equal(decision.label, "Ready");
    assert.equal(decision.detail, "java-runtime-epsilon · Java 25 · 25.0.1");
  });

  it("shows damaged and missing runtimes as actionable", () => {
    assert.equal(
      javaRuntimeStatus(runtime({ status: "damaged", problems: ["file missing"] }), false, null, null)
        .label,
      "Damaged",
    );
    assert.equal(
      javaRuntimeStatus(runtime({ status: "missing", runtimeVersion: null }), false, null, null)
        .label,
      "Not installed",
    );
  });

  it("shows busy and error states", () => {
    assert.equal(
      javaRuntimeStatus(null, true, null, "validating").label,
      "Checking…",
    );
    assert.equal(
      javaRuntimeStatus(null, false, "the query failed", null).label,
      "Status failed",
    );
  });
});
