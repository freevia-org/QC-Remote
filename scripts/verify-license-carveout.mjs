#!/usr/bin/env node
// Keeps LICENSE's carve-out and the package metadata honest about each other.
//
// LICENSE lists the device-derived and third-party-sourced material it does
// not license (Schedule A) and the screen reconstructions it licenses only as
// to our own expression (Schedule B) by whole file - a path, or a glob that
// matches whole files. Those files are covered by
// LICENSES/LicenseRef-QC-Remote-Reference.txt, so:
//
// - every schedule entry must match at least one tracked file (no entry may
//   name something that does not exist);
// - every package.json / Cargo.toml whose package contains a listed file, or
//   embeds one into its build (a Rust include_str!/include_bytes!, or a Vite
//   config copying it), must declare
//   `(MIT OR Apache-2.0) AND LicenseRef-QC-Remote-Reference`;
// - so must every package that links or bundles such a package, followed
//   transitively: a Cargo `path` dependency (or `workspace = true` resolving
//   to one) in a normal, build or target table, or an npm dependency on a
//   workspace member or a `file:` / `link:` package. Dev dependencies do not
//   ship and are not followed;
// - every other package must declare plain `MIT OR Apache-2.0`;
// - a package may carry copies of the license texts (LICENSE, LICENSE-MIT,
//   LICENSE-APACHE, and the reference-material terms) so a registry artifact
//   is self-describing; every copy must equal the repository's text, and a
//   Cargo manifest with an `include` allowlist must ship each text its
//   declared expression requires;
// - the device-knowledge skill carries LICENSE and every document it links
//   to, transitively, as byte copies, bundles each of them, and every relative
//   link in every bundled file names a bundled file at that path in the skill.
//
// Usage:
//   node scripts/verify-license-carveout.mjs        # exit 1 on any error
//
// The functions are exported so other tooling can apply the same rule to the
// same manifests, including through a directory junction or symlink.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, posix, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const BASE_LICENSE_EXPRESSION = "MIT OR Apache-2.0";
export const REFERENCE_LICENSE_REF = "LicenseRef-QC-Remote-Reference";
export const REFERENCE_LICENSE_EXPRESSION = `(${BASE_LICENSE_EXPRESSION}) AND ${REFERENCE_LICENSE_REF}`;
export const REFERENCE_TERMS_PATH = `LICENSES/${REFERENCE_LICENSE_REF}.txt`;

const MANIFEST = /(?:^|\/)(?:package\.json|Cargo\.toml)$/;

/** Schedule A and B table rows of LICENSE, as `{ schedule, pattern }`. */
export function carveOutEntries(licenseText) {
  const entries = [];
  let schedule = null;
  for (const line of licenseText.split(/\r?\n/)) {
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    if (heading) {
      schedule = /^Schedule ([A-Z])\b/.exec(heading[1].trim())?.[1] ?? null;
      continue;
    }
    if (!schedule) continue;
    const row = /^\|\s*`([^`]+)`\s*\|/.exec(line);
    if (row) entries.push({ schedule, pattern: row[1] });
  }
  return entries;
}

/** A glob over whole repository-relative paths: `**` crosses directories, `*` and `?` do not. */
export function globToRegExp(glob) {
  let source = "";
  for (let index = 0; index < glob.length; index += 1) {
    const character = glob[index];
    if (character === "*" && glob[index + 1] === "*") {
      index += 1;
      if (glob[index + 1] === "/") { index += 1; source += "(?:.*/)?"; } else source += ".*";
    } else if (character === "*") source += "[^/]*";
    else if (character === "?") source += "[^/]";
    else source += character.replace(/[.+^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${source}$`);
}

function tomlSection(source, heading) {
  const match = new RegExp(`^\\[${heading.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\]\\s*$`, "m").exec(source);
  if (!match) return null;
  const remainder = source.slice(match.index + match[0].length);
  const next = /^\s*\[/m.exec(remainder);
  return next ? remainder.slice(0, next.index) : remainder;
}

/** The license a manifest declares: package.json `license`, Cargo `[package]` or else `[workspace.package]`. */
export function declaredManifestLicense(path, source) {
  if (path.endsWith("package.json")) {
    try {
      const value = JSON.parse(source).license;
      return typeof value === "string" ? value.trim() : "";
    } catch {
      return "";
    }
  }
  const read = (section) => section === null ? "" : (/^license\s*=\s*"([^"]+)"\s*$/m.exec(section)?.[1] ?? "").trim();
  return read(tomlSection(source, "package")) || read(tomlSection(source, "workspace.package"));
}

