/** Demand-triggered HTTP only: no observation cache, polling timer or idle SSE.
 * Preserve OAuth responses and honor server-requested cooldowns. */
export class RemoteTraffic {
  private blockedUntil = 0;
  private failures = 0;
  private now: () => number;
  private active: () => boolean;
  constructor(now = Date.now, active = () => true) { this.now = now; this.active = active; }

  async fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    const url = new URL(input instanceof Request ? input.url : String(input));
    const method = init?.method ?? (input instanceof Request ? input.method : "GET");
    // This relay has no supported push subscription. Do not open an idle SSE
    // request or let the SDK retry it: serve its documented unsupported status
    // locally. Tool POSTs and demand-triggered OAuth requests stay untouched.
    if (url.pathname === "/mcp" && method.toUpperCase() === "GET") return new Response(null, { status: 405 });
    const now = this.now();
    if (!this.active()) throw new Error("Cloud requests are paused while the app is inactive.");
    if (now < this.blockedUntil) throw new Error("Cloud requests are cooling down; try again shortly.");
    try {
      const response = await globalThis.fetch(input, init);
      if (response.status === 429 || response.status >= 500) {
        const retry = response.headers.get("retry-after");
        const seconds = retry === null ? NaN : Number(retry);
        const until = Number.isFinite(seconds) ? now + Math.max(0, seconds) * 1_000 : Date.parse(retry ?? "");
        this.failures++;
        this.blockedUntil = Math.max(now + Math.min(300_000, 5_000 * 2 ** Math.min(this.failures, 6)), Number.isFinite(until) ? until : 0);
      } else if (response.ok) this.failures = 0;
      return response;
    } catch (error) {
      this.failures++;
      this.blockedUntil = now + Math.min(300_000, 5_000 * 2 ** Math.min(this.failures, 6));
      throw error;
    }
  }
}
