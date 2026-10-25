// Static regression tests for the DCO enforcement workflow.
//
// .github/workflows/dco.yml is the one workflow that runs on
// `pull_request_target`: GitHub reads it from the BASE branch, so a pull request
// cannot edit or skip it. That only holds while the workflow never runs pull
// request code. These tests pin the properties that keep it safe, and prove each
// one by running the same checker over deliberately broken copies of the real
// file (mutated in memory) and requiring it to flag them.
//
// There is no YAML parser among the root dependencies, so the checker is
// text-based and deliberately conservative: it flags anything it does not
// recognise rather than trying to understand it. Comments are ignored outside
// `run:` blocks; inside a `run:` block even a comment is examined, because
// GitHub expands `${{ }}` before the shell ever sees a comment.

import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const workflowsDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '.github', 'workflows');

const DCO_FILE = 'dco.yml';
const JOB_NAME = 'DCO sign-off';
const PINNED_CHECKOUT = 'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1';
const ALLOWED_PERMISSIONS = { contents: 'read', 'pull-requests': 'read' };
const BOT_GUARD = "github.event.pull_request.user.type != 'Bot'";
const TRUSTED_ASSOCIATIONS = ['OWNER', 'MEMBER', 'COLLABORATOR'];
const ASSOCIATION_FIELD = 'github.event.pull_request.author_association';
const ASSOCIATION_GUARD = `!contains(fromJSON('["OWNER","MEMBER","COLLABORATOR"]'), ${ASSOCIATION_FIELD})`;
const JOB_CONDITION = `${BOT_GUARD} && ${ASSOCIATION_GUARD}`;
const CHECKER_COMMAND = 'node scripts/verify-dco.mjs --base "$QC_BASE_SHA" --head "$QC_HEAD_SHA"';
const ALLOWED_GIT_SUBCOMMANDS = new Set(['fetch', 'cat-file']);

// ---- text-based workflow reading ---------------------------------------------

const indentOf = (line) => /^ */.exec(line)[0].length;
const isBlank = (line) => line.trim() === '';

function readLines(text) {
  return text.replace(/\r\n/g, '\n').split('\n');
}

// { start, end, text } for every `run:` value; `end` is exclusive.
function runBlocks(lines) {
  const blocks = [];
  for (let index = 0; index < lines.length; index += 1) {
    const match = /^(\s*)(-\s+)?run:[ \t]*(.*)$/.exec(lines[index]);
    if (!match) continue;
    const keyColumn = match[1].length + (match[2] ? match[2].length : 0);
    const rest = match[3];
    if (/^[|>][+-]?\d*[ \t]*$/.test(rest)) {
      let end = index + 1;
      while (end < lines.length && (isBlank(lines[end]) || indentOf(lines[end]) > keyColumn)) end += 1;
      blocks.push({ start: index, end, text: lines.slice(index + 1, end).join('\n') });
      index = end - 1;
    } else {
      blocks.push({ start: index, end: index + 1, text: rest });
    }
  }
  return blocks;
}

