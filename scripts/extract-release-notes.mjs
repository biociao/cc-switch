#!/usr/bin/env node
// Extracts one version's section from CHANGELOG.md for the release body.
//
// The release workflow (publish-release job) appends this to the download
// template so every tag's GitHub Release carries its own notes instead of
// the bare download links.
//
// Usage:
//   node scripts/extract-release-notes.mjs v3.19.2+ciao.7   # prints the section
//   node scripts/extract-release-notes.mjs 3.19.2+ciao.7    # leading v optional
//
// Prints nothing (exit 0) when the section is missing — a release must never
// fail just because the changelog entry was forgotten.

import { readFileSync } from 'node:fs';

const raw = process.argv[2];
if (!raw || process.argv.length !== 3) {
  console.error('Usage: node scripts/extract-release-notes.mjs <tag-or-version>');
  process.exit(1);
}
const version = raw.replace(/^v/, '');

const changelog = readFileSync('CHANGELOG.md', 'utf8');
const lines = changelog.split('\n');

const heading = `## [${version}]`;
const start = lines.findIndex((line) => line.startsWith(heading));
if (start === -1) {
  console.error(`warning: no CHANGELOG.md section found for ${heading}`);
  process.exit(0);
}

let end = lines.length;
for (let i = start + 1; i < lines.length; i++) {
  if (lines[i].startsWith('## [')) {
    end = i;
    break;
  }
}

// Drop the heading itself and surrounding blank lines / trailing "---" rule.
const body = lines
  .slice(start + 1, end)
  .join('\n')
  .trim()
  .replace(/\n?---\s*$/, '')
  .trim();

console.log(body);
