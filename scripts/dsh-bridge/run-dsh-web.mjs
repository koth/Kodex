#!/usr/bin/env node
// Run the built `dsh web` binary directly and report its bound endpoint.
//
// Spawns `node apps/cli/lib/bin.js web` (the `dsh web` profile) with a
// loopback bind and, by default, `--port 0` so the OS picks a free port. The
// web profile prints a readiness line on stdout the moment the server is
// listening:
//
//     dsh web: http://127.0.0.1:<port>
//
// This script captures that line, prints the resolved endpoint as a machine-
// readable `DSH_ENDPOINT=<url>` line on its own stdout, then streams the
// process's stdout/stderr until it exits or is signaled. The endpoint is also
// written to the file named by --endpoint-file (default: none) so a caller
// (e.g. the dsh-bridge integration tests, or Kodex's managed-spawn path) can
// pick it up without parsing logs.
//
// Usage:
//   node scripts/dsh-bridge/run-dsh-web.mjs                      # --port 0, print endpoint
//   node scripts/dsh-bridge/run-dsh-web.mjs --port 3080          # fixed port
//   node scripts/dsh-bridge/run-dsh-web.mjs --endpoint-file ./dsh.url
//   node scripts/dsh-bridge/run-dsh-web.mjs --host 127.0.0.1 --port 0
//
// Environment:
//   DEEPSEEK_API_KEY  REQUIRED for the agent to talk to the model. The web
//                     profile refuses to start a session without it; this
//                     script forwards the parent env verbatim, so export it
//                     in your shell before running.
//   DSH_HOME          Optional. Defaults to a fresh temp dir so profiles are
//                     isolated per run; set it to share state across runs.
//   DSH_TELEMETRY_DISABLED  Set to any non-empty value to disable OTel.
//
// Signal handling: SIGINT/SIGTERM are forwarded to the child; the child's own
// shutdown controller disposes the cordis tree and exits 0/130 accordingly.

import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..', '..');
const dshRoot = join(repoRoot, 'deepseek-harness');
const cliBin = join(dshRoot, 'apps', 'cli', 'lib', 'bin.js');

function fail(msg) {
  console.error(`run-dsh-web: ${msg}`);
  process.exit(1);
}

function parseArgs(argv) {
  const out = { port: '0', host: undefined, endpointFile: undefined, extra: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--port') out.port = argv[++i];
    else if (a === '--host') out.host = argv[++i];
    else if (a === '--endpoint-file') out.endpointFile = argv[++i];
    else if (a === '--help' || a === '-h') {
      printHelp();
      process.exit(0);
    } else out.extra.push(a);
  }
  return out;
}

function printHelp() {
  console.log(`run-dsh-web: run the built dsh web binary and report its endpoint.

Usage:
  node scripts/dsh-bridge/run-dsh-web.mjs [options] [extra dsh web args]

Options:
  --port <n>            listen port (default: 0 = OS-assigned)
  --host <host>         bind host (default: 127.0.0.1; 0.0.0.0 refused by dsh)
  --endpoint-file <p>   write the resolved endpoint URL to this file
  -h, --help            show this help

Environment:
  DEEPSEEK_API_KEY      required (model access)
  DSH_HOME              optional (default: fresh temp dir)
  DSH_TELEMETRY_DISABLED  optional (any non-empty value disables OTel)`);
}

// Parse first so --help works even before the binary is built.
const opts = parseArgs(process.argv.slice(2));

if (!existsSync(cliBin)) {
  fail(`dsh CLI binary not found at ${cliBin}.
Run the build first:
  node scripts/dsh-bridge/build-dsh-web.mjs`);
}

// Isolated DSH_HOME by default so each run gets a clean profiles tree; a
// caller can override by exporting DSH_HOME in the parent env.
const env = { ...process.env };
if (!env.DSH_HOME) {
  env.DSH_HOME = mkdtempSync(join(tmpdir(), 'dsh-web-home-'));
}
// Telemetry off unless the caller explicitly opted in by leaving it unset with
// intent — simplest is to default-disable for a local dev/test runner.
if (!env.DSH_TELEMETRY_DISABLED) env.DSH_TELEMETRY_DISABLED = '1';

if (!env.DEEPSEEK_API_KEY) {
  console.error('run-dsh-web: WARNING — DEEPSEEK_API_KEY is not set; the web UI will boot but sessions will fail to reach the model.');
}

const webArgs = ['web', '--port', String(opts.port)];
if (opts.host) webArgs.push('--host', opts.host);
webArgs.push(...opts.extra);

console.error(`run-dsh-web: spawning node "${cliBin}" ${webArgs.join(' ')}`);
console.error(`run-dsh-web: DSH_HOME=${env.DSH_HOME}`);

const child = spawn(process.execPath, [cliBin, ...webArgs], {
  cwd: dshRoot,
  env,
  stdio: ['ignore', 'pipe', 'pipe'],
});

let endpoint = null;
let readinessBuffer = '';
let exited = false;

function emitEndpoint(url) {
  if (endpoint) return;
  endpoint = url;
  // Machine-readable line on the runner's own stdout.
  process.stdout.write(`DSH_ENDPOINT=${url}\n`);
  if (opts.endpointFile) {
    mkdirSync(dirname(resolve(opts.endpointFile)), { recursive: true });
    writeFileSync(opts.endpointFile, url, 'utf8');
  }
  console.error(`run-dsh-web: ready at ${url}`);
}

// Parse the readiness line out of the child's stdout. The web profile prints
// `dsh web: http://127.0.0.1:<port>` once the server is listening; until then
// we buffer per-line and forward everything to stderr so the caller still
// sees the dsh log stream.
function handleChunk(chunk) {
  const str = chunk.toString('utf8');
  if (endpoint) {
    process.stderr.write(str);
    return;
  }
  readinessBuffer += str;
  let nl;
  while ((nl = readinessBuffer.indexOf('\n')) !== -1) {
    const line = readinessBuffer.slice(0, nl);
    readinessBuffer = readinessBuffer.slice(nl + 1);
    const m = line.match(/dsh web:\s+(https?:\/\/[^\s]+)/);
    if (m) emitEndpoint(m[1]);
    process.stderr.write(line + '\n');
  }
  // Keep any trailing partial line in the buffer for the next chunk.
}

child.stdout.on('data', handleChunk);
child.stderr.on('data', (c) => process.stderr.write(c));

function shutdown(signal) {
  if (exited) return;
  exited = true;
  try { child.kill(signal); } catch { /* already gone */ }
}
process.on('SIGINT', () => shutdown('SIGINT'));
process.on('SIGTERM', () => shutdown('SIGTERM'));

child.on('exit', (code, signal) => {
  // Flush any remaining buffered stdout.
  if (readinessBuffer) process.stderr.write(readinessBuffer);
  if (!endpoint) {
    console.error('run-dsh-web: process exited before the readiness line was printed.');
  }
  console.error(`run-dsh-web: child exited (code=${code}, signal=${signal ?? 'none'}).`);
  process.exit(code ?? (signal ? 130 : 0));
});
