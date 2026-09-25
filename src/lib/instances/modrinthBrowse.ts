import type { ContentType, ModrinthSearchPage } from "../backend";

export class DefaultBrowseCache {
  private readonly pages = new Map<string, { page: ModrinthSearchPage; at: number }>();
  private readonly ttlMs: number;

  constructor(ttlMs = 45_000) { this.ttlMs = ttlMs; }

  get(instanceId: string, kind: ContentType, minecraftVersion: string, now = Date.now()): ModrinthSearchPage | null {
    const key = `${instanceId}:${kind}:${minecraftVersion}`;
    const cached = this.pages.get(key);
    if (!cached) return null;
    if (now - cached.at >= this.ttlMs) {
      this.pages.delete(key);
      return null;
    }
    return cached.page;
  }

  put(instanceId: string, kind: ContentType, minecraftVersion: string, page: ModrinthSearchPage, now = Date.now()): void {
    this.pages.set(`${instanceId}:${kind}:${minecraftVersion}`, { page, at: now });
  }
}

export function appendBrowsePage(
  current: ModrinthSearchPage | null,
  incoming: ModrinthSearchPage,
  requestedOffset: number,
): ModrinthSearchPage {
  return requestedOffset === 0 || !current
    ? incoming
    : { ...incoming, hits: [...current.hits, ...incoming.hits] };
}

type Timer = ReturnType<typeof setTimeout>;
type Schedule = (callback: () => void, delay: number) => Timer;
type Cancel = (timer: Timer) => void;

export class TransientNotice {
  private fadeTimer: Timer | null = null;
  private removeTimer: Timer | null = null;
  private readonly update: (message: string, exiting: boolean) => void;
  private readonly schedule: Schedule;
  private readonly cancel: Cancel;

  constructor(
    update: (message: string, exiting: boolean) => void,
    schedule: Schedule = (callback, delay) => setTimeout(callback, delay),
    cancel: Cancel = (timer) => clearTimeout(timer),
  ) { this.update = update; this.schedule = schedule; this.cancel = cancel; }

  show(message: string): void {
    this.dispose();
    this.update(message, false);
    this.fadeTimer = this.schedule(() => this.update(message, true), 2700);
    this.removeTimer = this.schedule(() => {
      this.update("", false);
      this.fadeTimer = null;
      this.removeTimer = null;
    }, 3000);
  }

  dispose(): void {
    if (this.fadeTimer !== null) this.cancel(this.fadeTimer);
    if (this.removeTimer !== null) this.cancel(this.removeTimer);
    this.fadeTimer = null;
    this.removeTimer = null;
  }
}
