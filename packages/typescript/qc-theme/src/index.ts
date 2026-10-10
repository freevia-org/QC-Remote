import nativeTheme from "./native-theme.json" with { type: "json" };
import colorTheme from "./colors.json" with { type: "json" };
import visualAssets from "./assets.json" with { type: "json" };
import brand from "./brand.json" with { type: "json" };

export { QC_SCREEN_ICON_VECTORS, QC_SCENE_SHUTTLE_REVERSE, QC_SELECTOR_TRIANGLE_TRANSFORMS, type QcScreenIconLayerRole, type QcScreenIconRendering, type QcSelectorTriangleOrientation } from "./screen-icon-vectors.ts";
export { QC_TYPOGRAPHY } from "./typography.ts";
export { QC_LEGAL, QC_LEGAL_DOCUMENTS, type QcLegalDocument } from "./legal.ts";
/**
 * Shared visual contract for the Windows and Android QC Remote apps.
 *
 * `reference` values are the exact colors the Quad Cortex screen renders.
 * Category colors come from the CorOS 4.1 device taxonomy and are shared with
 * physical LED fallback behavior.
 */
export const QC_COLORS = colorTheme;

export const QC_VISUAL_ASSETS = {
  ...visualAssets,
  chassisVector: {
    ...visualAssets.chassisVector,
    url: new URL("../assets/qc-chassis-neutral.svg", import.meta.url).href
  },
  appIcon: {
    ...visualAssets.appIcon,
    url: new URL("../assets/app-icon.svg", import.meta.url).href
  }
} as const;

export const QC_NATIVE_THEME = nativeTheme;
export const QC_BRAND = brand;
