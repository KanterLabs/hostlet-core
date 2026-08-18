#!/usr/bin/env node

/**
 * Regenerate the Core code-map artifacts from the checked-in editorial model.
 *
 * The model stays intentionally curated: source inspection can prove that
 * anchors, callers, effects, tests, and workflow edges still exist, but it
 * cannot infer product boundaries or release semantics.
 *
 * Usage:
 *   node scripts/generate-codemap.mjs
 *   node scripts/generate-codemap.mjs --check
 *   node scripts/generate-codemap.mjs --root /path/to/hostlet-core
 *   node scripts/generate-codemap.mjs --out docs/codemap
 *
 * Generation is deterministic.  The timestamp is taken from the selected
 * source commit (or SOURCE_DATE_EPOCH when supplied), never from the wall
 * clock.  The script only reads the checkout and writes the selected output
 * directory; no network or task-specific state is consulted.
 */

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const scriptRoot = path.resolve(path.dirname(scriptPath), "..");
const defaultOutput = "docs/codemap";
const excludedPaths = [
  "docs/codemap/**",
  "apps/web/.design-sync/**",
  "scripts/fixtures/generated-apps/**",
];
const excludedDirectories = [
  "docs/codemap",
  "apps/web/.design-sync",
  "scripts/fixtures/generated-apps",
];
const partitionNames = [
  "apps/api",
  "apps/agent",
  "apps/cli",
  "apps/screenshotter",
  "apps/web",
  "crates/contracts",
  "infra",
  "scripts",
  "docs",
  ".github",
  "repository-root",
];
const scope = [...partitionNames];
const recordFormat =
  "codemap-fingerprint-v1\\0 + bytewise path order + decimal path length:path\\0 + decimal content length:content\\0; missing tracked file content is MISSING\\0";

function usage(message = "") {
  if (message) console.error(`error: ${message}`);
  console.error("usage: node scripts/generate-codemap.mjs [--check] [--root PATH] [--out PATH]");
  process.exit(message ? 2 : 0);
}

function parseArgs(argv) {
  const result = { check: false, root: scriptRoot, out: defaultOutput };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--check") result.check = true;
    else if (arg === "--root") {
      const value = argv[++index];
      if (!value) usage("--root requires a path");
      result.root = path.resolve(value);
    } else if (arg === "--out") {
      const value = argv[++index];
      if (!value) usage("--out requires a path");
      result.out = value;
    } else if (arg === "--help" || arg === "-h") usage();
    else usage(`unknown argument ${arg}`);
  }
  result.out = path.resolve(result.root, result.out);
  return result;
}

function git(root, ...args) {
  return execFileSync("git", ["-C", root, ...args], { encoding: "utf8" }).trim();
}

function isInside(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith(`..${path.sep}`) && relative !== "..");
}

function relativePath(root, candidate) {
  return path.relative(root, candidate).split(path.sep).join("/");
}

function trackedPaths(root) {
  const bytes = execFileSync("git", ["-C", root, "ls-files", "-z"]);
  const paths = [];
  let start = 0;
  for (let index = 0; index < bytes.length; index += 1) {
    if (bytes[index] !== 0) continue;
    if (index > start) paths.push(bytes.subarray(start, index).toString("utf8"));
    start = index + 1;
  }
  return paths.filter(filePath => !excludedPaths.some(pattern => {
    const prefix = pattern.endsWith("/**") ? pattern.slice(0, -2) : pattern;
    return filePath === prefix || filePath.startsWith(prefix);
  }));
}

function partition(filePath) {
  return partitionNames.find(name => name !== "repository-root" && filePath.startsWith(`${name}/`))
    ?? "repository-root";
}

function fingerprint(root, filePaths) {
  const hash = crypto.createHash("sha256");
  hash.update(Buffer.from("codemap-fingerprint-v1\0", "utf8"));
  for (const filePath of [...filePaths].sort((left, right) =>
    Buffer.from(left).compare(Buffer.from(right)))) {
    const pathBytes = Buffer.from(filePath, "utf8");
    const fullPath = path.join(root, filePath);
    const data = fs.existsSync(fullPath) ? fs.readFileSync(fullPath) : Buffer.from("MISSING\0", "utf8");
    hash.update(Buffer.from(`${pathBytes.length}:`, "utf8"));
    hash.update(pathBytes);
    hash.update(Buffer.from("\0", "utf8"));
    hash.update(Buffer.from(`${data.length}:`, "utf8"));
    hash.update(data);
    hash.update(Buffer.from("\0", "utf8"));
  }
  return hash.digest("hex");
}

