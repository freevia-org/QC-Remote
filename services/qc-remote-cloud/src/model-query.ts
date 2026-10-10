/** Apply `list_models`' `query` argument on the remote path.
 *
 * `device.listModels` takes no arguments: the contract gives `list_models` a
 * `query` input but no gateway argument for it, so the local MCP server in
 * `services/rust-mcp` filters the catalog itself after the gateway answers.
 * The worker projected the action onto the gateway method, dropped `query`
 * because the method has no slot for it, and never filtered, so
 * `{"query": "fuzz"}` and `{"query": "gate"}` each returned the whole
 * ~600-model, ~72 KB catalog - larger than an assistant's tool-result limit,
 * which is the one caller the argument exists for. This mirrors the local
 * server's rule exactly.
 */

type Json = Record<string, unknown>;

/** Whether a lowercase query selects a category.
 *
 * Categories use the catalog's standard abbreviations - "Guitar Amp",
 * "Guitar Cab", "EQ" - while a caller naturally types the whole word, so a
 * query word and a category word match when either is a prefix of the other.
 * This only widens the substring match; it never rejects something that
 * already matched. Same rule as `category_matches` in `services/rust-mcp`. */
export function categoryMatches(category: string, query: string): boolean {
  if (category.includes(query)) return true;
  const words = category.split(/\s+/u).filter(Boolean);
  let matchedAny = false;
  for (const word of query.split(/\s+/u).filter(Boolean)) {
    // One-character fragments would select nearly every category.
    if (word.length < 2) return false;
    if (!words.some((entry) => entry.length >= 2 && (word.startsWith(entry) || entry.startsWith(word)))) return false;
    matchedAny = true;
  }
  return matchedAny;
}

/** Keep the models whose name or `basedOn` contains the query, or whose
 * category it selects, case-insensitively. An absent, null or blank query
 * returns the result unchanged. */
export function filterModelResult<T extends Json>(result: T, rawQuery: unknown): T {
  const query = typeof rawQuery === "string" ? rawQuery.trim().toLowerCase() : "";
  if (!query) return result;
  const models = result.models;
  if (!Array.isArray(models)) throw new Error("device.listModels returned a malformed response");
  const field = (model: Json, name: string) => {
    const value = model[name];
    return typeof value === "string" ? value.toLowerCase() : "";
  };
  return {
    ...result,
    models: models.filter((model): boolean => {
      if (model === null || typeof model !== "object" || Array.isArray(model)) return false;
      const entry = model as Json;
      return ["name", "basedOn"].some((name) => field(entry, name).includes(query))
        || categoryMatches(field(entry, "category"), query);
    })
  };
}

/** Action inputs the worker consumes itself rather than forwarding to the
 * gateway, keyed `action.input`. Every entry is applied by
 * `applyWorkerInputs`; the maintainers' contract tests (not yet published in
 * this repository) hold each one to that instead of treating it as an
 * exception. */
export const WORKER_APPLIED_INPUTS: ReadonlySet<string> = new Set(["list_models.query"]);

/** Apply the worker-consumed inputs of `actionName` to the gateway's result. */
export function applyWorkerInputs(actionName: string, rawParams: Record<string, unknown>, result: Json): Json {
  return actionName === "list_models" ? filterModelResult(result, rawParams.query) : result;
}
