#!/usr/bin/env node
// Measures how well Onus's `dependents` answers match the TypeScript
// compiler's own "find all references", the Phase 2 gate (PLAN.md).
//
//   onus map <repo> --json > map.json
//   node scripts/benchmark-references.mjs <repo> map.json \
//     --typescript <dir with node_modules/typescript> [--sample 100] [--seed 1]
//
// For a deterministic sample of public symbols, it compares the files Onus
// says depend on each symbol (imports, calls and type references, outside
// the symbol's own file) with the files the TypeScript language service
// finds references in. It prints precision and recall per symbol kind and
// overall, and the symbols with the most disagreement.
//
// The language service only parses and type-checks; nothing in the
// repository runs. Load TypeScript from outside the repository being
// measured (`--typescript`), never from its node_modules. Workspace
// packages are resolved to their sources through `paths` built from the
// map, so the repository needs no installed dependencies.

import { readFileSync, existsSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve, relative } from "node:path";

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const [repoArg, mapArg] = args.filter((a, i) => !a.startsWith("--") && !args[i - 1]?.startsWith("--"));
if (!repoArg || !mapArg || !flag("typescript")) {
  console.error("usage: benchmark-references.mjs <repo> <map.json> --typescript <dir> [--sample N] [--seed N]");
  process.exit(2);
}
const repo = resolve(repoArg);
const ts = createRequire(join(resolve(flag("typescript")), "package.json"))("typescript");
const map = JSON.parse(readFileSync(mapArg, "utf8"));
const sampleSize = Number(flag("sample", "100"));
let seed = Number(flag("seed", "1"));

const SOURCE = /\.(ts|tsx|mts|cts)$/;
const files = map.files.map((f) => f.path).filter((p) => SOURCE.test(p) && !p.endsWith(".d.ts"));
const fileSet = new Set(files);

// Compiler options: the root tsconfig if any, plus paths to workspace sources.
let options = { allowJs: false, noEmit: true, skipLibCheck: true, jsx: ts.JsxEmit.Preserve };
for (const name of ["tsconfig.base.json", "tsconfig.json"]) {
  const p = join(repo, name);
  if (existsSync(p)) {
    const read = ts.readConfigFile(p, ts.sys.readFile);
    if (read.config) {
      options = { ...ts.parseJsonConfigFileContent(read.config, ts.sys, repo).options, ...options };
    }
    break;
  }
}
options.baseUrl = repo;
options.moduleResolution = ts.ModuleResolutionKind.Bundler;
options.module = ts.ModuleKind.ESNext;
options.paths = { ...(options.paths ?? {}) };
for (const c of map.components) {
  if (!c.packageName || !c.publicEntrypoints?.length) continue;
  const dir = (c.roots[0] ?? "").replace(/\/?\*\*$/, "");
  options.paths[c.packageName] = c.publicEntrypoints;
  options.paths[`${c.packageName}/*`] = [`${dir}/src/*`, `${dir}/*`];
}

const host = {
  getScriptFileNames: () => files.map((f) => join(repo, f)),
  getScriptVersion: () => "1",
  getScriptSnapshot: (name) =>
    existsSync(name) ? ts.ScriptSnapshot.fromString(readFileSync(name, "utf8")) : undefined,
  getCurrentDirectory: () => repo,
  getCompilationSettings: () => options,
  getDefaultLibFileName: (o) => ts.getDefaultLibFilePath(o),
  fileExists: ts.sys.fileExists,
  readFile: ts.sys.readFile,
  readDirectory: ts.sys.readDirectory,
  directoryExists: ts.sys.directoryExists,
  getDirectories: ts.sys.getDirectories,
};
const service = ts.createLanguageService(host, ts.createDocumentRegistry());

