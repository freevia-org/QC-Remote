import { readFile, writeFile } from "node:fs/promises";

const manifest = new URL("../ios/App/CapApp-SPM/Package.swift", import.meta.url);
const source = await readFile(manifest, "utf8");
const normalized = source.replaceAll("\\", "/");
if (normalized !== source) await writeFile(manifest, normalized, "utf8");
