// Run the host-path Rust unit tests without the Windows SDK. The WASI module
// sees only the OS temporary directory and receives no host environment.
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { WASI } from 'node:wasi';

const [wasmPath, ...testArgs] = process.argv.slice(2);
if (!wasmPath) throw new Error('pass a compiled WASI Rust test module');
const wasi = new WASI({ version: 'preview1', args: ['worker-root-tests', ...testArgs],
  env: {}, preopens: { '/tmp': tmpdir() }, returnOnExit: true });
const module = new WebAssembly.Module(readFileSync(wasmPath));
const instance = new WebAssembly.Instance(module,
  { wasi_snapshot_preview1: wasi.wasiImport });
process.exitCode = wasi.start(instance);
