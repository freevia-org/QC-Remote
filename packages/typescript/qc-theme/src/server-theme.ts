/** The theme surface a Cloudflare Worker can actually import.
 *
 * `./index.ts` resolves two asset URLs with `new URL(path, import.meta.url)`,
 * which Wrangler's bundler does not evaluate the way a browser build does -
 * importing it from a Worker pulls in that resolution regardless of whether
 * the Worker ever reads the result. Every server-rendered HTML page in this
 * repository needs only colors and typography, so this file imports exactly
 * those two source modules and nothing that touches an asset path. Any
 * Worker that wants the same dark theme imports from here, not from a local
 * copy of this file - a second copy would drift from this one silently,
 * which is exactly the duplication a single shared source is meant to
 * prevent. */
import colorTheme from "./colors.json" with { type: "json" };
import { QC_TYPOGRAPHY } from "./typography.ts";

export const QC_COLORS = colorTheme;
export { QC_TYPOGRAPHY };