function balancedArguments(source, openIndex) {
  let depth = 0;
  for (let index = openIndex; index < source.length; index += 1) {
    if (source[index] === "(") depth += 1;
    else if (source[index] === ")") {
      depth -= 1;
      if (depth === 0) return source.slice(openIndex + 1, index);
    }
  }
  return source.slice(openIndex + 1);
}

/**
 * The listed files a package's own sources pull into its build from outside
 * the package: Rust `include_str!` / `include_bytes!` arguments naming a listed
 * file or its directory, and string literals in a Vite config naming a listed
 * file.
 */
export function embeddedReferenceFiles(packageFiles, listedFiles, readFile) {
  const embedded = new Set();
  for (const file of packageFiles) {
    if (file.endsWith(".rs")) {
      const source = readFile(file);
      for (const match of source.matchAll(/include_(?:str|bytes)!\s*\(/g)) {
        const argumentsText = balancedArguments(source, match.index + match[0].length - 1);
        for (const listed of listedFiles) {
          const directory = posix.dirname(listed);
          if (argumentsText.includes(listed) || (directory.includes("/") && argumentsText.includes(`${directory}/`))) embedded.add(listed);
        }
      }
    } else if (/(?:^|\/)vite\.config\.[cm]?[jt]s$/.test(file)) {
      const source = readFile(file);
      for (const match of source.matchAll(/(["'`])((?:(?!\1).)*)\1/g)) {
        for (const listed of listedFiles) if (match[2].includes(listed)) embedded.add(listed);
      }
    }
  }
  return [...embedded].sort();
}

function tomlSections(source) {
  const sections = [];
  let current = { name: "", lines: [] };
  for (const line of source.split(/\r?\n/)) {
    const header = /^\s*\[([^[\]]+)\]\s*(?:#.*)?$/.exec(line);
    if (header) {
      sections.push(current);
      current = { name: header[1].trim(), lines: [] };
    } else current.lines.push(line);
  }
  sections.push(current);
  return sections;
}

const SHIPPED_CARGO_TABLE = /^(?:target\.(?:'[^']*'|"[^"]*"|[^.]+)\.)?(?:dependencies|build-dependencies)(?:\.(.+))?$/;

/** `name = value` entries of a TOML section body, with inline tables joined onto one line. */
function tomlEntries(lines) {
  const entries = [];
  const open = (text) => (text.match(/{/g) ?? []).length > (text.match(/}/g) ?? []).length;
  let pending = null;
  for (const raw of lines) {
    const line = raw.replace(/\s+#.*$/, "");
    if (pending !== null) {
      pending.value += ` ${line.trim()}`;
      if (!open(pending.value)) { entries.push(pending); pending = null; }
      continue;
    }
    const entry = /^\s*([A-Za-z0-9_.-]+|"[^"]+")\s*=\s*(.*)$/.exec(line);
    if (!entry) continue;
    const current = { key: entry[1].replace(/^"|"$/g, ""), value: entry[2].trim() };
    if (open(current.value)) pending = current;
    else entries.push(current);
  }
  if (pending) entries.push(pending);
  return entries;
}

const tomlString = (text, key) => new RegExp(`(?:^|[\\s{,])${key}\\s*=\\s*"([^"]+)"`).exec(text)?.[1];
const tomlTrue = (text, key) => new RegExp(`(?:^|[\\s{,])${key}\\s*=\\s*true\\b`).test(text);
const unquote = (text) => text.replace(/^"|"$/g, "");

/** Path dependencies a Cargo workspace root declares under [workspace.dependencies], by name. */
function cargoWorkspacePaths(manifest, source) {
  const paths = new Map();
  const directory = posix.dirname(manifest);
  for (const section of tomlSections(source)) {
    const table = /^workspace\.dependencies(?:\.(.+))?$/.exec(section.name);
    if (!table) continue;
    if (table[1]) {
      const path = tomlEntries(section.lines).find(({ key }) => key === "path")?.value;
      if (path) paths.set(unquote(table[1]), posix.normalize(posix.join(directory, unquote(path))));
      continue;
    }
    for (const { key, value } of tomlEntries(section.lines)) {
      const path = tomlString(value, "path");
      if (path) paths.set(key, posix.normalize(posix.join(directory, path)));
    }
  }
  return paths;
}

/**
 * The first-party package directories a manifest links or bundles: Cargo path
 * dependencies (direct, or `workspace = true` resolving to one) in normal,
 * build and target tables, and npm `dependencies` / `optionalDependencies`
 * naming a workspace member or a `file:` / `link:` package. `[dev-dependencies]`
 * and `devDependencies` are not shipped and are not followed.
 */
export function firstPartyDependencyDirectories(manifest, source, { npmPackageDirectories = new Map(), cargoWorkspaceDependencyPaths = new Map() } = {}) {
  const directory = posix.dirname(manifest);
  const found = new Set();
  if (manifest.endsWith("package.json")) {
    let parsed;
    try { parsed = JSON.parse(source); } catch { return []; }
    for (const field of ["dependencies", "optionalDependencies"]) {
      for (const [name, spec] of Object.entries(parsed?.[field] ?? {})) {
        const local = /^(?:file|link):(.+)$/.exec(String(spec));
        if (local) found.add(posix.normalize(posix.join(directory, local[1].replaceAll("\\", "/"))));
        else if (npmPackageDirectories.has(name)) found.add(npmPackageDirectories.get(name));
      }
    }
    return [...found].sort();
  }
  for (const section of tomlSections(source)) {
    const table = SHIPPED_CARGO_TABLE.exec(section.name);
    if (!table) continue;
    const entries = tomlEntries(section.lines);
    const consider = (name, text) => {
      const path = tomlString(text, "path");
      if (path) found.add(posix.normalize(posix.join(directory, path)));
      else if (tomlTrue(text, "workspace") && cargoWorkspaceDependencyPaths.has(name)) found.add(cargoWorkspaceDependencyPaths.get(name));
    };
    if (table[1]) consider(unquote(table[1]), entries.map(({ key, value }) => `${key} = ${value}`).join(", "));
    else for (const { key, value } of entries) {
      const dotted = /^(.+)\.workspace$/.exec(key);
      if (dotted) consider(dotted[1], value === "true" ? "workspace = true" : "");
      else consider(key, value);
    }
  }
  return [...found].sort();
}

/**
 * Every first-party manifest's expected license expression, with the reason.
 * `trackedFiles` are repository-relative POSIX paths of the public repository.
 */
export function expectedManifestLicenses({ licenseText, trackedFiles, readFile }) {
  const entries = carveOutEntries(licenseText);
  const matchers = entries.map((entry) => ({ ...entry, regex: globToRegExp(entry.pattern) }));
  const listedFiles = trackedFiles.filter((file) => matchers.some(({ regex }) => regex.test(file)));
  const manifests = trackedFiles.filter((file) => MANIFEST.test(file));
  const packageDirectories = [...new Set(manifests.map((file) => posix.dirname(file)))];
  const inside = (file, directory) => directory === "." || file.startsWith(`${directory}/`);
  const npmPackageDirectories = new Map();
  const cargoWorkspaceDependencyPaths = new Map();
  for (const manifest of manifests) {
    if (manifest.endsWith("package.json")) {
      try {
        const name = JSON.parse(readFile(manifest)).name;
        if (typeof name === "string" && posix.dirname(manifest) !== ".") npmPackageDirectories.set(name, posix.dirname(manifest));
      } catch { /* an unreadable manifest names no package */ }
    } else for (const [name, path] of cargoWorkspacePaths(manifest, readFile(manifest))) cargoWorkspaceDependencyPaths.set(name, path);
  }
  const own = new Map();
  for (const manifest of manifests) {
    const directory = posix.dirname(manifest);
    const contained = listedFiles.filter((file) => inside(file, directory));
    const nested = packageDirectories.filter((other) => other !== directory && inside(other, directory));
    const ownFiles = trackedFiles.filter((file) => inside(file, directory) && !nested.some((other) => inside(file, other)));
    const embedded = embeddedReferenceFiles(ownFiles, listedFiles.filter((file) => !inside(file, directory)), readFile);
    const kind = posix.basename(manifest);
    const dependencies = firstPartyDependencyDirectories(manifest, readFile(manifest), { npmPackageDirectories, cargoWorkspaceDependencyPaths })
      .map((dependency) => (dependency === "." ? kind : `${dependency}/${kind}`))
      .filter((dependency) => dependency !== manifest && manifests.includes(dependency));
    own.set(manifest, { contained, embedded, dependencies });
  }
  // A package that links or bundles a reference-material package ships that
  // material too, so the requirement follows dependencies transitively.
  const reaches = new Map([...own].map(([manifest, { contained, embedded }]) => [manifest, contained.length > 0 || embedded.length > 0]));
  for (let changed = true; changed;) {
    changed = false;
    for (const [manifest, { dependencies }] of own) {
      if (!reaches.get(manifest) && dependencies.some((dependency) => reaches.get(dependency))) {
        reaches.set(manifest, true);
        changed = true;
      }
    }
  }
  const expected = new Map();
  for (const [manifest, { contained, embedded, dependencies }] of own) {
    expected.set(manifest, {
      expression: reaches.get(manifest) ? REFERENCE_LICENSE_EXPRESSION : BASE_LICENSE_EXPRESSION,
      contained,
      embedded,
      linked: dependencies.filter((dependency) => reaches.get(dependency))
    });
  }
  return { entries, listedFiles, expected };
}

export function licenseCarveOutErrors({ licenseText, termsText, trackedFiles, readFile }) {
  const errors = [];
  const { entries, listedFiles, expected } = expectedManifestLicenses({ licenseText, trackedFiles, readFile });
  for (const schedule of ["A", "B"]) {
    if (!entries.some((entry) => entry.schedule === schedule)) errors.push(`LICENSE has no Schedule ${schedule} entries`);
  }
  for (const { schedule, pattern } of entries) {
    const regex = globToRegExp(pattern);
    if (!trackedFiles.some((file) => regex.test(file))) errors.push(`LICENSE Schedule ${schedule} lists ${pattern}, which matches no tracked file`);
  }
  const duplicates = entries.map(({ pattern }) => pattern).filter((pattern, index, all) => all.indexOf(pattern) !== index);
  for (const pattern of new Set(duplicates)) errors.push(`LICENSE lists ${pattern} more than once`);
  if (!licenseText.includes(REFERENCE_TERMS_PATH)) errors.push(`LICENSE does not point at ${REFERENCE_TERMS_PATH}`);
  if (!licenseText.includes(REFERENCE_LICENSE_EXPRESSION)) errors.push(`LICENSE does not state the package expression ${REFERENCE_LICENSE_EXPRESSION}`);
  if (termsText === null) errors.push(`${REFERENCE_TERMS_PATH} is missing`);
  else if (!termsText.includes(`SPDX identifier: ${REFERENCE_LICENSE_REF}`)) errors.push(`${REFERENCE_TERMS_PATH} does not define ${REFERENCE_LICENSE_REF}`);
  for (const [manifest, { expression, contained, embedded, linked }] of expected) {
    const actual = declaredManifestLicense(manifest, readFile(manifest));
    if (actual === expression) continue;
    const reason = contained.length ? `it contains ${contained[0]}${contained.length > 1 ? ` and ${contained.length - 1} other listed file(s)` : ""}`
      : embedded.length ? `it embeds ${embedded.join(", ")}`
        : linked.length ? `it links or bundles ${linked.join(", ")}, ${linked.length > 1 ? "which ship" : "which ships"} material listed in LICENSE Schedule A or B`
          : "it contains and embeds no file listed in LICENSE Schedule A or B, and links or bundles no package that does";
    errors.push(`${manifest} declares ${actual || "no license"}, expected ${expression}: ${reason}`);
  }
  return errors;
}

/** Names of the license texts a package may carry a copy of, with the repository file each copies. */
const LICENSE_TEXT_COPIES = new Map([
  ["LICENSE", "LICENSE"],
  ["LICENSE-MIT", "LICENSE-MIT"],
  ["LICENSE-APACHE", "LICENSE-APACHE"],
  [`${REFERENCE_LICENSE_REF}.txt`, REFERENCE_TERMS_PATH]
]);

/**
 * Third-party notice texts kept as byte copies under two names, as
 * [copy, canonical]: the pyquadcortex MIT notice ships inside the quadcortex
 * crate and, as the community protocol license, with every app build.
 */
export const NOTICE_TEXT_COPIES = [
  ["packages/rust/quadcortex/licenses/pyquadcortex-MIT.txt", "legal/COMMUNITY-PROTOCOL-LICENSE.txt"]
];

/** The device-knowledge skill folder, and the Rust module that lists its bundled files. */
export const DEVICE_KNOWLEDGE_SKILL_DIRECTORY = "skills/qc-device-knowledge";
export const DEVICE_KNOWLEDGE_SKILL_BUNDLE = "apps/windows/src-tauri/src/client_packages/device_knowledge_skill.rs";

/**
 * Repository documents the device-knowledge skill carries as byte copies, by
 * their path inside the skill, which is also the path of the canonical file in
 * the repository. SKILL.md and the lineage catalogue are Schedule A files, so
 * every copy of the skill carries LICENSE and, transitively, every document a
 * bundled document links to, at the relative path the link names. (Unlike a
 * package, the skill has no `licenses/` directory for `LICENSES/` to collide
 * with.)
 */
export const SKILL_DOCUMENT_COPIES = [
  "LICENSE",
  REFERENCE_TERMS_PATH,
  "LICENSE-MIT",
  "LICENSE-APACHE",
  "CONTRIBUTING.md",
  "TRADEMARKS.md",
  "THIRD_PARTY-NOTICES.md",
  "SECURITY.md",
  "legal/THIRD_PARTY-LICENSE-TEXTS.txt",
  "legal/THIRD_PARTY-LICENSE-INVENTORY.json",
  "legal/THIRD_PARTY-SOURCE-OFFER.md"
];

/** The skill files listed in the Rust bundle as `device_knowledge_skill_file!("…")`. */
export function bundledSkillFiles(source) {
  return [...source.matchAll(/device_knowledge_skill_file!\(\s*"([^"]+)"\s*\)/g)].map((entry) => entry[1]);
}

/**
 * The targets of the relative `[text](target)` links in a Markdown document at
 * `document` (a path relative to the skill root), resolved against that
 * document's directory, or null for a target that climbs out of the skill.
 * URLs, `mailto:` and in-page anchors are skipped; a `#fragment` is dropped.
 */
export function relativeMarkdownLinkTargets(document, text) {
  const base = posix.dirname(document);
  return [...text.matchAll(/\]\(\s*([^)\s]+)[^)]*\)/g)]
    .map((entry) => entry[1].split("#")[0])
    .filter((target) => target && !target.includes("://") && !target.startsWith("mailto:"))
    .map((target) => {
      const resolved = posix.normalize(posix.join(base, target));
      return resolved === ".." || resolved.startsWith("../") ? null : resolved;
    });
}

/**
 * The skill's copies of the repository documents must be byte-identical to
 * the originals and bundled, and every relative link in every bundled file
 * must name a file the bundle carries at that path. Checking every bundled
 * file closes the set under linking: whatever LICENSE reaches, through any
 * number of links, is bundled, so the links resolve in the client package and
 * in every installed copy of the skill.
 */
export function skillDocumentErrors({ trackedFiles, readFile }) {
  const errors = [];
  const tracked = new Set(trackedFiles);
  const inSkill = (relative) => `${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/${relative}`;
  if (!tracked.has(DEVICE_KNOWLEDGE_SKILL_BUNDLE)) return [`${DEVICE_KNOWLEDGE_SKILL_BUNDLE} is missing; it lists the files the skill bundles`];
  const bundled = new Set(bundledSkillFiles(readFile(DEVICE_KNOWLEDGE_SKILL_BUNDLE)));
  for (const relative of SKILL_DOCUMENT_COPIES) {
    const copy = inSkill(relative);
    const missing = [copy, relative].filter((file) => !tracked.has(file));
    if (missing.length) errors.push(`${missing.join(" and ")} ${missing.length > 1 ? "are" : "is"} missing; ${copy} must be a copy of ${relative}`);
    else if (readFile(copy) !== readFile(relative)) errors.push(`${copy} differs from ${relative}; copy ${relative} over it byte for byte`);
    if (!bundled.has(relative)) errors.push(`${DEVICE_KNOWLEDGE_SKILL_BUNDLE} does not bundle ${relative}, which the skill must carry`);
  }
  for (const document of bundled) {
    if (!tracked.has(inSkill(document))) {
      errors.push(`${DEVICE_KNOWLEDGE_SKILL_BUNDLE} bundles ${document}, but ${inSkill(document)} does not exist`);
      continue;
    }
    for (const target of relativeMarkdownLinkTargets(document, readFile(inSkill(document)))) {
      if (target === null) errors.push(`${inSkill(document)} has a relative link that leaves the skill`);
      else if (!bundled.has(target) || !tracked.has(inSkill(target))) errors.push(`${inSkill(document)} links ${target}, which the skill does not carry and bundle at that path`);
    }
  }
  return errors;
}

/** The strings of a Cargo `[package]` `include` allowlist, or null when the manifest has none. */
export function cargoIncludeList(source) {
  const section = tomlSection(source, "package");
  const match = section === null ? null : /^include\s*=\s*\[([\s\S]*?)\]/m.exec(section);
  return match ? [...match[1].matchAll(/"([^"]*)"/g)].map((entry) => entry[1]) : null;
}

/**
 * Copies of the license texts inside a package must equal the repository's
 * (line endings aside), and a Cargo package that lists its `include`d files
 * must list and carry each text its declared license expression requires:
 * LICENSE, LICENSE-MIT and LICENSE-APACHE at the package root, plus the
 * reference-material terms under `licenses/` when it declares the LicenseRef.
 * (A `LICENSES/` directory is not used inside a package: it would collide
 * with `licenses/` on case-insensitive file systems.)
 */
export function licenseCopyErrors({ trackedFiles, readFile }) {
  const errors = [];
  const normalize = (file) => readFile(file).replace(/\r\n/g, "\n");
  const tracked = new Set(trackedFiles);
  for (const file of trackedFiles) {
    const canonical = LICENSE_TEXT_COPIES.get(posix.basename(file));
    if (canonical === undefined || file === canonical || !tracked.has(canonical)) continue;
    if (normalize(file) !== normalize(canonical)) errors.push(`${file} differs from ${canonical}; a copy of a license text must equal the repository's`);
  }
  for (const [copy, canonical] of NOTICE_TEXT_COPIES) {
    const missing = [copy, canonical].filter((file) => !tracked.has(file));
    if (missing.length) errors.push(`${missing.join(" and ")} ${missing.length > 1 ? "are" : "is"} missing; ${copy} must be a copy of ${canonical}`);
    else if (normalize(copy) !== normalize(canonical)) errors.push(`${copy} differs from ${canonical}; the two copies of that notice must be identical`);
  }
  for (const manifest of trackedFiles.filter((file) => posix.basename(file) === "Cargo.toml")) {
    const source = readFile(manifest);
    const include = cargoIncludeList(source);
    if (include === null) continue;
    const directory = posix.dirname(manifest);
    const inPackage = (relative) => (directory === "." ? relative : `${directory}/${relative}`);
    const license = declaredManifestLicense(manifest, source);
    const required = ["LICENSE", "LICENSE-MIT", "LICENSE-APACHE"];
    if (license.includes(REFERENCE_LICENSE_REF)) required.push(`licenses/${REFERENCE_LICENSE_REF}.txt`);
    for (const relative of required) {
      if (!tracked.has(inPackage(relative))) errors.push(`${manifest} restricts its package with \`include\` and declares ${license}, but ${inPackage(relative)} does not exist`);
      if (!include.some((pattern) => globToRegExp(pattern).test(relative))) errors.push(`${manifest} \`include\` does not ship ${relative}, which its license expression requires`);
    }
  }
  return errors;
}

// Tracked plus not-yet-added (but not ignored) files, so a new data file is
// checked before its first commit; a clean CI checkout sees exactly the index.
export function trackedRepositoryFiles(root) {
  return execFileSync("git", ["-C", root, "ls-files", "-z", "--cached", "--others", "--exclude-standard"], { encoding: "utf8", maxBuffer: 1 << 28 })
    .split("\0").filter(Boolean).map((file) => file.replaceAll("\\", "/"))
    .filter((file) => existsSync(resolve(root, file)));
}

export function currentLicenseCarveOutErrors(root) {
  const read = (file) => readFileSync(resolve(root, file), "utf8");
  const termsPath = resolve(root, REFERENCE_TERMS_PATH);
  const trackedFiles = trackedRepositoryFiles(root);
  return [
    ...licenseCarveOutErrors({
      licenseText: read("LICENSE"),
      termsText: existsSync(termsPath) ? readFileSync(termsPath, "utf8") : null,
      trackedFiles,
      readFile: read
    }),
    ...licenseCopyErrors({ trackedFiles, readFile: read }),
    ...skillDocumentErrors({ trackedFiles, readFile: read })
  ];
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const errors = currentLicenseCarveOutErrors(root);
  if (errors.length) {
    console.error(`LICENSE carve-out and package metadata disagree:\n- ${errors.join("\n- ")}`);
    process.exitCode = 1;
  } else {
    console.log("LICENSE schedules name only existing files, and every package declares the license its contents require; every license text copy matches, and every link in the skill resolves inside it.");
  }
}
