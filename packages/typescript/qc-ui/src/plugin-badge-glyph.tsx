import { QC_COLORS, QC_TYPOGRAPHY } from "@qc-remote/theme";

export interface QcPluginBadgeGlyphProps {
  abbreviation: string;
  color: string;
  x: number;
  y: number;
  tileSize: number;
}

/** Generated plugin overlay. The base block tile contains no badge geometry. */
export function QcPluginBadgeGlyph({ abbreviation, color, x, y, tileSize }: QcPluginBadgeGlyphProps) {
  return <g className="official-plugin-badge" aria-label={`Plugin ${abbreviation}`}>
    <rect x={x - tileSize * .225} y={y - tileSize * .565} width={tileSize * .45} height={tileSize * .205} rx={tileSize * .065} fill={color} />
    <text x={x} y={y - tileSize * .405} textAnchor="middle" fill={QC_COLORS.device.blockLabel} stroke="none" fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="900" fontSize={tileSize * .145}>{abbreviation}</text>
  </g>;
}