// Lines outside run blocks with comments removed; run-block lines are kept raw.
function analysisLines(lines, blocks) {
  const inRun = new Set();
  for (const block of blocks) for (let i = block.start; i < block.end; i += 1) inRun.add(i);
  return lines.map((line, index) => {
    if (inRun.has(index)) return line;
    if (/^\s*#/.test(line)) return '';
    return line.replace(/\s#.*$/, '');
  });
}

// The body (lines after the key) of a block-style `key:` found at `index`.
function childLines(lines, index) {
  const keyIndent = indentOf(lines[index]);
  const children = [];
  for (let i = index + 1; i < lines.length; i += 1) {
    if (isBlank(lines[i])) continue;
    if (indentOf(lines[i]) <= keyIndent) break;
    children.push(lines[i]);
  }
  return children;
}

function permissionFindings(lines) {
  const findings = [];
  let topLevel = null;
  lines.forEach((line, index) => {
    const match = /^(\s*)permissions:[ \t]*(.*)$/.exec(line);
    if (!match) return;
    const inline = match[2].trim();
    if (inline !== '') {
      findings.push({ rule: 'permissions', detail: `inline permissions value: ${inline}` });
      return;
    }
    const granted = {};
    for (const child of childLines(lines, index)) {
      const entry = /^\s*([\w-]+):[ \t]*(\S+)\s*$/.exec(child);
      if (!entry) {
        findings.push({ rule: 'permissions', detail: `unrecognised permissions line: ${child.trim()}` });
        continue;
      }
      granted[entry[1]] = entry[2];
      if (ALLOWED_PERMISSIONS[entry[1]] !== entry[2]) {
        findings.push({ rule: 'permissions', detail: `permission not allowed: ${entry[1]}: ${entry[2]}` });
      }
    }
    if (match[1].length === 0) topLevel = granted;
  });
  if (topLevel === null || topLevel.contents !== 'read') {
    findings.push({ rule: 'permissions', detail: 'top-level `permissions: contents: read` is missing' });
  }
  return findings;
}

function triggerEvents(lines) {
  const events = new Set();
  const onIndex = lines.findIndex((line) => /^on:/.test(line));
  if (onIndex === -1) return events;
  const body = [lines[onIndex].replace(/^on:/, ''), ...childLines(lines, onIndex)].join('\n');
  for (const match of body.matchAll(/\b(pull_request_target|pull_request)\b/g)) events.add(match[1]);
  return events;
}

// Every step of every `steps:` list, as an array of lines.
function stepsOf(lines) {
  const steps = [];
  lines.forEach((line, index) => {
    if (!/^\s*steps:\s*$/.test(line)) return;
    const body = [];
    for (let i = index + 1; i < lines.length; i += 1) {
      if (isBlank(lines[i])) {
        body.push(lines[i]);
        continue;
      }
      if (indentOf(lines[i]) <= indentOf(line)) break;
      body.push(lines[i]);
    }
    const itemIndent = body.filter((entry) => /^\s*-\s/.test(entry)).map(indentOf)[0];
    let current = null;
    for (const entry of body) {
      if (indentOf(entry) === itemIndent && /^\s*-\s/.test(entry)) {
        current = [];
        steps.push(current);
      }
      if (current) current.push(entry);
    }
  });
  return steps;
}

function commandFindings(runText) {
  const findings = [];
  const script = runText
    .split('\n')
    .filter((line) => !/^\s*#/.test(line))
    .join('\n');
  if (/\$\{\{/.test(runText)) {
    findings.push({ rule: 'run-interpolation', detail: 'a `${{ }}` expression appears inside a run block' });
  }
  if (/GITHUB_(ENV|PATH|OUTPUT|STATE)/.test(script)) {
    findings.push({ rule: 'command', detail: 'run block writes to a GITHUB_* command file' });
  }
  const forbidden =
    /(^|[\s;&|(])(npm|npx|yarn|pnpm|bun|deno|cargo|rustc|gradlew?|python3?|pip3?|make|pwsh|powershell|bash|sh|source|eval|exec|curl|wget)(?=\s|$|;)/m.exec(
      script,
    );
  if (forbidden) findings.push({ rule: 'command', detail: `forbidden command: ${forbidden[2]}` });
  if (/(^|[\s;&|(])\.{1,2}\//m.test(script) || /^\s*\.\s/m.test(script)) {
    findings.push({ rule: 'command', detail: 'run block executes a relative path' });
  }
  for (const match of script.matchAll(/(?:^|[\s;&|(`])git\s+(\S+)/gm)) {
    if (!ALLOWED_GIT_SUBCOMMANDS.has(match[1])) {
      findings.push({ rule: 'command', detail: `git ${match[1]} is not an allowed subcommand` });
    }
  }
  return { findings, nodeLines: [...script.matchAll(/\bnode\b[^\n]*/g)].map((match) => match[0].trim()) };
}

// Findings for the job-level `if:` (every `if:` line of the file is passed in).
// Required shape, exactly one condition:
//   <Bot guard> && !contains(fromJSON('["OWNER","MEMBER","COLLABORATOR"]'), <the PR's author_association>)
export function conditionFindings(conditions) {
  const findings = [];
  const flag = (rule, detail) => findings.push({ rule, detail });
  if (conditions.length !== 1) {
    flag('job-condition', `exactly one \`if:\` is allowed (the job's), found ${JSON.stringify(conditions)}`);
    if (!conditions.some((condition) => condition.includes(BOT_GUARD))) flag('bot-guard', 'the Bot guard is missing');
    if (!conditions.some((condition) => condition.includes('author_association'))) {
      flag('association-guard', 'the association guard is missing');
    }
    return findings;
  }
  const [condition] = conditions;
  if (!condition.includes(BOT_GUARD)) flag('bot-guard', `the Bot guard \`${BOT_GUARD}\` is missing`);
  for (const [, field] of condition.matchAll(/([\w.]*author_association)/g)) {
    if (field !== ASSOCIATION_FIELD) flag('association-guard', `author_association must be read from ${ASSOCIATION_FIELD}, found ${field}`);
  }
  if (!condition.includes('author_association')) flag('association-guard', 'the association guard is missing');
  const lists = [...condition.matchAll(/fromJSON\('([^']*)'\)/g)].map((match) => match[1]);
  if (lists.length !== 1) {
    flag('association-guard', `expected exactly one trusted-association list, found ${lists.length}`);
  } else {
    let values = null;
    try {
      values = JSON.parse(lists[0]);
    } catch {
      flag('association-guard', `trusted list is not JSON: ${lists[0]}`);
    }
    if (values !== null && JSON.stringify([...values].sort()) !== JSON.stringify([...TRUSTED_ASSOCIATIONS].sort())) {
      flag('association-guard', `trusted associations must be exactly ${TRUSTED_ASSOCIATIONS.join(', ')}, found ${JSON.stringify(values)}`);
    }
  }
  if (/\bgithub\.(actor|triggering_actor)\b|\.login\b|\.comment\b|\bname\b/.test(condition)) {
    flag('association-guard', 'the exemption must not use an actor, login, comment or name');
  }
  if (condition !== JOB_CONDITION) {
    flag('job-condition', `job condition must be exactly \`${JOB_CONDITION}\`, found ${JSON.stringify(condition)}`);
  }
  return findings;
}

// Findings for the `pull_request_target` DCO workflow itself.
export function dcoWorkflowFindings(text) {
  const lines = readLines(text);
  const blocks = runBlocks(lines);
  const code = analysisLines(lines, blocks);
  const findings = [];

  const events = triggerEvents(code);
  if (!events.has('pull_request_target')) {
    findings.push({ rule: 'trigger', detail: 'does not trigger on pull_request_target' });
  }
  if (events.has('pull_request')) {
    findings.push({ rule: 'trigger', detail: 'triggers on pull_request, whose YAML a pull request can edit' });
  }

  findings.push(...permissionFindings(code));

  const names = code.filter((line) => new RegExp(`^\\s*(-\\s+)?name:\\s*['"]?${JOB_NAME}['"]?\\s*$`).test(line));
  if (names.length !== 1) {
    findings.push({ rule: 'job-name', detail: `expected exactly one job named \`${JOB_NAME}\`, found ${names.length}` });
  }

  const conditions = code.filter((line) => /^\s*(?:-\s+)?if:/.test(line)).map((line) => line.replace(/^\s*(?:-\s+)?if:\s*/, '').trim());
  findings.push(...conditionFindings(conditions));
  const associationReads = code.join('\n').match(/author_association/g) ?? [];
  if (associationReads.length !== 1) {
    findings.push({ rule: 'association-guard', detail: `author_association must be read exactly once, found ${associationReads.length}` });
  }

  for (const line of code) {
    const uses = /^\s*(?:-\s+)?uses:\s*(\S+)/.exec(line);
    if (uses && uses[1] !== PINNED_CHECKOUT) {
      findings.push({ rule: 'uses', detail: `only ${PINNED_CHECKOUT} is allowed, found ${uses[1]}` });
    }
  }
  const steps = stepsOf(code);
  if (!steps.some((step) => step.some((line) => line.includes(PINNED_CHECKOUT)))) {
    findings.push({ rule: 'uses', detail: 'the pinned checkout step is missing' });
  }
  for (const step of steps.filter((entry) => entry.some((line) => line.includes('actions/checkout')))) {
    for (const line of step) {
      const key = /^\s*(?:-\s+)?(ref|repository|token|path|submodules)\s*:/.exec(line);
      if (key) findings.push({ rule: 'checkout-option', detail: `checkout must use its defaults, found \`${key[1]}:\`` });
    }
  }

  if (code.some((line) => /\bsecrets\b/.test(line))) {
    findings.push({ rule: 'secrets', detail: 'secrets must not be referenced' });
  }
  if (code.some((line) => /\b(actions\/cache|upload-artifact|download-artifact)\b/.test(line))) {
    findings.push({ rule: 'uses', detail: 'caches and artifacts are not allowed' });
  }

  const nodeLines = [];
  for (const block of blocks) {
    const result = commandFindings(block.text);
    findings.push(...result.findings);
    nodeLines.push(...result.nodeLines);
  }
  if (nodeLines.length !== 1 || nodeLines[0] !== CHECKER_COMMAND) {
    findings.push({
      rule: 'checker-command',
      detail: `the only executed script must be \`${CHECKER_COMMAND}\`, found ${JSON.stringify(nodeLines)}`,
    });
  }
  return findings;
}

// Findings across a { fileName: text } map of every workflow in the repository.
export function repositoryWorkflowFindings(files) {
  const findings = [];
  if (!(DCO_FILE in files)) {
    findings.push({ rule: 'dco-workflow', detail: `${DCO_FILE} is missing` });
  } else {
    findings.push(...dcoWorkflowFindings(files[DCO_FILE]));
  }
  let jobNameCount = 0;
  for (const [name, text] of Object.entries(files)) {
    const lines = readLines(text);
    const code = analysisLines(lines, runBlocks(lines));
    jobNameCount += code.filter((line) => new RegExp(`^\\s*(-\\s+)?name:\\s*['"]?${JOB_NAME}['"]?\\s*$`).test(line)).length;
    if (name !== DCO_FILE && triggerEvents(code).has('pull_request_target')) {
      findings.push({ rule: 'other-workflow-target', detail: `${name} uses pull_request_target` });
    }
    if (name !== DCO_FILE && code.some((line) => new RegExp(`^\\s*(-\\s+)?name:\\s*['"]?${JOB_NAME}['"]?\\s*$`).test(line))) {
      findings.push({ rule: 'duplicate-job', detail: `${name} also defines a job named \`${JOB_NAME}\`` });
    }
  }
  if (jobNameCount !== 1) {
    findings.push({ rule: 'job-name', detail: `\`${JOB_NAME}\` must be defined exactly once across workflows, found ${jobNameCount}` });
  }
  return findings;
}

// ---- the real repository -------------------------------------------------------

const realFiles = Object.fromEntries(
  readdirSync(workflowsDir)
    .filter((name) => /\.ya?ml$/.test(name))
    .map((name) => [name, readFileSync(path.join(workflowsDir, name), 'utf8')]),
);
const realDco = realFiles[DCO_FILE];

const rules = (findings) => new Set(findings.map((finding) => finding.rule));

test('the repository workflows pass every DCO safety rule', () => {
  assert.ok(realDco, `${DCO_FILE} must exist`);
  assert.deepEqual(repositoryWorkflowFindings(realFiles), []);
});

test('ci.yml defines no job named `DCO sign-off`; dco.yml defines exactly one', () => {
  const countIn = (text) => (text.match(new RegExp(`^\\s*(-\\s+)?name:\\s*['"]?${JOB_NAME}['"]?\\s*$`, 'gm')) ?? []).length;
  assert.equal(countIn(realFiles['ci.yml']), 0);
  assert.equal(countIn(realDco), 1);
});

test('dco.yml triggers on pull_request_target only, and no other workflow does', () => {
  assert.match(realDco, /^on:\n {2}pull_request_target:\n/m);
  const onSection = /^on:\n[\s\S]*?^permissions:/m.exec(realDco)[0];
  assert.doesNotMatch(onSection, /\bpull_request\b/);
  for (const [name, text] of Object.entries(realFiles)) {
    if (name === DCO_FILE) continue;
    assert.doesNotMatch(text.replace(/^\s*#.*$/gm, ''), /pull_request_target/, `${name} must not use pull_request_target`);
  }
});

test('the only executed script path is scripts/verify-dco.mjs from the base checkout', () => {
  const lines = readLines(realDco);
  const executed = runBlocks(lines).flatMap((block) => commandFindings(block.text).nodeLines);
  assert.deepEqual(executed, [CHECKER_COMMAND]);
});

// ---- mutation proofs: every rule must actually fire --------------------------------

function mutate(text, search, replacement) {
  assert.ok(text.includes(search), `mutation anchor not found: ${search}`);
  const mutated = text.replace(search, () => replacement);
  assert.notEqual(mutated, text);
  return mutated;
}

const FETCH_DEPTH = '          fetch-depth: 0\n';
const JOB_LINE = `    if: ${JOB_CONDITION}\n`;
const CHECKER_LINE = '          node scripts/verify-dco.mjs --base "$QC_BASE_SHA" --head "$QC_HEAD_SHA"\n';

const dcoMutations = [
  ['trigger', 'pull_request instead of pull_request_target', (t) => mutate(t, 'pull_request_target:', 'pull_request:')],
  ['trigger', 'both triggers', (t) => mutate(t, '  pull_request_target:\n', '  pull_request:\n  pull_request_target:\n')],
  ['trigger', 'inline trigger list', (t) => mutate(t, /^on:\n {2}pull_request_target:\n.*\n/m.exec(t)[0], 'on: [pull_request]\n')],
  ['checkout-option', 'ref at the PR head', (t) =>
    mutate(t, FETCH_DEPTH, `${FETCH_DEPTH}          ref: \${{ github.event.pull_request.head.sha }}\n`)],
  ['checkout-option', 'repository of the fork', (t) =>
    mutate(t, FETCH_DEPTH, `${FETCH_DEPTH}          repository: \${{ github.event.pull_request.head.repo.full_name }}\n`)],
  ['checkout-option', 'ref as a literal', (t) => mutate(t, FETCH_DEPTH, `${FETCH_DEPTH}          ref: refs/pull/1/merge\n`)],
  ['run-interpolation', 'event value in the checker line', (t) =>
    mutate(t, CHECKER_LINE, '          node scripts/verify-dco.mjs --base "$QC_BASE_SHA" --head "${{ github.event.pull_request.head.sha }}"\n')],
  ['run-interpolation', 'head_ref in the fetch', (t) =>
    mutate(t, 'git fetch --no-tags', 'git fetch --no-tags origin "${{ github.head_ref }}" #')],
  ['run-interpolation', 'event value in a shell comment', (t) =>
    mutate(t, '          set -euo pipefail\n', '          set -euo pipefail\n          # ${{ github.event.pull_request.title }}\n')],
  ['run-interpolation', 'inline single-line run', (t) =>
    mutate(t, '      - name: Check Signed-off-by', '      - run: echo "${{ github.event.pull_request.title }}"\n      - name: Check Signed-off-by')],
  ['permissions', 'contents write', (t) => mutate(t, 'permissions:\n  contents: read\n', 'permissions:\n  contents: write\n')],
  ['permissions', 'extra pull-requests write', (t) =>
    mutate(t, 'permissions:\n  contents: read\n', 'permissions:\n  contents: read\n  pull-requests: write\n')],
  ['permissions', 'write-all', (t) => mutate(t, 'permissions:\n  contents: read\n', 'permissions: write-all\n')],
  ['permissions', 'permissions removed', (t) => mutate(t, 'permissions:\n  contents: read\n\n', '')],
  ['permissions', 'job-level id-token', (t) => mutate(t, '    timeout-minutes: 10\n', '    timeout-minutes: 10\n    permissions:\n      id-token: write\n')],
  ['uses', 'unpinned checkout', (t) => mutate(t, PINNED_CHECKOUT, 'actions/checkout@v4')],
  ['uses', 'setup-node added', (t) =>
    mutate(t, '      - name: Check Signed-off-by', '      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020\n      - name: Check Signed-off-by')],
  ['uses', 'third-party action', (t) =>
    mutate(t, '      - name: Check Signed-off-by', '      - uses: someone/else@0123456789012345678901234567890123456789\n      - name: Check Signed-off-by')],
  ['uses', 'cache action', (t) =>
    mutate(t, '      - name: Check Signed-off-by', '      - uses: actions/cache@0123456789012345678901234567890123456789\n      - name: Check Signed-off-by')],
  ['command', 'npm ci step', (t) => mutate(t, '      - name: Check Signed-off-by', '      - run: npm ci\n      - name: Check Signed-off-by')],
  ['command', 'cargo in a block', (t) => mutate(t, CHECKER_LINE, `${CHECKER_LINE}          cargo test\n`)],
  ['command', 'relative script', (t) => mutate(t, CHECKER_LINE, `${CHECKER_LINE}          ./scripts/other.sh\n`)],
  ['command', 'git checkout of the PR head', (t) => mutate(t, CHECKER_LINE, '          git checkout "$QC_HEAD_SHA"\n' + CHECKER_LINE)],
  ['command', 'git merge of the PR head', (t) => mutate(t, CHECKER_LINE, '          git merge "$QC_HEAD_SHA"\n' + CHECKER_LINE)],
  ['command', 'GITHUB_ENV write', (t) => mutate(t, CHECKER_LINE, `${CHECKER_LINE}          echo X=1 >> "$GITHUB_ENV"\n`)],
  ['checker-command', 'checker taken from another path', (t) => mutate(t, 'node scripts/verify-dco.mjs', 'node scripts/verify-public-history.mjs')],
  ['checker-command', 'extra node script', (t) => mutate(t, CHECKER_LINE, `${CHECKER_LINE}          node pr-supplied.mjs\n`)],
  ['checker-command', 'checker not run', (t) => mutate(t, CHECKER_LINE, '')],
  ['checker-command', 'checker args changed', (t) => mutate(t, '--head "$QC_HEAD_SHA"', '--head HEAD')],
  ['secrets', 'secret referenced', (t) => mutate(t, '          QC_PR_NUMBER:', '          TOKEN: ${{ secrets.TOKEN }}\n          QC_PR_NUMBER:')],
  ['bot-guard', 'whole condition removed', (t) => mutate(t, JOB_LINE, '')],
  ['association-guard', 'whole condition removed', (t) => mutate(t, JOB_LINE, '')],
  ['bot-guard', 'Bot guard removed, association guard kept', (t) => mutate(t, `${BOT_GUARD} && `, '')],
  ['association-guard', 'association guard removed, Bot guard kept', (t) => mutate(t, ` && ${ASSOCIATION_GUARD}`, '')],
  ['job-condition', 'association guard removed, Bot guard kept', (t) => mutate(t, ` && ${ASSOCIATION_GUARD}`, '')],
  ['job-condition', 'guard widened with && false', (t) => mutate(t, BOT_GUARD, `${BOT_GUARD} && false`)],
  ['job-condition', 'guard widened with || true', (t) => mutate(t, JOB_CONDITION, `${JOB_CONDITION} || true`)],
  ['job-condition', 'second condition on a step', (t) =>
    mutate(t, '      - name: Check Signed-off-by', "      - if: github.actor != 'x'\n        name: Check Signed-off-by")],
  ['job-condition', 'second job-level if', (t) => mutate(t, JOB_LINE, `${JOB_LINE}    if: true\n`)],
  ...['CONTRIBUTOR', 'FIRST_TIME_CONTRIBUTOR', 'FIRST_TIMER', 'NONE', 'MANNEQUIN', '*'].map((value) => [
    'association-guard',
    `extra trusted value ${value}`,
    (t) => mutate(t, '"COLLABORATOR"]', `"COLLABORATOR","${value}"]`),
  ]),
  ['association-guard', 'a trusted value dropped', (t) => mutate(t, '"OWNER","MEMBER","COLLABORATOR"', '"OWNER","MEMBER"')],
  ['association-guard', 'an empty list replaces the trusted list', (t) => mutate(t, '\'["OWNER","MEMBER","COLLABORATOR"]\'', "'[]'")],
  ['association-guard', 'author_association from a comment event', (t) => mutate(t, JOB_LINE, JOB_LINE.replace(ASSOCIATION_FIELD, 'github.event.comment.author_association'))],
  ['association-guard', 'author_association from another object', (t) =>
    mutate(t, JOB_LINE, JOB_LINE.replace(ASSOCIATION_FIELD, 'github.event.pull_request.head.repo.owner.author_association'))],
  ['association-guard', 'actor-based exemption', (t) => mutate(t, ASSOCIATION_GUARD, "github.actor != 'maintainer'")],
  ['association-guard', 'login-based exemption', (t) =>
    mutate(t, ASSOCIATION_GUARD, "github.event.pull_request.user.login != 'maintainer'")],
  ['association-guard', 'actor exemption added beside the guard', (t) =>
    mutate(t, JOB_CONDITION, `${JOB_CONDITION} && github.actor != 'maintainer'`)],
  ['association-guard', 'a second read of author_association in env', (t) =>
    mutate(t, '          QC_PR_NUMBER:', '          QC_ASSOC: ${{ github.event.pull_request.author_association }}\n          QC_PR_NUMBER:')],
  ['job-name', 'job renamed', (t) => mutate(t, 'name: DCO sign-off', 'name: DCO check')],
];

for (const [rule, label, apply] of dcoMutations) {
  test(`dco.yml mutation is flagged as ${rule}: ${label}`, () => {
    const findings = dcoWorkflowFindings(apply(realDco));
    assert.ok(rules(findings).has(rule), `expected a ${rule} finding, got ${JSON.stringify(findings)}`);
  });
}

test('the mutation table covers every rule the checker can raise for dco.yml', () => {
  const covered = new Set(dcoMutations.map(([rule]) => rule));
  for (const rule of [
    'trigger',
    'permissions',
    'job-name',
    'bot-guard',
    'association-guard',
    'job-condition',
    'uses',
    'checkout-option',
    'secrets',
    'run-interpolation',
    'command',
    'checker-command',
  ]) {
    assert.ok(covered.has(rule), `no mutation exercises ${rule}`);
  }
});

test('comments that mention forbidden things are not findings', () => {
  const noisy = mutate(
    realDco,
    FETCH_DEPTH,
    `${FETCH_DEPTH}          # ref: \${{ github.event.pull_request.head.sha }} and secrets.X and npm ci\n`,
  );
  assert.deepEqual(dcoWorkflowFindings(noisy), []);
});

test('a copy of the DCO job added to ci.yml is flagged as a duplicate', () => {
  const mutated = {
    ...realFiles,
    'ci.yml': `${realFiles['ci.yml']}\n  dco:\n    name: DCO sign-off\n    runs-on: ubuntu-latest\n    steps:\n      - run: true\n`,
  };
  const found = rules(repositoryWorkflowFindings(mutated));
  assert.ok(found.has('duplicate-job'));
  assert.ok(found.has('job-name'));
});

test('pull_request_target in another workflow is flagged', () => {
  for (const edit of [
    (t) => mutate(t, '  pull_request:\n', '  pull_request:\n  pull_request_target:\n'),
    (t) => mutate(t, '  pull_request:\n', '  pull_request_target:\n'),
  ]) {
    const mutated = { ...realFiles, 'ci.yml': edit(realFiles['ci.yml']) };
    assert.ok(rules(repositoryWorkflowFindings(mutated)).has('other-workflow-target'));
  }
  const extra = { ...realFiles, 'extra.yml': 'name: x\non:\n  pull_request_target:\n    types: [opened]\njobs: {}\n' };
  assert.ok(rules(repositoryWorkflowFindings(extra)).has('other-workflow-target'));
  const inline = { ...realFiles, 'extra.yml': 'name: x\non: [push, pull_request_target]\njobs: {}\n' };
  assert.ok(rules(repositoryWorkflowFindings(inline)).has('other-workflow-target'));
});

test('a missing dco.yml is flagged', () => {
  const { [DCO_FILE]: _removed, ...rest } = realFiles;
  const found = rules(repositoryWorkflowFindings(rest));
  assert.ok(found.has('dco-workflow'));
  assert.ok(found.has('job-name'));
});

// ---- the job condition's meaning: a tiny evaluator for GitHub's expression syntax ----
//
// Nothing here can run on GitHub, so the real `if:` text is evaluated locally by
// a minimal interpreter of the subset it uses (context access, string literals,
// `!`, `==`, `!=`, `&&`, `||`, parentheses, `contains`, `fromJSON`) with GitHub's
// documented semantics: strings compare case-insensitively, a missing property
// is null, `contains(array, item)` is a case-insensitive membership test, and
// the job runs when the whole expression is truthy.

function tokenize(source) {
  const tokens = [];
  const pattern = /\s*(?:('(?:[^']|'')*')|(&&|\|\||!=|==)|([!(),])|([A-Za-z_][\w-]*(?:\.[A-Za-z_][\w-]*)*)|(-?\d+(?:\.\d+)?))/y;
  let position = 0;
  while (position < source.length) {
    pattern.lastIndex = position;
    const match = pattern.exec(source);
    if (!match) {
      if (source.slice(position).trim() === '') break;
      throw new Error(`cannot tokenize at ${position}: ${source.slice(position)}`);
    }
    position = pattern.lastIndex;
    if (match[1] !== undefined) tokens.push({ type: 'string', value: match[1].slice(1, -1).replace(/''/g, "'") });
    else if (match[2] !== undefined || match[3] !== undefined) tokens.push({ type: 'punct', value: match[2] ?? match[3] });
    else if (match[4] !== undefined) tokens.push({ type: 'name', value: match[4] });
    else tokens.push({ type: 'number', value: Number(match[5]) });
  }
  return tokens;
}

const looseEqual = (left, right) => {
  if (typeof left === 'string' && typeof right === 'string') return left.toLowerCase() === right.toLowerCase();
  return left === right;
};
const truthy = (value) => !(value === false || value === 0 || value === '' || value === null);

export function evaluateExpression(source, context) {
  const tokens = tokenize(source);
  let index = 0;
  const peek = () => tokens[index];
  const take = (value) => {
    const token = tokens[index];
    if (!token || (value !== undefined && token.value !== value)) throw new Error(`expected ${value ?? 'a token'} at ${index}`);
    index += 1;
    return token;
  };
  const isPunct = (value) => peek()?.type === 'punct' && peek().value === value;
  const functions = {
    contains: (search, item) => {
      if (Array.isArray(search)) return search.some((entry) => looseEqual(entry, item));
      if (typeof search === 'string' && typeof item === 'string') return search.toLowerCase().includes(item.toLowerCase());
      return false;
    },
    fromJSON: (text) => JSON.parse(text),
  };
  const parseOr = () => {
    let value = parseAnd();
    while (isPunct('||')) {
      take();
      const right = parseAnd();
      value = truthy(value) ? value : right;
    }
    return value;
  };
  const parseAnd = () => {
    let value = parseEquality();
    while (isPunct('&&')) {
      take();
      const right = parseEquality();
      value = truthy(value) ? right : value;
    }
    return value;
  };
  const parseEquality = () => {
    let value = parseUnary();
    while (isPunct('==') || isPunct('!=')) {
      const operator = take().value;
      const right = parseUnary();
      value = operator === '==' ? looseEqual(value, right) : !looseEqual(value, right);
    }
    return value;
  };
  const parseUnary = () => {
    if (isPunct('!')) {
      take();
      return !truthy(parseUnary());
    }
    return parsePrimary();
  };
  const parsePrimary = () => {
    const token = take();
    if (token.type === 'punct' && token.value === '(') {
      const value = parseOr();
      take(')');
      return value;
    }
    if (token.type === 'string' || token.type === 'number') return token.value;
    if (token.type !== 'name') throw new Error(`unexpected ${token.value}`);
    if (token.value === 'true') return true;
    if (token.value === 'false') return false;
    if (token.value === 'null') return null;
    if (isPunct('(')) {
      take('(');
      const args = [];
      if (!isPunct(')')) {
        args.push(parseOr());
        while (isPunct(',')) {
          take();
          args.push(parseOr());
        }
      }
      take(')');
      const fn = functions[token.value];
      if (!fn) throw new Error(`unsupported function ${token.value}`);
      return fn(...args);
    }
    let value = context;
    for (const part of token.value.split('.')) value = value !== null && typeof value === 'object' ? (value[part] ?? null) : null;
    return value;
  };
  const result = parseOr();
  if (index !== tokens.length) throw new Error(`trailing tokens from ${index}`);
  return result;
}

const realJobCondition = /^ {4}if: (.*)$/m.exec(realDco)[1].trim();
const prContext = (type, association) => ({
  github: {
    actor: 'maintainer',
    event: {
      pull_request: { user: { type, login: 'maintainer' }, ...(association === undefined ? {} : { author_association: association }) },
    },
  },
});
const jobRuns = (type, association, condition = realJobCondition) => truthy(evaluateExpression(condition, prContext(type, association)));

const ASSOCIATIONS = ['OWNER', 'MEMBER', 'COLLABORATOR', 'CONTRIBUTOR', 'FIRST_TIME_CONTRIBUTOR', 'FIRST_TIMER', 'NONE', 'MANNEQUIN'];

test('the evaluator itself follows GitHub semantics on small expressions', () => {
  const context = { a: { b: 'Bot' } };
  assert.equal(evaluateExpression("a.b == 'bot'", context), true);
  assert.equal(evaluateExpression("a.b != 'User'", context), true);
  assert.equal(evaluateExpression("a.missing != 'x'", context), true);
  assert.equal(evaluateExpression('!contains(fromJSON(\'["X","Y"]\'), a.missing)', context), true);
  assert.equal(evaluateExpression('contains(fromJSON(\'["X","bot"]\'), a.b)', context), true);
  assert.equal(evaluateExpression("false || (true && a.b == 'Bot')", context), true);
  assert.equal(evaluateExpression("'it''s' == 'IT''S'", context), true);
  assert.throws(() => evaluateExpression('unknownFn(1)', context));
});

test('the real job condition: truth table of "job runs (author is checked)"', (t) => {
  const rows = [];
  for (const type of ['User', 'Bot']) {
    for (const association of ASSOCIATIONS) rows.push({ type, association, checked: jobRuns(type, association) });
  }
  rows.push({ type: 'User', association: '(absent)', checked: jobRuns('User', undefined) });
  rows.push({ type: 'User', association: '(unknown value)', checked: jobRuns('User', 'SOMETHING_NEW') });
  const trusted = new Set(TRUSTED_ASSOCIATIONS);
  for (const row of rows) {
    const expected = row.type !== 'Bot' && !trusted.has(row.association);
    assert.equal(row.checked, expected, `${row.type} / ${row.association}`);
  }
  const table = rows.map((row) => `${row.type.padEnd(5)} ${row.association.padEnd(24)} ${row.checked ? 'CHECKED' : 'skipped (passes)'}`);
  t.diagnostic(`DCO job truth table (user.type, author_association, result)\n${table.join('\n')}`);
});

test('outside contributors are checked and maintainers are skipped, spelled out', () => {
  for (const association of ['OWNER', 'MEMBER', 'COLLABORATOR']) assert.equal(jobRuns('User', association), false, association);
  for (const association of ['CONTRIBUTOR', 'FIRST_TIME_CONTRIBUTOR', 'FIRST_TIMER', 'NONE', 'MANNEQUIN']) {
    assert.equal(jobRuns('User', association), true, association);
  }
  for (const association of ASSOCIATIONS) assert.equal(jobRuns('Bot', association), false, `Bot / ${association}`);
});

test('condition mutants that add a trusted value or drop a guard change the truth table', () => {
  for (const value of ['CONTRIBUTOR', 'FIRST_TIME_CONTRIBUTOR', 'FIRST_TIMER', 'NONE', 'MANNEQUIN']) {
    const condition = realJobCondition.replace('"COLLABORATOR"]', `"COLLABORATOR","${value}"]`);
    assert.notEqual(condition, realJobCondition);
    assert.equal(jobRuns('User', value, condition), false, `${value} would wrongly be exempted`);
    assert.equal(jobRuns('User', value), true, `${value} is checked by the real condition`);
  }
  const noAssociation = realJobCondition.replace(` && ${ASSOCIATION_GUARD}`, '');
  assert.notEqual(noAssociation, realJobCondition);
  assert.equal(jobRuns('User', 'OWNER', noAssociation), true, 'without the guard a maintainer is checked');
  const noBot = realJobCondition.replace(`${BOT_GUARD} && `, '');
  assert.notEqual(noBot, realJobCondition);
  assert.equal(jobRuns('Bot', 'NONE', noBot), true, 'without the Bot guard a bot is checked');
});

test('CI bounds jobs, caches npm, and verifies the quadcortex crate on Linux', () => {
  const ci = readFileSync(path.join(workflowsDir, 'ci.yml'), 'utf8');
  // Only pull request runs are cancelled; a master run must finish (it warms the caches).
  assert.match(ci, /^concurrency:\n[\s\S]*?cancel-in-progress: \$\{\{ github\.event_name == 'pull_request' \}\}/m);
  const jobsStart = ci.indexOf('\njobs:\n') + '\njobs:\n'.length;
  const jobs = ci.slice(jobsStart).split(/^  [a-z][a-z0-9-]*:\s*$/m).slice(1);
  assert.ok(jobs.length >= 8, `expected the CI job set, got ${jobs.length}`);
  for (const job of jobs) assert.match(job, /^    timeout-minutes: \d+$/m, 'every CI job needs a finite timeout');
  assert.match(ci, /node scripts\/test-windows-native\.mjs --root \./);
  assert.match(ci, /quadcortex-linux:[\s\S]*?runs-on: ubuntu-latest[\s\S]*?cargo test -p quadcortex --locked[\s\S]*?RUSTDOCFLAGS: -D warnings/);
  assert.match(ci, /name: TypeScript packages and apps[\s\S]*?cache: npm[\s\S]*?npm run typecheck[\s\S]*?npm test/);
  assert.doesNotMatch(ci, /- run: npm run build\s*$/m, 'the workspace build duplicates the following typecheck gate');
});

test('CI gates native jobs on the change filter and rebuilds them on master to keep caches warm', () => {
  const ci = readFileSync(path.join(workflowsDir, 'ci.yml'), 'utf8');
  const bodyOf = (name) => {
    const marker = `\n  ${name}:\n`;
    const start = ci.indexOf(marker);
    assert.notEqual(start, -1, `${name} job is missing`);
    const bodyStart = start + marker.length;
    const next = ci.slice(bodyStart).search(/^  [a-z][a-z0-9-]*:\s*$/m);
    return next === -1 ? ci.slice(bodyStart) : ci.slice(bodyStart, bodyStart + next);
  };
  assert.match(bodyOf('third-party-notices'), /^    if: github\.event_name == 'pull_request'$/m, 'third-party-notices must not rebuild protected master');
  // TypeScript and the relay are pull-request only too, and are skipped (never
  // dropped) when the filter says their inputs did not change; same fail-open rule.
  for (const [name, output] of [['typescript', 'typescript'], ['cloud-relay', 'cloud']]) {
    const body = bodyOf(name);
    assert.match(body, /^    needs: changes$/m, `${name} must depend on the change filter`);
    assert.ok(
      body.includes(`    if: \${{ !cancelled() && github.event_name == 'pull_request' && (needs.changes.result != 'success' || needs.changes.outputs.${output} != 'false') }}\n`),
      `${name} must be pull-request only and carry the fail-open ${output} gate`,
    );
  }
  // The native jobs also run on master, to refresh the Rust/Gradle caches that
  // pull requests restore. On a pull request they are skipped (never dropped)
  // when the change filter says nothing native changed, and they run unless
  // `changes` succeeded and answered exactly 'false' (fail open).
  const nativeGate = "if: ${{ !cancelled() && (needs.changes.result != 'success' || needs.changes.outputs.native != 'false') }}";
  const nativeJobs = ['rust', 'rust-msrv', 'rust-windows-app-static', 'rust-windows-app-tests', 'quadcortex-linux', 'android'];
  for (const name of nativeJobs) {
    const body = bodyOf(name);
    assert.match(body, /^    needs: changes$/m, `${name} must depend on the change filter`);
    assert.ok(body.includes(`    ${nativeGate}\n`), `${name} must carry the fail-open native gate`);
    assert.ok(body.includes("save-if: ${{ github.ref == 'refs/heads/master' }}"), `${name} must save its cache only from master`);
  }
  const changes = bodyOf('changes');
  assert.doesNotMatch(changes, /^    if:/m, 'the change filter runs on every event');
  assert.match(changes, /node scripts\/ci-changes\.mjs/);
  assert.doesNotMatch(bodyOf('public-history'), /^    if: github\.event_name == 'pull_request'$/m);
});

test('every scripts/*.test.mjs runs in a workflow or a root package script', () => {
  const scriptsDir = path.dirname(fileURLToPath(import.meta.url));
  const runners = [
    ...readdirSync(workflowsDir).filter((name) => /\.ya?ml$/.test(name)).map((name) => readFileSync(path.join(workflowsDir, name), 'utf8')),
    readFileSync(path.join(scriptsDir, '..', 'package.json'), 'utf8'),
  ].join('\n');
  const unrun = readdirSync(scriptsDir)
    .filter((name) => name.endsWith('.test.mjs'))
    .filter((name) => !runners.includes(`scripts/${name}`));
  assert.deepEqual(unrun, [], 'a script test that no workflow runs never gates anything');
});
