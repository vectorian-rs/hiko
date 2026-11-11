// Exact-commit wall-clock gate. See docs/architecture/performance.md.
import { cpus, loadavg, platform, release, arch, homedir, tmpdir } from "node:os";
import { mkdir, mkdtemp, lstat, readFile, symlink, open, rename, rm } from "node:fs/promises";
import { resolve, join, dirname, basename, isAbsolute } from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { main, requireTools, root, toolchain } from "./common.ts";

export const corpus = ["benchmarks/compare/fib.hml", "benchmarks/compare/tak.hml", "benchmarks/local_rec.hml", "benchmarks/string_concat.hml"];
export const frozenInputs = [...corpus, "hiko.toml", "hiko.lock.toml", "policies/benchmark.policy.toml"];
export interface Invocation {
  argv: string[]; cwd: string; env: Record<string, string>; timeoutMs: number;
}
export interface Execution {
  status: number; stdout: string; stderr: string; durationNs: string; timedOut: boolean;
}
export interface GateIO {
  platform: string; cpuCount: number; machine: string;
  now(): bigint; load(): number; pause(ms: number): Promise<void>;
  exec(invocation: Invocation): Promise<Execution>;
  writeHistory(path: string, text: string): Promise<void>;
  tools: { git: string; cargo: string; rustc: string; pmset: string };
}
export async function atomicHistory(path: string, text: string) {
  await mkdir(dirname(path), { recursive: true });
  const temporary = path + "." + randomUUID() + ".tmp";
  const file = await open(temporary, "wx", 0o600);
  try { await file.writeFile(text); await file.sync(); } finally { await file.close(); }
  await rename(temporary, path);
  const directory = await open(dirname(path), "r");
  try { await directory.sync(); } finally { await directory.close(); }
}
export async function execute(invocation: Invocation): Promise<Execution> {
  if (!isAbsolute(invocation.cwd) || !Number.isFinite(invocation.timeoutMs) || invocation.timeoutMs <= 0) throw Error("invalid subprocess cwd/deadline");
  const started = process.hrtime.bigint();
  const child = Bun.spawn(invocation.argv, { cwd: invocation.cwd, env: invocation.env,
    stdin: "ignore", stdout: "pipe", stderr: "pipe", detached: true });
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    try { process.kill(-child.pid, "SIGKILL"); } catch { child.kill("SIGKILL"); }
  }, invocation.timeoutMs);
  try {
    const [out, err, exit] = await Promise.all([
      new Response(child.stdout).arrayBuffer(), new Response(child.stderr).arrayBuffer(),
      child.exited.then(status => ({ status, ended: process.hrtime.bigint() })),
    ]);
    return { status: exit.status, stdout: Buffer.from(out).toString("base64"), stderr: Buffer.from(err).toString("base64"),
      durationNs: (exit.ended - started).toString(), timedOut };
  } finally { clearTimeout(timer); }
}
export function systemIO(): GateIO {
  // pmset is required only where it is ever invoked; on other platforms
  // probe() refuses before the placeholder path could be executed.
  requireTools(["git", "cargo", "rustc", ...(platform() === "darwin" ? ["pmset"] : [])], "vm-gate.ts");
  const tool = (name: string) => Bun.which(name) ?? name;
  return { platform: platform(), cpuCount: cpus().length, machine: `${release()} ${arch()} ${cpus()[0]?.model || "unknown"}`,
    now: () => process.hrtime.bigint(), load: () => loadavg()[0]!, pause: ms => Bun.sleep(ms), exec: execute,
    writeHistory: atomicHistory, tools: { git: tool("git"), cargo: tool("cargo"), rustc: tool("rustc"), pmset: tool("pmset") } };
}
export function nanos(value: string): bigint {
  if (!/^[1-9][0-9]*$/.test(value)) throw Error("duration must be positive integer nanoseconds");
  return BigInt(value);
}
export function median(values: string[]): bigint {
  if (!values.length || values.length % 2 === 0) throw Error("need an odd, nonempty sample count");
  const ordered = values.map(nanos).sort((a, b) => a < b ? -1 : a > b ? 1 : 0);
  return ordered[Math.floor(ordered.length / 2)]!;
}
export function regresses(base: string[], candidate: string[]) {
  if (base.length !== candidate.length) throw Error("unbalanced sample pairs");
  return 100n * median(candidate) > 101n * median(base);
}
export function acPower(output: string): boolean {
  return /^Now drawing from 'AC Power'\r?\n/.test(output);
}
export function loadLimit(cpuCount: number) {
  if (!Number.isSafeInteger(cpuCount) || cpuCount < 1) throw Error("invalid CPU count");
  return Math.min(4, Math.max(1, cpuCount / 2));
}
export function validLoad(value: number, cpuCount: number) {
  return Number.isFinite(value) && value >= 0 && value < loadLimit(cpuCount);
}
const digest = (value: Uint8Array | string) => createHash("sha256").update(value).digest("hex");
const decoded = (value: string) => Buffer.from(value, "base64").toString("utf8");
function keys(value: unknown, expected: string[]): asserts value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).sort().join() !== [...expected].sort().join()) throw Error("heap-metrics: unexpected schema fields");
}
function uint(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) throw Error("heap-metrics: unsafe unsigned integer");
  return value;
}
// Artifact numbers are all unsigned integers. Reject lexical floats, unsafe
// u64s and duplicate object fields rather than accepting JSON.parse rounding.
function exactArtifactJSON(text: string): unknown {
  const value: unknown = JSON.parse(text);
  const stack: (Set<string> | null)[] = [];
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]!;
    if (ch === "{" || ch === "[") stack.push(ch === "{" ? new Set() : null);
    else if (ch === "}" || ch === "]") stack.pop();
    else if (ch === '"') {
      const start = i++;
      while (text[i] !== '"') { if (text[i] === "\\") i++; i++; }
      let next = i + 1; while (/\s/.test(text[next] || "x")) next++;
      if (text[next] === ":") {
        const key: string = JSON.parse(text.slice(start, i + 1)); const object = stack.at(-1);
        if (!object || object.has(key)) throw Error("heap-metrics: duplicate field");
        object.add(key);
      }
    } else if (ch === "-" || /[0-9]/.test(ch)) {
      const start = i;
      while (i + 1 < text.length && !/[\s,\]}:]/.test(text[i + 1]!)) i++;
      const token = text.slice(start, i + 1);
      if (!/^(0|[1-9][0-9]*)$/.test(token) || BigInt(token) > BigInt(Number.MAX_SAFE_INTEGER)) throw Error("heap-metrics: unsafe unsigned integer token");
    }
  }
  return value;
}
export function validateMetrics(text: string, nonce: string, profile: string) {
  const value = exactArtifactJSON(text);
  keys(value, ["schema_version", "nonce", "profile", "complete", "heaps_created", "heaps_dropped", "heaps_active", "successful_gc_object_allocations", "max_per_heap_estimated_retained_bytes", "heaps"]);
  if (!/^[A-Za-z0-9._-]{1,128}$/.test(nonce) || !/^[A-Za-z0-9._-]{1,128}$/.test(profile) || value.schema_version !== 1 || value.nonce !== nonce || value.profile !== profile || value.complete !== true) throw Error("heap-metrics: stale/incomplete identity");
  const created = uint(value.heaps_created);
  if (!created || created !== uint(value.heaps_dropped) || uint(value.heaps_active) !== 0 || !Array.isArray(value.heaps) || value.heaps.length !== created) throw Error("heap-metrics: unbalanced lifecycle");
  const ids = new Set<number>(); let total = 0; let peak = 0;
  for (const heap of value.heaps) {
    keys(heap, ["heap_id", "successful_gc_object_allocations", "estimated_retained_bytes_peak"]);
    const id = uint(heap.heap_id);
    if (!id || id > created || ids.has(id)) throw Error("heap-metrics: invalid heap id");
    ids.add(id); total += uint(heap.successful_gc_object_allocations); uint(total);
    peak = Math.max(peak, uint(heap.estimated_retained_bytes_peak));
  }
  if (uint(value.successful_gc_object_allocations) !== total || uint(value.max_per_heap_estimated_retained_bytes) !== peak) throw Error("heap-metrics: inconsistent aggregate");
  return value;
}

