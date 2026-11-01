"""Reproduce the restricted header catalog from the bundled, qualified font.

Install authoring tools: python -m pip install -r scripts/requirements-font-authoring.txt
Check without writes: python scripts/generate-coros-header-font.py --check
Regenerate: python scripts/generate-coros-header-font.py --write

This is not an OpenType shaping engine. Input bytes and fontTools are pinned;
a new font or tool version requires review and explicit qualification first.
No captured strings, device images or browser-derived coordinates are inputs.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import sys
from pathlib import Path

FONT_PATH = "packages/typescript/qc-theme/assets/fonts/IBMPlexSans-Bold.ttf"
LICENSE_PATH = "packages/typescript/qc-theme/assets/fonts/LICENSE.txt"
OUTPUT_PATH = "packages/typescript/qc-ui/src/coros-header-glyphs.ts"
GENERATOR_PATH = "scripts/generate-coros-header-font.py"
QUALIFIED_FONT_SHA256 = "afb18cee6998d476be0964c0058cfcceb02f066cc6319c1edeff49aa78cd3517"
QUALIFIED_LICENSE_SHA256_LF = "f92a3724ca314d3636aa31af4af3d65bb934f36edce956fc5d359dfcd928a9b3"
FONTTOOLS_VERSION = "4.65.0"
SCHEMA_VERSION = 1
FIRST, LAST = 32, 126


class CatalogError(ValueError):
    """A fixed input or supported-layout contract was not satisfied."""


def authoring_tools():
    try:
        import fontTools
        from fontTools.pens.boundsPen import BoundsPen
        from fontTools.pens.svgPathPen import SVGPathPen
        from fontTools.ttLib import TTFont
    except ImportError as error:
        raise CatalogError("Missing fontTools; install scripts/requirements-font-authoring.txt") from error
    if fontTools.__version__ != FONTTOOLS_VERSION:
        raise CatalogError(f"fontTools {FONTTOOLS_VERSION} required; install scripts/requirements-font-authoring.txt")
    return TTFont, BoundsPen, SVGPathPen


def advance_only(value):
    if value is None:
        return 0
    if any(name != "XAdvance" and amount not in (None, 0)
           for name, amount in vars(value).items()):
        raise CatalogError("Non-advance positioning cannot enter the ASCII catalog")
    amount = getattr(value, "XAdvance", 0)
    if not isinstance(amount, int):
        raise CatalogError("Integral design advances required")
    return amount


def pair_in_subtable(table, left, right):
    if left not in table.Coverage.glyphs:
        return None
    if table.Format == 1:
        pairs = table.PairSet[table.Coverage.glyphs.index(left)].PairValueRecord
        for pair in pairs:
            if pair.SecondGlyph == right:
                if advance_only(getattr(pair, "Value2", None)):
                    raise CatalogError("Second-glyph advance is unsupported")
                return advance_only(getattr(pair, "Value1", None))
        return None
    if table.Format == 2:
        c1 = table.ClassDef1.classDefs.get(left, 0)
        c2 = table.ClassDef2.classDefs.get(right, 0)
        pair = table.Class1Record[c1].Class2Record[c2]
        if advance_only(getattr(pair, "Value2", None)):
            raise CatalogError("Second-glyph advance is unsupported")
        return advance_only(getattr(pair, "Value1", None))
    raise CatalogError("Unsupported GPOS pair format")


def pair_subtables(lookup):
    if lookup.LookupFlag != 0:
        raise CatalogError("Flagged GPOS lookups require shaping support")
    subtables = []
    for subtable in lookup.SubTable:
        kind = lookup.LookupType
        if kind == 9:
            if subtable.Format != 1:
                raise CatalogError("Unsupported GPOS extension format")
            kind, subtable = subtable.ExtensionLookupType, subtable.ExtSubTable
        if kind != 2:
            raise CatalogError("Only pair-positioning kern lookups are supported")
        if subtable.Format not in (1, 2):
            raise CatalogError("Unsupported GPOS pair format")
        subtables.append(subtable)
    return subtables


def ligatures_for_ascii(gsub, ascii_by_glyph):
    sequences = set()
    indices = sorted({index for feature in gsub.FeatureList.FeatureRecord
                      if feature.FeatureTag in ("liga", "clig", "rlig")
                      for index in feature.Feature.LookupListIndex})
    for index in indices:
        lookup = gsub.LookupList.Lookup[index]
        if lookup.LookupFlag != 0 or lookup.LookupType != 4:
            raise CatalogError("Default ligatures require an unflagged simple ligature lookup")
        for subtable in lookup.SubTable:
            for left, substitutions in subtable.ligatures.items():
                for substitution in substitutions:
                    names = [left] + list(substitution.Component)
                    if all(name in ascii_by_glyph for name in names):
                        sequences.add("".join(ascii_by_glyph[name] for name in names))
    return sorted(sequences)


def require_non_ascii_composition(gsub, ascii_glyphs):
    """Prove the pinned face's ccmp rules cannot match an all-ASCII run."""
    indices = sorted({index for feature in gsub.FeatureList.FeatureRecord
                      if feature.FeatureTag == "ccmp" for index in feature.Feature.LookupListIndex})
    for index in indices:
        lookup = gsub.LookupList.Lookup[index]
        if lookup.LookupFlag != 0:
            raise CatalogError("Flagged composition lookups are unsupported")
        for table in lookup.SubTable:
            if lookup.LookupType == 4:
                for left, substitutions in table.ligatures.items():
                    for substitution in substitutions:
                        if all(name in ascii_glyphs for name in [left] + list(substitution.Component)):
                            raise CatalogError("ASCII composition must use whole-run fallback")
            elif lookup.LookupType == 6 and table.Format == 3:
                groups = table.BacktrackCoverage + table.InputCoverage + table.LookAheadCoverage
                if not table.InputCoverage or all(ascii_glyphs.intersection(group.glyphs) for group in groups):
                    raise CatalogError("ASCII contextual composition is unsupported")
            else:
                raise CatalogError("Unsupported composition lookup profile")


