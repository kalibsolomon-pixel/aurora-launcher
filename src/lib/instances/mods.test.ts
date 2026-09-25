import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { ModEntry } from "$lib/backend";
import {
  beginRemoval,
  confirmedRemovalId,
  formatModSize,
  visibleMods,
} from "./mods.ts";

function entry(
  id: string,
  displayName: string,
  options: Partial<ModEntry> = {},
): ModEntry {
  return {
    entryId: id,
    fileName: `${id}.jar`,
    displayName,
    enabled: true,
    fileType: "enabledJar",
    sizeBytes: 1024,
    modifiedUnixMillis: 1,
    ownership: "userManaged",
    sha256: null,
    provenance: null,
    metadata: {
      id,
      name: displayName,
      version: "1.0.0",
      description: null,
      authors: [],
      environment: "client",
      depends: [],
      recommends: [],
      suggests: [],
      conflicts: [],
      breaks: [],
      hasDeclaredIcon: false,
    },
    warnings: [],
    canToggle: true,
    canRemove: true,
    actionBlockedReason: null,
    ...options,
  };
}

const fixture = [
  entry("zeta", "Zeta"),
  entry("alpha", "Alpha", {
    enabled: false,
    fileType: "disabledJar",
    fileName: "alpha.jar.disabled",
    metadata: {
      ...entry("alpha", "Alpha").metadata!,
      authors: ["Ada Lovelace"],
    },
  }),
  entry("warning", "Warning", {
    warnings: [{ code: "required_dependency_missing", message: "Missing helper" }],
  }),
  entry("aurora", "Aurora Client", {
    ownership: "launcherManagedRequired",
    canToggle: false,
    canRemove: false,
    actionBlockedReason: "Required",
  }),
];

describe("local mod list projection", () => {
  it("sorts by display name without changing the source snapshot", () => {
    assert.deepEqual(
      visibleMods(fixture, "", "all", "name").map((item) => item.displayName),
      ["Alpha", "Aurora Client", "Warning", "Zeta"],
    );
    assert.equal(fixture[0]?.displayName, "Zeta");
  });

  it("searches name, mod id, filename, and author locally", () => {
    assert.deepEqual(visibleMods(fixture, "ada", "all", "name").map((item) => item.entryId), ["alpha"]);
    assert.deepEqual(visibleMods(fixture, "alpha.jar.disabled", "all", "name").map((item) => item.entryId), ["alpha"]);
    assert.deepEqual(visibleMods(fixture, "warning", "all", "name").map((item) => item.entryId), ["warning"]);
  });

  it("filters enabled, disabled, and warning entries", () => {
    assert.equal(visibleMods(fixture, "", "enabled", "name").length, 3);
    assert.deepEqual(visibleMods(fixture, "", "disabled", "name").map((item) => item.entryId), ["alpha"]);
    assert.deepEqual(visibleMods(fixture, "", "warnings", "name").map((item) => item.entryId), ["warning"]);
  });

  it("sorts by state and warning count with stable name fallback", () => {
    assert.deepEqual(
      visibleMods(fixture, "", "all", "state").map((item) => item.entryId),
      ["aurora", "warning", "zeta", "alpha"],
    );
    assert.equal(visibleMods(fixture, "", "all", "warnings")[0]?.entryId, "warning");
  });
});

describe("remove confirmation state", () => {
  it("starts for local and proven provider-managed entries, never required files", () => {
    assert.equal(beginRemoval(fixture[3]!), null);
    assert.deepEqual(beginRemoval(fixture[0]!), {
      entryId: "zeta",
      displayName: "Zeta",
      fileName: "zeta.jar",
    });
    assert.equal(beginRemoval(entry("managed", "Managed", { ownership: "providerManaged" }))?.entryId, "managed");
    assert.equal(beginRemoval(entry("unknown", "Unknown", { ownership: "unknown", canRemove: false })), null);
  });

  it("reconciles a confirmation against the latest inventory", () => {
    const candidate = beginRemoval(fixture[0]!);
    assert.equal(confirmedRemovalId(candidate, fixture), "zeta");
    assert.equal(confirmedRemovalId(candidate, fixture.slice(1)), null);
    assert.equal(confirmedRemovalId(null, fixture), null);
  });
});

describe("mod size formatting", () => {
  it("uses compact binary units", () => {
    assert.equal(formatModSize(null), null);
    assert.equal(formatModSize(57), "57 B");
    assert.equal(formatModSize(1536), "1.5 KiB");
    assert.equal(formatModSize(2 * 1024 * 1024), "2.0 MiB");
  });
});