export interface GateOptions {
  repo: string;
  base: string;
  candidate: string;
  history: string;
  pairs?: number;
  environment?: Record<string, string | undefined>;
}
interface Build { binary: string; }
interface Transcript { stdoutArtifact?: string; stderrArtifact?: string; stdoutSha256?: string; stderrSha256?: string; }
interface CallRecord extends Invocation, Partial<Execution>, Transcript { error?: string; }
interface BuildRecord { key: string; sha: string; features: string[]; effectiveVmFeatures: string[]; binary: string; sha256: string; }
interface Comparison { mode: string; input: string; attempt: number; base: string[]; candidate: string[]; verdict: string; }
interface MetricRecord { name: string; mode: string; input: string; artifact: Record<string, unknown>; raw: string; }
export async function gate(options: GateOptions, io: GateIO = systemIO()) {
  const pairs = options.pairs ?? 11;
  if (!Number.isSafeInteger(pairs) || pairs < 11 || pairs % 2 === 0) {
    throw Error("--pairs must be an odd integer >=11");
  }
  const repo = resolve(options.repo);
  const destination = resolve(options.history);
  try {
    await lstat(destination);
    throw Error("history destination already exists; choose a fresh path to preserve previous evidence");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const workspace = await mkdtemp(join(tmpdir(), "hiko-vm-gate-"));
  const recovery = join(workspace, "history.json");
  const history = {
    schema_version: 1, started: new Date().toISOString(), machine: io.machine,
    platform: io.platform, cpus: io.cpuCount, pairs, recovery,
    refs: {} as Record<string, string>, inputsRef: "",
    inputs: [] as { path: string; sha256: string }[],
    tools: io.tools, toolchain: "", gateSha256: digest(await readFile(import.meta.path)),
    calls: [] as CallRecord[],
    probes: [] as { atNs: string; power: string; load: number }[],
    builds: [] as BuildRecord[], comparisons: [] as Comparison[], metrics: [] as MetricRecord[],
    errors: [] as string[], verdict: "incomplete", completed: "",
  };
  const worktrees: string[] = [];
  const sourceEnv = options.environment ?? process.env;
  const home = join(workspace, "home");
  const cargoHome = join(workspace, "cargo-home");
  const env: Record<string, string> = {
    PATH: sourceEnv.PATH ?? "", HOME: home, CARGO_HOME: cargoHome,
    // The host's rustup location is configuration, not an override; it is
    // recorded with every call and the toolchain itself stays pinned.
    RUSTUP_HOME: sourceEnv.RUSTUP_HOME ?? join(homedir(), ".rustup"), CARGO_BUILD_JOBS: "1",
    GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null", LC_ALL: "C", TZ: "UTC",
  };
  async function save() {
    // Keep recoverable evidence even if the requested destination is unwritable.
    const text = JSON.stringify(history, null, 2) + "\n";
    await atomicHistory(recovery, text);
    await io.writeHistory(destination, text);
  }
  async function publish(label: string) {
    history.completed = new Date().toISOString();
    try { await save(); return true; }
    catch (error) {
      history.errors.push(`${label} failed: ${String(error)}; evidence retained at ${recovery}`);
      history.verdict = "fail";
      await atomicHistory(recovery, JSON.stringify(history, null, 2) + "\n");
      return false;
    }
  }
  async function invoke(argv: string[], cwd: string, extra: Record<string, string> = {}, timeoutMs = 300_000) {
    const invocation = { argv, cwd, env: { ...env, ...extra }, timeoutMs };
    const entry: CallRecord = { ...invocation };
    history.calls.push(entry);
    let result: Execution;
    try { result = await io.exec(invocation); }
    catch (error) { entry.error = String(error); await save(); throw error; }
    Object.assign(entry, result);
    // Large compiler transcripts are immutable sidecars, not rewritten at every probe.
    for (const stream of ["stdout", "stderr"] as const) {
      if (result[stream].length > 65_536) {
        const path = join(destination + ".artifacts", basename(workspace), `${history.calls.length}.${stream}.b64`);
        await atomicHistory(path, result[stream]);
        entry[`${stream}Artifact`] = path;
        entry[`${stream}Sha256`] = digest(Buffer.from(result[stream], "base64"));
        entry[stream] = "";
      }
    }
    await save();
    if (result.timedOut || result.status !== 0) {
      // The complete transcript is already in the call record or its sidecar;
      // the error carries a bounded excerpt so history.errors stays small.
      const stderr = decoded(result.stderr);
      const brief = stderr.length > 2_000
        ? `${stderr.slice(0, 2_000)}… (${stderr.length} chars; full transcript in the call record)` : stderr;
      throw Error(`${argv[0]} failed (status=${result.status}, timeout=${result.timedOut}): ${brief}`);
    }
    return result;
  }
  async function phase(name: string, action: () => Promise<void>) {
    try { await action(); }
    catch (error) { history.errors.push(`${name}: ${String(error)}`); }
    await save();
  }
  async function probe(deadline?: bigint) {
    if (io.platform !== "darwin") throw Error("VM gate requires macOS AC power proof; no override");
    const remaining = deadline === undefined ? 5_000 : Number((deadline - io.now()) / 1_000_000n);
    if (remaining <= 0) throw Error("AC/load settling deadline exceeded");
    const result = await invoke([io.tools.pmset, "-g", "batt"], repo, {}, Math.min(5_000, remaining));
    const power = decoded(result.stdout);
    const load = io.load();
    history.probes.push({ atNs: io.now().toString(), power, load });
    await save();
    if (deadline !== undefined && io.now() > deadline) throw Error("AC/load settling deadline exceeded");
    if (!acPower(power)) throw Error("AC power unproven or changed; attempt invalid");
    if (!Number.isFinite(load) || load < 0) throw Error("invalid load probe");
    return validLoad(load, io.cpuCount);
  }
  async function settle() {
    const deadline = io.now() + 60_000_000_000n;
    for (;;) {
      if (await probe(deadline)) return;
      const remaining = Number((deadline - io.now()) / 1_000_000n);
      if (remaining <= 0) throw Error("load did not settle within 60s");
      await io.pause(Math.min(1_000, remaining));
    }
  }
  async function sample(build: Build, input: string, extra: Record<string, string> = {}) {
    if (!await probe()) throw Error("load exceeds bound; attempt invalid");
    const result = await invoke([
      build.binary, "run", "--config", join(workspace, "inputs/hiko.toml"),
      "--policy", "benchmark", "--workers", "1", join(workspace, "inputs", input),
    ], join(workspace, "inputs"), extra, 60_000);
    if (!await probe()) throw Error("load exceeds bound; attempt invalid");
    nanos(result.durationNs);
    return result;
  }
  try {
    await save();
    for (const [key, value] of Object.entries(sourceEnv)) {
      if (value && (/^(CARGO_|RUSTFLAGS$|RUSTDOCFLAGS$|RUSTC|RUSTUP_TOOLCHAIN$|HIKO_|ALLOW_BATTERY$|RUNS$)/.test(key))) {
        throw Error(`injected ${key} is forbidden; use the recorded isolated gate profiles`);
      }
    }
    await mkdir(home);
    await mkdir(cargoHome);
    // Share downloaded source caches, never ambient Cargo configuration/credentials.
    for (const cache of ["registry", "git"]) {
      const source = join(homedir(), ".cargo", cache);
      try { await lstat(source); } catch { continue; }
      await symlink(source, join(cargoHome, cache), "dir");
    }
    history.toolchain = decoded((await invoke([io.tools.rustc, `+${toolchain}`, "--version"], repo)).stdout);
    for (const [name, ref] of [["base", options.base], ["candidate", options.candidate]]) {
      const result = await invoke([io.tools.git, "rev-parse", "--verify", "--end-of-options", `${ref}^{commit}`], repo);
      const sha = decoded(result.stdout).trim();
      if (!/^[a-f0-9]{40}$/.test(sha)) throw Error("git did not resolve an immutable commit SHA");
      history.refs[name!] = sha;
    }
    // Inputs are frozen from the candidate commit, not the working tree, so
    // the recorded SHAs are measured with exactly the bytes they name.
    history.inputsRef = history.refs.candidate!;
    const listing = decoded((await invoke([io.tools.git, "ls-tree", "-r", "-z", history.inputsRef, "--", "benchmarks", "libraries", "policies", "hiko.toml", "hiko.lock.toml"], repo)).stdout);
    const files: string[] = [];
    for (const entry of listing.split("\0").filter(Boolean).sort((a, b) => a.slice(a.indexOf("\t") + 1) < b.slice(b.indexOf("\t") + 1) ? -1 : 1)) {
      const tab = entry.indexOf("\t");
      const [mode, kind] = entry.slice(0, tab).split(" ");
      const path = entry.slice(tab + 1);
      if (isAbsolute(path) || path.split(/[\\/]/).includes("..")) throw Error("unsafe frozen input path");
      if (!/^100(644|755)$/.test(mode!) || kind !== "blob") throw Error(`frozen inputs must be regular files: ${path}`);
      const bytes = Buffer.from((await invoke([io.tools.git, "cat-file", "blob", `${history.inputsRef}:${path}`], repo)).stdout, "base64");
      const output = join(workspace, "inputs", path);
      await mkdir(dirname(output), { recursive: true });
      await Bun.write(output, bytes);
      history.inputs.push({ path, sha256: digest(bytes) });
      files.push(path);
    }
    for (const required of frozenInputs) {
      if (!files.includes(required)) throw Error(`missing frozen input ${required}`);
    }
    // Cargo reads configuration from the cwd's ancestors: the shared workspace
    // chain is proven clean once, each fresh checkout's own tree at creation.
    for (let directory = workspace;; directory = dirname(directory)) {
      for (const config of [".cargo/config", ".cargo/config.toml"]) {
        if (await Bun.file(join(directory, config)).exists()) throw Error(`uncontrolled Cargo config: ${join(directory, config)}`);
      }
      if (dirname(directory) === directory) break;
    }
    // Profiles of the same commit share one checkout; isolation comes from the
    // per-profile target directory and verified effective features.
    const sources = new Map<string, string>();
    for (const name of ["base", "candidate"]) {
      await phase(`worktree ${name}`, async () => {
        await settle();
        const cwd = join(workspace, name);
        // Register before checkout so partial failures are also cleaned safely.
        worktrees.push(cwd);
        await invoke([io.tools.git, "-c", "core.hooksPath=/dev/null", "worktree", "add", "--detach", cwd, history.refs[name]!], repo);
        for (const config of [".cargo/config", ".cargo/config.toml"]) {
          if (await Bun.file(join(cwd, config)).exists()) throw Error(`uncontrolled Cargo config: ${join(cwd, config)}`);
        }
        sources.set(name, cwd);
      });
    }
    const builds = new Map<string, Build>();
    for (const name of ["base", "candidate"]) for (const mode of ["default", "register"]) {
      for (const metrics of [false, true]) {
        const profile = `${mode}${metrics ? ".metrics" : ""}`;
        const key = `${name}.${profile}`;
        await phase(`build ${key}`, async () => {
          const cwd = sources.get(name);
          if (!cwd) throw Error("source worktree unavailable");
          await settle();
          const target = join(workspace, "targets", key);
          const features = [...(mode === "register" ? ["register-vm"] : []), ...(metrics ? ["heap-metrics"] : [])];
          const argv = [io.tools.cargo, `+${toolchain}`, "build", "--release", "--locked", "--offline", "--message-format=json", "-p", "hiko-cli", "--target-dir", target];
          if (features.length) argv.push("--features", features.join(","));
          // Cold single-job release builds far exceed the default deadline.
          const compiled = await invoke(argv, cwd, {}, 3_600_000);
          const artifacts = decoded(compiled.stdout).trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
          const vm = artifacts.find(a => a.reason === "compiler-artifact" && a.target?.name === "hiko_vm");
          if (!vm || !Array.isArray(vm.features) || vm.features.includes("opcode-stats") ||
              vm.features.includes("heap-metrics") !== metrics ||
              vm.features.includes("register-vm") !== (mode === "register")) {
            throw Error("effective VM build features do not match the controlled counter-free/metrics profile");
          }
          const binary = join(target, "release/hiko-cli");
          const sha256 = digest(await readFile(binary));
          history.builds.push({ key, sha: history.refs[name]!, features, effectiveVmFeatures: vm.features, binary, sha256 });
          builds.set(key, { binary });
          await settle();
        });
      }
    }
    for (const mode of ["default", "register"]) for (const input of corpus) {
      const base = builds.get(`base.${mode}`);
      const candidate = builds.get(`candidate.${mode}`);
      let expected: string | undefined;
      await phase(`timing ${mode} ${input}`, async () => {
        if (!base || !candidate) throw Error("required timing build unavailable");
        const preflight = await sample(base, input);
        expected = preflight.stdout;
        if ((await sample(candidate, input)).stdout !== expected) throw Error("full stdout mismatch at preflight");
        for (let attempt = 0; attempt < 2; attempt++) {
          const row: Comparison = { mode, input, attempt: attempt + 1, base: [], candidate: [], verdict: "incomplete" };
          history.comparisons.push(row);
          for (let pair = 0; pair < pairs; pair++) {
            for (const name of pair % 2 ? ["candidate", "base"] : ["base", "candidate"]) {
              const result = await sample(name === "base" ? base : candidate, input);
              if (result.stdout !== expected) throw Error(`full stdout mismatch in attempt ${attempt + 1}, pair ${pair}, ${name}`);
              (name === "base" ? row.base : row.candidate).push(result.durationNs);
              await save();
            }
          }
          const regression = regresses(row.base, row.candidate);
          row.verdict = regression ? "regression" : attempt ? "pass-with-recheck" : "pass";
          await save();
          if (!regression) break;
          if (attempt === 1) throw Error("measured regression persists after one full recheck");
        }
      });
      for (const name of ["base", "candidate"]) await phase(`metrics ${name} ${mode} ${input}`, async () => {
        const build = builds.get(`${name}.${mode}.metrics`);
        if (!build) throw Error("unsupported/unavailable heap-metrics build; metrics incomplete, never zero");
        const nonce = randomUUID();
        const profile = `${mode}.metrics`;
        const path = join(workspace, `${name}-${mode}-${nonce}.json`);
        const result = await sample(build, input, {
          HIKO_HEAP_METRICS: path, HIKO_HEAP_METRICS_NONCE: nonce, HIKO_HEAP_METRICS_PROFILE: profile,
        });
        if (expected === undefined || result.stdout !== expected) throw Error("metrics stdout differs or timing preflight incomplete");
        const text = await readFile(path, "utf8");
        const artifact = validateMetrics(text, nonce, profile);
        history.metrics.push({ name, mode, input, artifact, raw: text });
      });
    }
    history.verdict = history.errors.length ? "fail" : "pass";
  } catch (error) {
    history.errors.push(String(error));
    history.verdict = "fail";
  } finally {
    if (await publish("history publication")) {
      let cleaned = true;
      for (const cwd of worktrees.reverse()) {
        try { await invoke([io.tools.git, "worktree", "remove", "--force", cwd], repo); }
        catch (error) {
          history.errors.push(`owned worktree cleanup: ${String(error)}`);
          history.verdict = "fail";
          cleaned = false;
        }
      }
      if (!await publish("final history publication")) cleaned = false;
      if (cleaned) await rm(workspace, { recursive: true, force: true });
    }
  }
  return history;
}

if (import.meta.main) await main(async () => {
  const args = process.argv.slice(2);
  if (args.length < 2 || args[0]!.startsWith("-") || args[1]!.startsWith("-")) {
    throw Error("Usage: bun scripts/vm-gate.ts <base> <candidate> [--history path] [--pairs 11]");
  }
  // Evidence must survive cargo clean, so the default lives outside target/.
  const options: GateOptions = { repo: root, base: args[0]!, candidate: args[1]!, history: join(root, "vm-gate-history", randomUUID() + ".json") };
  const seen = new Set<string>();
  for (let i = 2; i < args.length; i += 2) {
    const flag = args[i]!;
    if (seen.has(flag) || args[i + 1] === undefined) throw Error("duplicate/missing VM gate option");
    seen.add(flag);
    if (flag === "--history") options.history = args[i + 1]!;
    else if (flag === "--pairs") options.pairs = Number(args[i + 1]);
    else throw Error(`unknown VM gate option ${flag}`);
  }
  const report = await gate(options);
  console.log(`${report.verdict.toUpperCase()}: ${resolve(options.history)}`);
  if (report.verdict !== "pass") {
    for (const error of report.errors) console.error(error);
    if (await Bun.file(report.recovery).exists()) console.error(`Recovery evidence: ${report.recovery}`);
    process.exitCode = 1;
  }
});