def generate_catalog(raw, license_raw):
    TTFont, BoundsPen, SVGPathPen = authoring_tools()
    if hashlib.sha256(raw).hexdigest() != QUALIFIED_FONT_SHA256:
        raise CatalogError("Bundled font differs from the qualified Bold 3.1 bytes; requalify before changing the pin")
    license_text = license_raw.decode("utf-8").replace("\r\n", "\n")
    if hashlib.sha256(license_text.encode("utf-8")).hexdigest() != QUALIFIED_LICENSE_SHA256_LF:
        raise CatalogError("The bundled font copyright/OFL source is missing or changed")
    with TTFont(io.BytesIO(raw)) as font:
        if "fvar" in font or any(tag not in font for tag in ("glyf", "hmtx", "head", "GPOS", "GSUB")):
            raise CatalogError("Qualified static TrueType tables required")
        cmap = font.getBestCmap()
        if not cmap or any(code not in cmap for code in range(FIRST, LAST + 1)):
            raise CatalogError("Complete printable ASCII cmap required")
        glyph_set = font.getGlyphSet()
        glyphs = []
        for codepoint in range(FIRST, LAST + 1):
            name = cmap[codepoint]
            pen, bounds = SVGPathPen(glyph_set), BoundsPen(glyph_set)
            glyph_set[name].draw(pen)
            glyph_set[name].draw(bounds)
            glyphs.append({"advanceUnits": font["hmtx"][name][0], "path": pen.getCommands(), "bounds": bounds.bounds})
        gpos = font["GPOS"].table
        # The four script-specific kern features reference one lookup in the
        # qualified face. Apply each shared lookup once, not once per script.
        indices = sorted({index for feature in gpos.FeatureList.FeatureRecord
                          if feature.FeatureTag == "kern" for index in feature.Feature.LookupListIndex})
        if indices != [0]:
            raise CatalogError("Qualified kern lookup profile changed")
        lookups = [pair_subtables(gpos.LookupList.Lookup[index]) for index in indices]
        kerns = {}
        for left in range(FIRST, LAST + 1):
            for right in range(FIRST, LAST + 1):
                units = 0
                for subtables in lookups:
                    for subtable in subtables:
                        amount = pair_in_subtable(subtable, cmap[left], cmap[right])
                        if amount is not None:
                            units += amount
                            break
                if units:
                    kerns[f"{left},{right}"] = units
        ascii_by_glyph = {cmap[c]: chr(c) for c in range(FIRST, LAST + 1)}
        require_non_ascii_composition(font["GSUB"].table, set(ascii_by_glyph))
        ligatures = ligatures_for_ascii(font["GSUB"].table, ascii_by_glyph)
        if ligatures != ["fi"] or len(kerns) != 1247:
            raise CatalogError("Qualified ASCII shaping profile changed")
        metadata = {
            "schemaVersion": SCHEMA_VERSION,
            "generatorPath": GENERATOR_PATH,
            "generatorSha256Lf": hashlib.sha256(Path(__file__).read_bytes().replace(b"\r\n", b"\n")).hexdigest(),
            "fontToolsVersion": FONTTOOLS_VERSION,
            "sourcePath": FONT_PATH,
            "sourceSha256": QUALIFIED_FONT_SHA256,
            "sourceVersion": font["name"].getDebugName(5),
            "sourcePostScriptName": font["name"].getDebugName(6),
            "sourceCopyright": font["name"].getDebugName(0),
            "license": "OFL-1.1",
            "licensePath": LICENSE_PATH,
            "licenseSha256Lf": hashlib.sha256(license_text.encode("utf-8")).hexdigest(),
            "reservedFontName": "Plex",
            "unitsPerEm": font["head"].unitsPerEm,
            "firstCodepoint": FIRST,
            "lastCodepoint": LAST,
            "fallbackLigatureSequences": ligatures,
            "derivation": "Original font outlines, hmtx advances and GPOS kern pair advances; no device positions or images",
            "scope": "Qualified pinned face only; printable ASCII; default fi ligature uses whole-run text fallback",
        }
    compact = lambda value: json.dumps(value, ensure_ascii=True, separators=(",", ":"))
    entries = "\n".join(f"  {{advanceUnits:{g['advanceUnits']},path:{compact(g['path'])},bounds:{compact(g['bounds'])}}}," for g in glyphs)
    output = (
        "// Generated by scripts/generate-coros-header-font.py --write; do not edit.\n"
        '// Derived IBM font outlines: Copyright © 2017 IBM Corp. with Reserved Font Name "Plex".\n'
        "// SIL OFL-1.1; see packages/typescript/qc-theme/assets/fonts/LICENSE.txt.\n"
        "// Original contours/design metrics, not pixel traces. Font data retains its OFL license.\n"
        f"export const COROS_HEADER_FONT = {compact(metadata)} as const;\n"
        "export interface CorOsHeaderFontGlyph {\n"
        "  readonly advanceUnits: number;\n  readonly path: string;\n"
        "  readonly bounds: readonly [number, number, number, number] | null;\n}\n"
        f"export const COROS_HEADER_GLYPHS: readonly CorOsHeaderFontGlyph[] = [\n{entries}\n];\n"
        f"export const COROS_HEADER_KERNS: Readonly<Record<string, number>> = {compact(kerns)};\n"
    )
    return output.encode("utf-8")


def run(repo, output, write=False):
    try:
        expected = generate_catalog((repo / FONT_PATH).read_bytes(), (repo / LICENSE_PATH).read_bytes())
    except OSError as error:
        raise CatalogError("Source font or license is missing/unreadable in this checkout") from error
    if write:
        # Generation and all input validation finish before the sole product write.
        output.write_bytes(expected)
    else:
        try:
            actual = output.read_bytes()
        except OSError as error:
            raise CatalogError("Catalog is missing/unreadable; run generator --write") from error
        if actual != expected:
            raise CatalogError("Catalog differs from deterministic qualified output; review inputs and run generator --write")
    return {"glyphs": LAST - FIRST + 1, "sourceSha256": QUALIFIED_FONT_SHA256,
            "catalogSha256": hashlib.sha256(expected).hexdigest(), "catalogBytes": len(expected), "mode": "write" if write else "check"}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--write", action="store_true")
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path, help="Explicit output for authoring tests; default is the declared checkout catalog")
    args = parser.parse_args(argv)
    try:
        result = run(args.repo.resolve(), args.output or args.repo / OUTPUT_PATH, args.write)
    except (CatalogError, OSError, UnicodeError, KeyError, IndexError, AttributeError) as error:
        print(f"Header catalog refused: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
