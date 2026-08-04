#!/usr/bin/env node
// Stamps a release version into every place the app reads it from:
//   package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml,
//   src-tauri/Cargo.lock (the [[package]] entry for cc-switch itself).
//
// All four must move together — Tauri bakes tauri.conf.json's version into the
// bundle metadata shown in the app, so stamping only the git tag ships a
// binary that reports the previous release (this has happened twice: ciao.4
// and ciao.6).
//
// Usage:
//   node scripts/bump-version.mjs 3.19.1+ciao.7   # set an explicit version
//   node scripts/bump-version.mjs --check 3.19.1+ciao.7   # verify only (CI)
//   node scripts/bump-version.mjs --check-tag v3.19.1+ciao.7   # strip the v, verify

import { readFileSync, writeFileSync } from 'node:fs';

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

const args = process.argv.slice(2);
const checkOnly = args[0] === '--check' || args[0] === '--check-tag';
const raw = checkOnly ? args[1] : args[0];
if (!raw || args.length !== (checkOnly ? 2 : 1)) {
  console.error('Usage: node scripts/bump-version.mjs [--check|--check-tag] <version>');
  process.exit(1);
}
const version = args[0] === '--check-tag' ? raw.replace(/^v/, '') : raw;
if (!SEMVER.test(version)) {
  console.error(`Not a semver version: ${version}`);
  process.exit(1);
}

const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'));

const sources = [
  { path: 'package.json', read: () => readJson('package.json').version },
  { path: 'src-tauri/tauri.conf.json', read: () => readJson('src-tauri/tauri.conf.json').version },
  {
    path: 'src-tauri/Cargo.toml',
    read: () => readFileSync('src-tauri/Cargo.toml', 'utf8').match(/^version = "(.*)"$/m)?.[1],
  },
  {
    path: 'src-tauri/Cargo.lock',
    read: () =>
      readFileSync('src-tauri/Cargo.lock', 'utf8').match(
        /\[\[package\]\]\nname = "cc-switch"\nversion = "(.*)"/,
      )?.[1],
  },
];

const current = sources.map(({ path, read }) => ({ path, version: read() }));

if (checkOnly) {
  const mismatched = current.filter(({ version: v }) => v !== version);
  if (mismatched.length > 0) {
    console.error(`Version fields do not match ${version}:`);
    for (const { path, version: v } of current) console.error(`  ${path}: ${v}`);
    console.error(`Run: node scripts/bump-version.mjs ${version}`);
    process.exit(1);
  }
  console.log(`All version fields match ${version}`);
  process.exit(0);
}

// Refuse to clobber a mixed state silently — bump from a consistent base.
const distinct = new Set(current.map(({ version: v }) => v));
if (distinct.size > 1) {
  console.error('Version fields are inconsistent, refusing to guess the base:');
  for (const { path, version: v } of current) console.error(`  ${path}: ${v}`);
  process.exit(1);
}

const bumpJson = (path, key) => {
  const text = readFileSync(path, 'utf8');
  const replaced = text.replace(`"${key}": "${current[0].version}"`, `"${key}": "${version}"`);
  if (replaced === text) throw new Error(`Failed to bump ${key} in ${path}`);
  writeFileSync(path, replaced);
};

bumpJson('package.json', 'version');
bumpJson('src-tauri/tauri.conf.json', 'version');

const cargoToml = readFileSync('src-tauri/Cargo.toml', 'utf8');
writeFileSync(
  'src-tauri/Cargo.toml',
  cargoToml.replace(/^version = ".*"$/m, `version = "${version}"`),
);

const lockPath = 'src-tauri/Cargo.lock';
const lock = readFileSync(lockPath, 'utf8');
writeFileSync(
  lockPath,
  lock.replace(
    /(\[\[package\]\]\nname = "cc-switch"\nversion = ").*"/,
    `$1${version}"`,
  ),
);

console.log(`${current[0].version} -> ${version}`);
for (const { path } of sources) console.log(`  bumped ${path}`);
