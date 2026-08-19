#!/usr/bin/env node
// Build the DeepSeek Harness `dsh web` binary so it can be run directly later.
//
// What this produces:
//   - `deepseek-harness/apps/cli/lib/bin.js`  — the bundled dsh CLI entry
//     (run with `node apps/cli/lib/bin.js web`, aka `dsh web`).
//   - `deepseek-harness/packages/bundle/web-app/lib/*.js` — the web-app bundle.
//   - `deepseek-harness/apps/web/dist/` — the built browser frontend served
//     by the web profile (`@deepseek-ai/dsh-web-frontend/dist/index.html`).
//
// The build mirrors the dsh repo's own `pnpm run build`:
//   1. `pnpm install`      — link the workspace (first run only; cached after).
//   2. `pnpm run build:lib` — `tsc -b` (host + client faces) then `tsdown` bundle.
//   3. `pnpm run build:web` — `vite build` of the browser frontend (apps/web).
//
// Requirements: Node >= 22.19, pnpm >= 10 (the repo pins pnpm@11.7.0 via
// packageManager; corepack will fetch the right version if `pnpm` is absent).
//
// Usage:
//   node scripts/dsh-bridge/build-dsh-web.mjs            # full build
//   node scripts/dsh-bridge/build-dsh-web.mjs --no-install # skip pnpm install
//   node scripts/dsh-bridge/build-dsh-web.mjs --lib-only   # skip the web/vite pass
//   node scripts/dsh-bridge/build-dsh-web.mjs --web-only   # skip the lib pass
//
// Re-running is safe: every step is idempotent and overwrites prior output.

import { spawnSync } from 'node:child_process';
import { existsSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..', '..');
const dshRoot = join(repoRoot, 'deepseek-harness');
const cliBin = join(dshRoot, 'apps', 'cli', 'lib', 'bin.js');
const webDist = join(dshRoot, 'apps', 'web', 'dist', 'index.html');

const args = new Set(process.argv.slice(2));
const skipInstall = args.has('--no-install');
const libOnly = args.has('--lib-only');
const webOnly = args.has('--web-only');

function fail(msg) {
  console.error(`build-dsh-web: ${msg}`);
  process.exit(1);
}

function run(cmd, cmdArgs, opts = {}) {
  console.error(`build-dsh-web: ${cmd} ${cmdArgs.join(' ')}`);
  const result = spawnSync(cmd, cmdArgs, {
    cwd: dshRoot,
    stdio: 'inherit',
    shell: process.platform === 'win32',
    ...opts,
  });
  if (result.status !== 0) {
    fail(`command failed (exit ${result.status}): ${cmd} ${cmdArgs.join(' ')}`);
  }
}

function resolvePnpm() {
  // Prefer a real pnpm on PATH; fall back to npx pnpm (corepack-managed).
  const probe = spawnSync(process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm', ['--version'], {
    stdio: 'ignore',
    shell: process.platform === 'win32',
  });
  if (probe.status === 0) return process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm';
  console.error('build-dsh-web: pnpm not found on PATH; using npx pnpm (corepack will fetch the pinned version).');
  return process.platform === 'win32' ? 'npx.cmd' : 'npx';
}

if (!existsSync(join(dshRoot, 'package.json'))) {
  fail(`expected dsh repo at ${dshRoot} (deepseek-harness/package.json not found)`);
}

const pnpm = resolvePnpm();
const pnpmArgs = pnpm.endsWith('pnpm') || pnpm === 'pnpm.cmd' ? [] : ['pnpm'];

if (!skipInstall) {
  run(pnpm, [...pnpmArgs, 'install', '--frozen-lockfile']);
} else {
  console.error('build-dsh-web: skipping pnpm install (--no-install)');
}

if (!webOnly) {
  // build:lib = build:lib:host (tsc -b tsconfig.host.json + tsdown host face)
  //          + build:lib:client (tsc -b tsconfig.client.json + tsdown client face)
  run(pnpm, [...pnpmArgs, 'run', 'build:lib']);
}

if (!libOnly) {
  // build:web = vite build of @deepseek-ai/dsh-web-frontend (apps/web -> dist/)
  run(pnpm, [...pnpmArgs, 'run', 'build:web']);
}

// Verify the artifacts the runner needs exist.
if (!existsSync(cliBin)) {
  fail(`build produced no CLI binary at ${cliBin}`);
}
if (!libOnly && !existsSync(webDist)) {
  fail(`build produced no web frontend dist at ${webDist}`);
}

console.error('build-dsh-web: done.');
console.error(`  CLI binary: ${cliBin}`);
if (!libOnly) console.error(`  web dist:   ${webDist}`);
console.error('');
console.error('Run it with:');
console.error(`  node scripts/dsh-bridge/run-dsh-web.mjs                # 127.0.0.1:3080, OS port with --port 0`);
console.error(`  node "${cliBin}" web --port 0                          # direct, OS-assigned port`);