function buildFingerprints(root) {
  const tracked = trackedPaths(root);
  const grouped = Object.fromEntries(partitionNames.map(name => [
    name,
    tracked.filter(filePath => partition(filePath) === name),
  ]));
  const moduleFingerprints = Object.fromEntries(partitionNames.map(name => [
    name,
    fingerprint(root, grouped[name]),
  ]));
  const moduleFileCounts = Object.fromEntries(partitionNames.map(name => [name, grouped[name].length]));
  return {
    tracked,
    moduleFingerprints,
    moduleFileCounts,
    fingerprint: fingerprint(root, tracked),
  };
}

function sourceDirty(root, out) {
  const outputPath = relativePath(root, out);
  const ignored = new Set([outputPath, "docs/codemap"]);
  const status = execFileSync(
    "git",
    ["-C", root, "status", "--porcelain", "--untracked-files=all"],
    { encoding: "utf8" },
  );
  return status.split("\n").filter(Boolean).some(line => {
    const filePath = line.slice(3).split(" -> ").at(-1);
    // The generator is staged before artifacts are rendered so its own bytes
    // participate in the lock fingerprint. A staged-only generator is part of
    // this intentional artifact update; a worktree edit still counts dirty.
    if (filePath === "scripts/generate-codemap.mjs" && line[1] === " ") return false;
    return ![...ignored].some(prefix => filePath === prefix || filePath.startsWith(`${prefix}/`));
  });
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function isReference(value) {
  return value && typeof value.path === "string" && value.path.length > 0
    && typeof value.symbol === "string" && value.symbol.length > 0;
}

function validateReferences(root, owner, field) {
  const refs = owner[field] ?? [];
  assert(Array.isArray(refs), `${owner.id ?? "map"}.${field} must be an array`);
  for (const reference of refs) {
    assert(isReference(reference), `${owner.id ?? "map"}.${field} has an invalid reference`);
    const referencePath = path.resolve(root, reference.path);
    assert(isInside(root, referencePath), `${owner.id ?? "map"}.${field} escapes the checkout`);
    assert(fs.existsSync(referencePath) && fs.statSync(referencePath).isFile(),
      `${owner.id ?? "map"}.${field} refers to missing file ${reference.path}`);
    assert(fs.readFileSync(referencePath, "utf8").includes(reference.symbol),
      `${owner.id ?? "map"}.${field} cannot find ${reference.symbol} in ${reference.path}`);
  }
}

function validateMap(root, map) {
  assert(map && typeof map === "object", "codemap.json must be an object");
  assert(Array.isArray(map.nodes) && map.nodes.length > 0, "codemap must contain nodes");
  assert(Array.isArray(map.edges), "codemap.edges must be an array");
  assert(Array.isArray(map.flows), "codemap.flows must be an array");

  const nodeIds = new Set();
  for (const node of map.nodes) {
    assert(node && typeof node.id === "string" && node.id.length > 0, "node id is required");
    assert(!nodeIds.has(node.id), `duplicate node id ${node.id}`);
    nodeIds.add(node.id);
    const nodePath = path.resolve(root, node.path ?? "");
    assert(isInside(root, nodePath) && fs.existsSync(nodePath) && fs.statSync(nodePath).isFile(),
      `node ${node.id} has missing path ${node.path}`);
    assert(typeof node.role === "string" && node.role.length > 0, `node ${node.id} needs a role`);
    validateReferences(root, node, "entrypoints");
    validateReferences(root, node, "callers");
    validateReferences(root, node, "effects");
    validateReferences(root, node, "evidence");
    validateReferences(root, node, "tests");
  }

  const edgeIds = new Set();
  for (const edge of map.edges) {
    assert(edge && typeof edge.id === "string" && edge.id.length > 0, "edge id is required");
    assert(!edgeIds.has(edge.id), `duplicate edge id ${edge.id}`);
    edgeIds.add(edge.id);
    assert(nodeIds.has(edge.from) && nodeIds.has(edge.to), `edge ${edge.id} has an unknown endpoint`);
    validateReferences(root, edge, "evidence");
  }

  const flowIds = new Set();
  for (const flow of map.flows) {
    assert(flow && typeof flow.id === "string" && flow.id.length > 0, "flow id is required");
    assert(!flowIds.has(flow.id), `duplicate flow id ${flow.id}`);
    flowIds.add(flow.id);
    assert(Array.isArray(flow.steps) && flow.steps.every(id => nodeIds.has(id)),
      `flow ${flow.id} has an unknown node`);
    assert(Array.isArray(flow.edges) && flow.edges.every(id => edgeIds.has(id)),
      `flow ${flow.id} has an unknown edge`);
  }

  for (const symbol of map.symbol_index ?? []) {
    assert(isReference(symbol), "symbol_index has an invalid anchor");
    validateReferences(root, symbol, "callers");
    validateReferences(root, symbol, "effects");
    validateReferences(root, symbol, "tests");
  }
}

function readArtifacts(out) {
  const mapPath = path.join(out, "codemap.json");
  const htmlPath = path.join(out, "codemap.html");
  const lockPath = path.join(out, "codemap.lock");
  assert(fs.existsSync(mapPath), `missing ${mapPath}; seed the curated map before generating`);
  assert(fs.existsSync(htmlPath), `missing ${htmlPath}; seed the HTML viewer before generating`);
  return {
    mapPath,
    htmlPath,
    lockPath,
    map: JSON.parse(fs.readFileSync(mapPath, "utf8")),
    html: fs.readFileSync(htmlPath, "utf8"),
  };
}

function htmlWithMap(template, mapJson) {
  const marker = /(<script id="codemap-data" type="application\/json">)([\s\S]*?)(<\/script>)/g;
  const matches = template.match(marker) ?? [];
  assert(matches.length === 1, "codemap.html must contain one codemap-data script element");
  return template.replace(marker, (_whole, open, _payload, close) => `${open}\n${mapJson}${close}`)
    .replace(/\n+$/, "\n");
}

function selectedCommit(root, existingMap) {
  const head = git(root, "rev-parse", "HEAD");
  assert(/^[0-9a-f]{40}$/.test(head), "HEAD is not a full commit SHA");
  const existing = existingMap?.generated_from_commit;
  if (existing === head) return head;
  try {
    const parent = git(root, "rev-parse", "HEAD^");
    if (existing === parent) return parent;
  } catch {
    // An initial commit has no parent; HEAD remains the only valid source.
  }
  return head;
}

function generatedAt(root, commit) {
  const epoch = process.env.SOURCE_DATE_EPOCH;
  if (epoch !== undefined) {
    assert(/^[0-9]+$/.test(epoch), "SOURCE_DATE_EPOCH must be an integer number of seconds");
    return new Date(Number(epoch) * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");
  }
  return new Date(git(root, "show", "-s", "--format=%cI", commit)).toISOString()
    .replace(/\.\d{3}Z$/, "Z");
}

function buildLock(root, generatedAtValue, generatedFromCommit, dirty) {
  const fingerprints = buildFingerprints(root);
  const partitions = partitionNames.map(id => ({
    id,
    file_count: fingerprints.moduleFileCounts[id],
    fingerprint: fingerprints.moduleFingerprints[id],
  }));
  return {
    generated_at: generatedAtValue,
    generated_from_commit: generatedFromCommit,
    algorithm: "sha256",
    fingerprint_algorithm: "sha256 / codemap-fingerprint-v1",
    working_tree_has_uncommitted_changes: dirty,
    record_format: recordFormat,
    scope,
    scanned_scope: scope,
    excluded_paths: excludedPaths,
    excluded_directories: excludedDirectories,
    partitions,
    module_fingerprints: fingerprints.moduleFingerprints,
    module_file_counts: fingerprints.moduleFileCounts,
    total_file_count: fingerprints.tracked.length,
    fingerprint: fingerprints.fingerprint,
  };
}

function check(root, out) {
  assert(isInside(root, out), "--out must stay inside the checkout");
  const { mapPath, htmlPath, lockPath, map, html } = readArtifacts(out);
  assert(fs.existsSync(lockPath), `missing ${lockPath}`);
  validateMap(root, map);
  const mapJson = `${JSON.stringify(map, null, 2)}\n`;
  assert(fs.readFileSync(mapPath, "utf8") === mapJson, "codemap.json is not canonical JSON");
  assert(html === htmlWithMap(html, mapJson), "HTML embedded map differs from codemap.json");

  const lock = JSON.parse(fs.readFileSync(lockPath, "utf8"));
  assert(lock.generated_at === map.generated_at, "lock and map generated_at differ");
  assert(lock.generated_from_commit === map.generated_from_commit,
    "lock and map generated_from_commit differ");
  assert(lock.algorithm === "sha256" && lock.fingerprint_algorithm === "sha256 / codemap-fingerprint-v1",
    "lock fingerprint algorithm is not canonical");
  assert(JSON.stringify(lock.scope) === JSON.stringify(scope), "lock scope differs from generator scope");
  const fingerprints = buildFingerprints(root);
  assert(lock.total_file_count === fingerprints.tracked.length,
    "lock total_file_count differs from current tracked scope");
  assert(lock.fingerprint === fingerprints.fingerprint,
    "lock fingerprint differs from current tracked content");
  assert(JSON.stringify(lock.module_fingerprints) === JSON.stringify(fingerprints.moduleFingerprints),
    "lock module fingerprints differ from current tracked content");
  assert(JSON.stringify(lock.module_file_counts) === JSON.stringify(fingerprints.moduleFileCounts),
    "lock module file counts differ from current tracked scope");
  for (const expected of partitionNames.map(id => ({
    id,
    file_count: fingerprints.moduleFileCounts[id],
    fingerprint: fingerprints.moduleFingerprints[id],
  }))) {
    const actual = lock.partitions?.find(item => item.id === expected.id);
    assert(actual?.file_count === expected.file_count && actual?.fingerprint === expected.fingerprint,
      `lock partition ${expected.id} differs from current tracked content`);
  }
  console.log(JSON.stringify({
    result: "pass",
    root,
    generated_from_commit: map.generated_from_commit,
    tracked_inputs: fingerprints.tracked.length,
    fingerprint: fingerprints.fingerprint,
    nodes: map.nodes.length,
    edges: map.edges.length,
    flows: map.flows.length,
  }, null, 2));
}

function generate(root, out) {
  assert(isInside(root, out), "--out must stay inside the checkout");
  const dirty = sourceDirty(root, out);
  const { mapPath, htmlPath, lockPath, map, html } = readArtifacts(out);
  const generatedFromCommit = selectedCommit(root, map);
  const generatedAtValue = generatedAt(root, generatedFromCommit);
  map.generated_at = generatedAtValue;
  map.generated_from_commit = generatedFromCommit;
  map.scope = scope;
  map.algorithm = "sha256";
  map.fingerprint_algorithm = "sha256 / codemap-fingerprint-v1";
  map.working_tree_has_uncommitted_changes = dirty;
  validateMap(root, map);
  const mapJson = `${JSON.stringify(map, null, 2)}\n`;
  const renderedHtml = htmlWithMap(html, mapJson);
  const lock = buildLock(root, generatedAtValue, generatedFromCommit, dirty);
  fs.mkdirSync(out, { recursive: true });
  fs.writeFileSync(mapPath, mapJson);
  fs.writeFileSync(htmlPath, renderedHtml);
  fs.writeFileSync(lockPath, `${JSON.stringify(lock, null, 2)}\n`);
  console.log(JSON.stringify({
    result: "generated",
    root,
    output: out,
    generated_from_commit: generatedFromCommit,
    source_dirty_before_generation: dirty,
    tracked_inputs: lock.total_file_count,
    fingerprint: lock.fingerprint,
    nodes: map.nodes.length,
    edges: map.edges.length,
    flows: map.flows.length,
  }, null, 2));
}

try {
  const args = parseArgs(process.argv.slice(2));
  if (args.check) check(args.root, args.out);
  else generate(args.root, args.out);
} catch (error) {
  console.error(`codemap: ${error.message}`);
  process.exit(1);
}
