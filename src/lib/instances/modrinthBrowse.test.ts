import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { ModrinthSearchPage } from "../backend";
import { appendBrowsePage, DefaultBrowseCache, TransientNotice } from "./modrinthBrowse.ts";

function page(offset: number, id: string): ModrinthSearchPage {
  return {
    offset,
    totalHits: 40,
    hits: [{ projectId: id, title: id, summary: "", author: "", downloads: 0, iconUrl: null, projectType: "mod" }],
  };
}

describe("Modrinth browse state", () => {
  it("keeps default results separate by instance and domain and expires them", () => {
    const cache = new DefaultBrowseCache(45_000);
    cache.put("one", "mod", "1.21.11", page(0, "mod"), 100);
    cache.put("one", "shaderPack", "1.21.11", page(0, "shader"), 100);
    assert.equal(cache.get("one", "mod", "1.21.11", 200)?.hits[0]?.projectId, "mod");
    assert.equal(cache.get("one", "shaderPack", "1.21.11", 200)?.hits[0]?.projectId, "shader");
    assert.equal(cache.get("two", "mod", "1.21.11", 200), null);
    assert.equal(cache.get("one", "resourcePack", "1.21.11", 200), null);
    assert.equal(cache.get("one", "mod", "1.20.1", 200), null);
    assert.equal(cache.get("one", "mod", "1.21.11", 45_100), null);
  });

  it("resets on a new first page and appends only subsequent pages", () => {
    const first = page(0, "old");
    assert.deepEqual(appendBrowsePage(first, page(0, "new"), 0).hits.map((hit) => hit.projectId), ["new"]);
    assert.deepEqual(appendBrowsePage(first, page(20, "next"), 20).hits.map((hit) => hit.projectId), ["old", "next"]);
  });

  it("fades at 2.7 seconds, removes at 3 seconds, and cancels stale timers", () => {
    let now = 0;
    let id = 0;
    const pending = new Map<number, { at: number; callback: () => void }>();
    const schedule = (callback: () => void, delay: number): ReturnType<typeof setTimeout> => {
      const next = ++id;
      pending.set(next, { at: now + delay, callback });
      return next as unknown as ReturnType<typeof setTimeout>;
    };
    const cancel = (timer: ReturnType<typeof setTimeout>) => { pending.delete(timer as unknown as number); };
    const state: Array<[string, boolean]> = [];
    const notice = new TransientNotice((message, exiting) => state.push([message, exiting]), schedule, cancel);
    const advance = (time: number) => {
      now = time;
      for (const [key, task] of [...pending]) if (task.at <= now) { pending.delete(key); task.callback(); }
    };
    notice.show("No compatible version is available for this instance.");
    advance(2000);
    notice.show("A local file conflicts with this project.");
    advance(3000);
    assert.deepEqual(state.at(-1), ["A local file conflicts with this project.", false]);
    advance(4700);
    assert.deepEqual(state.at(-1), ["A local file conflicts with this project.", true]);
    advance(5000);
    assert.deepEqual(state.at(-1), ["", false]);
    notice.show("Network unavailable.");
    notice.dispose();
    assert.equal(pending.size, 0);
  });
});