// A deterministic sample of public, located symbols, spread over kinds.
const KINDS = new Set(["function", "class", "interface", "type", "const", "enum", "variable"]);
const rand = () => {
  seed = (seed * 1103515245 + 12345) % 2147483648;
  return seed / 2147483648;
};
const candidates = map.symbols.filter(
  (s) => s.visibility === "public" && KINDS.has(s.kind) && s.loc && fileSet.has(s.loc.file),
);
const byKind = new Map();
for (const s of candidates) {
  if (!byKind.has(s.kind)) byKind.set(s.kind, []);
  byKind.get(s.kind).push(s);
}
const sample = [];
const kinds = [...byKind.keys()].sort();
while (sample.length < Math.min(sampleSize, candidates.length)) {
  for (const k of kinds) {
    const list = byKind.get(k);
    if (list.length && sample.length < sampleSize) {
      sample.push(list.splice(Math.floor(rand() * list.length), 1)[0]);
    }
  }
}

// Onus: files with a code dependency on the symbol or its members.
const CODE = new Set(["imports", "calls", "references-type"]);
const onusFiles = (s) => {
  const out = new Set();
  for (const e of map.edges) {
    if (!CODE.has(e.kind) || (e.to !== s.id && !e.to.startsWith(`${s.id}.`))) continue;
    for (const site of e.sites) if (site.file !== s.loc.file) out.add(site.file);
  }
  return out;
};

// TypeScript: files with a reference to the declaration.
const tsFiles = (s) => {
  const abs = join(repo, s.loc.file);
  const text = readFileSync(abs, "utf8");
  const lines = text.split("\n");
  let offset = 0;
  for (let i = 0; i < s.loc.start - 1; i++) offset += lines[i].length + 1;
  const line = lines[s.loc.start - 1] ?? "";
  const name = s.name.split(".").pop();
  const col = line.search(new RegExp(`\\b${name.replace(/[$]/g, "\\$")}\\b`));
  if (col < 0) return null;
  const refs = service.findReferences(abs, offset + col) ?? [];
  const out = new Set();
  for (const r of refs) {
    for (const ref of r.references) {
      const rel = relative(repo, ref.fileName).split("\\").join("/");
      if (rel.startsWith("..") || rel.includes("node_modules")) continue;
      if (rel !== s.loc.file && fileSet.has(rel)) out.add(rel);
    }
  }
  return out;
};

const totals = new Map();
const add = (k, tp, fp, fn) => {
  const t = totals.get(k) ?? { symbols: 0, tp: 0, fp: 0, fn: 0 };
  t.symbols += 1;
  t.tp += tp;
  t.fp += fp;
  t.fn += fn;
  totals.set(k, t);
};
const disagreements = [];
let skipped = 0;
for (const s of sample) {
  const truth = tsFiles(s);
  if (!truth) {
    skipped += 1;
    continue;
  }
  const got = onusFiles(s);
  const tp = [...got].filter((f) => truth.has(f)).length;
  const fp = [...got].filter((f) => !truth.has(f));
  const fn = [...truth].filter((f) => !got.has(f));
  add(s.kind, tp, fp.length, fn.length);
  add("all", tp, fp.length, fn.length);
  if (fp.length || fn.length) {
    disagreements.push({ id: s.id, kind: s.kind, onus: got.size, typescript: truth.size, extra: fp.slice(0, 3), missed: fn.slice(0, 3) });
  }
}
const ratio = (a, b) => (b === 0 ? null : Math.round((1000 * a) / b) / 1000);
const result = {
  repo: relative(process.cwd(), repo) || ".",
  typescript: ts.version,
  sampled: sample.length,
  skipped,
  byKind: Object.fromEntries(
    [...totals].sort().map(([k, t]) => [k, { ...t, precision: ratio(t.tp, t.tp + t.fp), recall: ratio(t.tp, t.tp + t.fn) }]),
  ),
  disagreements: disagreements.sort((a, b) => b.missed.length + b.extra.length - (a.missed.length + a.extra.length)).slice(0, 15),
};
console.log(JSON.stringify(result, null, 2));
