import { afterEach, expect, test } from "bun:test";
import { chmod, copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { comparisonRow, lastLine } from "./compare.ts";
import { root, toolchain } from "./common.ts";
import { acPower, loadLimit, validLoad, median, regresses, validateMetrics, gate, execute, frozenInputs, type GateIO } from "./vm-gate.ts";

const temporary: string[] = [];
afterEach(async () => {
  await Promise.all(temporary.splice(0).map((path) => rm(path, { recursive: true, force: true })));
});

// Test executables are TypeScript run directly by Bun, including the cargo and
// hyperfine stand-ins. No shell is involved in these subprocess fixtures.
const toolSource = `#!${process.execPath}
import { appendFile, chmod, mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
const raw = basename(process.argv[1]);
const tool = raw.startsWith("hiko") ? "hiko" : raw;
const args = process.argv.slice(2);
await appendFile(process.env.TOOL_LOG, JSON.stringify({ tool, args, cwd: process.cwd() }) + "\\n");
if (tool === process.env.FAIL_TOOL && args.includes(process.env.FAIL_ARG)) process.exit(37);
if (tool === "cargo" && args.includes("build")) {
  const binary = join(process.cwd(), "target/release/hiko-cli");
  await mkdir(dirname(binary), { recursive: true });
  await writeFile(binary, await readFile(process.argv[1], "utf8"));
  await chmod(binary, 0o755);
} else if (tool === "cargo" && args.includes("process_creation_cost")) console.log("created a VM");
else if (tool === "git") console.log(args.includes("--git-common-dir") ? process.env.COMMON_GIT : "abc1234");
else if (tool === "pmset") console.log(process.env.BATTERY ? "Now drawing from 'Battery Power'" : "Now drawing from 'AC Power'");
else if (tool === "hyperfine") {
  const md = args.indexOf("--export-markdown");
  if (md >= 0) await writeFile(args[md + 1], "| Command | Mean |\\n|---|---|\\n| fixture | 2 ms |\\n");
  const json = args.indexOf("--export-json");
  if (json >= 0) {
    const means = [0.002, 0.004, 0.008, 0.001];
    await writeFile(args[json + 1], JSON.stringify({results: args.slice(json + 2).map((_, i) => ({mean: means[i]}))}));
  }
} else if ((tool === "lua" || tool === "luajit") && args.includes("-v")) console.error(tool === "lua" ? "Lua 5.5.0" : "LuaJIT 2.1.0");
else if (tool === "python3" && args.includes("--version")) console.log("Python 3.13.0");
else if (tool === "python3" && args.includes("-c")) process.exit(process.env.NO_BLAKE3 ? 1 : 0);
else if (["hiko", "lua", "luajit", "python3"].includes(tool)) console.log(tool === process.env.BAD_CHECKSUM ? "42 " : "42");
`;

interface Call { tool: string; args: string[]; cwd: string }

async function fixture(options: { jit?: boolean; worktree?: boolean } = {}) {
  const temp = await realpath(await mkdtemp(join(tmpdir(), "hiko-tooling-")));
  temporary.push(temp);
  const repo = join(temp, "repo with spaces");
  const bin = join(temp, "tools with spaces");
  const commonGit = join(temp, "common.git");
  for (const directory of ["scripts", "benchmarks/compare", "examples"]) {
    await mkdir(join(repo, directory), { recursive: true });
  }
  await mkdir(bin);
  await mkdir(join(temp, "outside"));
  await mkdir(join(commonGit, "objects"), { recursive: true });
  await writeFile(join(commonGit, "objects/fixture"), "git object");
  if (options.worktree) await writeFile(join(repo, ".git"), `gitdir: ${commonGit}\n`);
  else await mkdir(join(repo, ".git"));
  for (const file of ["common.ts", "lint.ts", "bench.ts", "compare.ts"]) {
    await copyFile(join(import.meta.dir, file), join(repo, "scripts", file));
  }
  for (const file of ["benchmarks/fanout_hash.hml", "examples/benchmark.hml", "examples/benchmark.py"]) {
    await writeFile(join(repo, file), 'root = ".git"\n');
  }
  for (const name of ["fib", "tak"]) await writeFile(join(repo, `benchmarks/compare/${name}.hml`), "fixture\n");
  const hiko = "hiko cli's fixture";
  for (const name of ["cargo", "git", "hyperfine", "lua", "python3", "pmset", hiko,
    ...(options.jit === false ? [] : ["luajit"])]) {
    await writeFile(join(bin, name), toolSource);
    await chmod(join(bin, name), 0o755);
  }
  const log = join(temp, "calls.jsonl");
  return {
    repo, bin, log,
    async calls(): Promise<Call[]> {
      return existsSync(log) ? (await readFile(log, "utf8")).trim().split("\n").filter(Boolean).map((line) => JSON.parse(line)) : [];
    },
    async run(script: string, args: string[] = [], env: Record<string, string> = {}) {
      const child = Bun.spawn([process.execPath, join(repo, "scripts", script), ...args], {
        cwd: join(temp, "outside"), stdout: "pipe", stderr: "pipe",
        env: { ...process.env, PATH: bin, TOOL_LOG: log, COMMON_GIT: commonGit, RUNS: "2",
          HIKO_BIN: join(bin, hiko), ALLOW_BATTERY: "", RUST_TOOLCHAIN: "", ...env },
      });
      const [stdout, stderr, code] = await Promise.all([
        new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
      ]);
      return { stdout, stderr, code };
    },
  };
}

test("lint runs all feature profiles from the script's repository", async () => {
  const f = await fixture();
  expect((await f.run("lint.ts")).code).toBe(0);
  const calls = await f.calls();
  expect(calls.map((c) => c.cwd)).toEqual(Array(4).fill(f.repo));
  expect(calls.map((c) => c.args)).toEqual([
    ["+1.99.1", "fmt", "--all", "--", "--check"],
    ["+1.99.1", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
    ["+1.99.1", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"],
    ["+1.99.1", "clippy", "--workspace", "--all-targets", "--no-default-features", "--", "-D", "warnings"],
  ]);
});

test("lint honors the toolchain override and stops at the failing child", async () => {
  const f = await fixture();
  const result = await f.run("lint.ts", [], { RUST_TOOLCHAIN: "custom", FAIL_TOOL: "cargo", FAIL_ARG: "clippy" });
  expect(result.code).toBe(37);
  const calls = await f.calls();
  expect(calls.length).toBe(2);
  expect(calls.every((c) => c.args[0] === "+custom")).toBe(true);
});

test("bench preserves groups, workers, custom binary paths, and report output", async () => {
  const f = await fixture();
  const result = await f.run("bench.ts", ["target/report with spaces.md"]);
  expect(result.stderr).toContain("wrote ");
  expect(result.code).toBe(0);
  const calls = await f.calls();
  const hyperfine = calls.filter((c) => c.tool === "hyperfine");
  expect(hyperfine.length).toBe(12);
  expect(hyperfine.every((c) => c.args.includes("-N") && c.args.includes("2"))).toBe(true);
  expect(hyperfine[0]!.args.at(-1)).toContain("list_length.hml");
  expect(hyperfine[0]!.args.at(-1)).toContain("'\\''");
  expect(hyperfine[1]!.args).toContain("1,8");
  expect(hyperfine.some((c) => c.args.includes("1,2,4,8"))).toBe(true);
  expect(calls.some((c) => c.args.includes("build"))).toBe(false);
  expect(await readFile(join(f.repo, "target/report with spaces.md"), "utf8")).toContain("## Process creation");
  expect(existsSync(dirname(hyperfine[0]!.args[hyperfine[0]!.args.indexOf("--export-markdown") + 1]!))).toBe(false);
});

test("bench handles a worktree, the full build, and optional Python blake3", async () => {
  const f = await fixture({ worktree: true });
  const result = await f.run("bench.ts", [], { HIKO_BIN: "", NO_BLAKE3: "1" });
  expect(result.code).toBe(0);
  expect(result.stderr).toContain("skipping the walk benchmark");
  const calls = await f.calls();
  expect(calls.find((c) => c.tool === "cargo" && c.args.includes("build"))!.args).toContain("builtin-all");
  expect(calls.filter((c) => c.tool === "hyperfine").length).toBe(11);
  expect(await readFile(join(f.repo, "target/bench-git/objects/fixture"), "utf8")).toBe("git object");
  expect(await readFile(join(f.repo, "target/bench-fanout_hash.hml"), "utf8")).toContain('"target/bench-git"');
});

test("compare checks every implementation and writes the means and ratios", async () => {
  const f = await fixture();
  const result = await f.run("compare.ts", ["target/compare report.md", "fib"]);
  expect(result.code).toBe(0);
  const report = await readFile(join(f.repo, "target/compare report.md"), "utf8");
  expect(report).toContain("Lua 5.5.0; Python 3.13.0");
  expect(report).toContain("| `fib` | 2 ms | 4 ms | 8 ms | 1 ms | 0.5x | 0.3x |");
  const calls = await f.calls();
  expect(calls.filter((c) => c.tool !== "hyperfine" && c.args.some((a) =>
    a.endsWith("/fib.lua") || a.endsWith("/fib.py") || a.endsWith("/fib.hml"))).length).toBe(5);
});

test("compare rejects differing checksum whitespace before measurement", async () => {
  const f = await fixture();
  const result = await f.run("compare.ts", ["target/compare.md", "fib"], { BAD_CHECKSUM: "lua" });
  expect(result.code).toBe(1);
  expect(result.stderr).toContain("printed '42 ', Hiko printed '42'");
  expect((await f.calls()).some((c) => c.tool === "hyperfine")).toBe(false);
});

test("compare refuses battery measurements and honors the explicit override", async () => {
  const f = await fixture();
  expect((await f.run("compare.ts", [], { BATTERY: "1" })).code).toBe(2);
  expect((await f.calls()).some((c) => c.tool === "hyperfine")).toBe(false);
  expect((await f.run("compare.ts", ["target/override.md", "fib"], { BATTERY: "1", ALLOW_BATTERY: "1" })).code).toBe(0);
});

test("compare discovers benchmarks without LuaJIT and builds the default CLI", async () => {
  const f = await fixture({ jit: false });
  const result = await f.run("compare.ts", [], { HIKO_BIN: "" });
  expect(result.code).toBe(0);
  const calls = await f.calls();
  const build = calls.find((c) => c.tool === "cargo" && c.args.includes("build"))!;
  expect(build.args).not.toContain("builtin-all");
  const report = await readFile(join(f.repo, "target/compare.md"), "utf8");
  expect(report).toContain("| `fib` | 2 ms | 4 ms | 8 ms | - |");
  expect(report).toContain("| `tak` |");
});

test("benchmark failures propagate their status and remove temporary reports", async () => {
  const f = await fixture();
  const result = await f.run("compare.ts", ["target/failed.md", "fib"], { FAIL_TOOL: "hyperfine", FAIL_ARG: "--export-json" });
  expect(result.code).toBe(37);
  const invocation = (await f.calls()).find((c) => c.tool === "hyperfine")!;
  expect(existsSync(dirname(invocation.args[invocation.args.indexOf("--export-json") + 1]!))).toBe(false);
});

test("last-line checks retain whitespace and trailing blank lines", () => {
  expect(lastLine("first\n42\n")).toBe("42");
  expect(lastLine("42 \n")).toBe("42 ");
  expect(lastLine("42\n\n")).toBe("");
  expect(lastLine("42")).toBe("42");
  expect(lastLine("")).toBe("");
  expect(() => comparisonRow("bad", [0, 1, 2])).toThrow("invalid hyperfine results");
});

test("pinned Rust toolchain stays consistent across scripts, workflows and docs", async () => {
  for (const file of [".github/workflows/test.yml", ".github/workflows/lint.yml", "docs/project/dev-standard.md"]) {
    expect(await readFile(join(root, file), "utf8")).toContain(toolchain);
  }
});

test("VM gate uses exact positive integer medians and strict one-percent comparison", () => {
  expect(median(["999", "100", "101"])).toBe(101n);
  expect(regresses(Array(11).fill("100"), Array(11).fill("101"))).toBe(false);
  expect(regresses(Array(11).fill("10000"), Array(11).fill("10101"))).toBe(true);
  expect(regresses(Array(11).fill("900719925474099300"), Array(11).fill("909727124728840293"))).toBe(false);
  for (const invalid of ["0", "-1", "NaN", "1.5", "1e3"]) expect(() => median([invalid])).toThrow();
  expect(() => median(["1", "2"])).toThrow();
});

test("VM gate fails closed on unrecognized AC and invalid/equal load", () => {
  expect(acPower("Now drawing from 'AC Power'\n -InternalBattery-0\n")).toBe(true);
  for (const invalid of ["", "AC Power", "Now drawing from 'Battery Power'\n", "garbage AC Power\n"]) expect(acPower(invalid)).toBe(false);
  expect(loadLimit(2)).toBe(1); expect(loadLimit(10)).toBe(4);
  for (const invalid of [NaN, Infinity, -1, 4]) expect(validLoad(invalid, 10)).toBe(false);
  expect(validLoad(3.999, 10)).toBe(true); expect(() => loadLimit(0)).toThrow();
});

function metricFixture() {
  return { schema_version: 1, nonce: "fresh", profile: "default.metrics", complete: true,
    heaps_created: 1, heaps_dropped: 1, heaps_active: 0, successful_gc_object_allocations: 3,
    max_per_heap_estimated_retained_bytes: 64,
    heaps: [{ heap_id: 1, successful_gc_object_allocations: 3, estimated_retained_bytes_peak: 64 }] };
}
test("VM gate validates exporter schema, identity, lifecycle and lossless safe integer totals", () => {
  const artifact = metricFixture();
  expect(validateMetrics(JSON.stringify(artifact), "fresh", "default.metrics")).toEqual(artifact);
  const corrupt = [ { ...artifact, complete: false }, { ...artifact, nonce: "stale" },
    { ...artifact, schema_version: 2 }, { ...artifact, heaps_active: 1 }, { ...artifact, heaps_dropped: 0 },
    { ...artifact, successful_gc_object_allocations: 4 }, { ...artifact, max_per_heap_estimated_retained_bytes: 63 },
    { ...artifact, extra: true }, { ...artifact, heaps: [] },
    { ...artifact, heaps: [{ ...artifact.heaps[0], heap_id: 0 }] },
    { ...artifact, heaps: [{ ...artifact.heaps[0], successful_gc_object_allocations: Number.MAX_SAFE_INTEGER + 1 }] } ];
  for (const value of corrupt) expect(() => validateMetrics(JSON.stringify(value), "fresh", "default.metrics")).toThrow();
  const text = JSON.stringify(artifact);
  expect(() => validateMetrics(text.replace('"schema_version":1', '"schema_version":1.0'), "fresh", "default.metrics")).toThrow();
  expect(() => validateMetrics(text.replace('"schema_version":1', '"schema_version":1,"schema_version":1'), "fresh", "default.metrics")).toThrow("duplicate");
  expect(() => validateMetrics("broken", "fresh", "default.metrics")).toThrow();
});

interface GateFixtureOptions {
  power?: string; load?: number; platform?: string; regression?: "once" | "always";
  mismatch?: boolean; exportExit?: boolean; badMetrics?: boolean; buildFailure?: boolean;
  historyFailure?: boolean; historyAfter?: number; powerSwitch?: boolean; loadSwitch?: boolean;
  hungProbe?: boolean; badFeatures?: boolean; largeTranscript?: boolean;
}
async function gateFixture(options: GateFixtureOptions = {}) {
  const directory = await mkdtemp(join(tmpdir(), "hiko-gate-test-"));
  temporary.push(directory);
  const repo = join(directory, "repo with spaces");
  const history = join(directory, "report.json");
  const files = frozenInputs;
  let clock = 1n; let publications = 0; let sampleCalls = 0;
  const calls: Parameters<GateIO["exec"]>[0][] = [];
  const counts = new Map<string, number>();
  const encoded = (text: string) => Buffer.from(text).toString("base64");
  const io: GateIO = {
    platform: options.platform ?? "darwin", cpuCount: 10, machine: "fixture",
    now: () => clock, load: () => options.loadSwitch && sampleCalls >= 4 ? 4 : options.load ?? 0,
    pause: async ms => { clock += BigInt(ms) * 1_000_000n; },
    tools: { git: "fixture-git", cargo: "fixture-cargo", rustc: "fixture-rustc", pmset: "fixture-pmset" },
    writeHistory: async (path, text) => {
      if (options.historyFailure || ++publications > (options.historyAfter ?? Infinity)) throw Error("injected history failure");
      await writeFile(path, text);
    },
    exec: async invocation => {
      calls.push(invocation); clock += 1_000_000n;
      const [tool, ...args] = invocation.argv;
      let stdout = ""; let stderr = ""; let status = 0; let durationNs = "10000"; let timedOut = false;
      if (tool === "fixture-rustc") stdout = "rustc1.99.1";
      else if (tool === "fixture-pmset") {
        stdout = options.powerSwitch && sampleCalls >= 4 ? "Now drawing from 'Battery Power'\n" : options.power ?? "Now drawing from 'AC Power'\n";
        if (options.hungProbe) { clock += BigInt(invocation.timeoutMs) * 1_000_000n; timedOut = true; }
      } else if (tool === "fixture-git") {
        if (args[0] === "rev-parse") stdout = (args.at(-1)!.startsWith("base") ? "a" : "b").repeat(40) + "\n";
        else if (args[0] === "ls-tree") stdout = files.map(f => `100644 blob ${"c".repeat(40)}\t${f}`).join("\0") + "\0";
        else if (args[0] === "cat-file") stdout = "frozen fixture\n";
        else if (args.includes("add")) await mkdir(args.at(-2)!, { recursive: true });
      } else if (tool === "fixture-cargo") {
        const target = args[args.indexOf("--target-dir") + 1]!;
        await mkdir(join(target, "release"), { recursive: true });
        await writeFile(join(target, "release/hiko-cli"), invocation.cwd);
        const featureIndex = args.indexOf("--features");
        const features = featureIndex < 0 ? [] : args[featureIndex + 1]!.split(",");
        if (options.badFeatures) features.push("opcode-stats");
        stdout = JSON.stringify({ reason: "compiler-artifact", target: { name: "hiko_vm" }, features });
        if (options.largeTranscript) stderr = "x".repeat(70_000);
        if (options.buildFailure && target.endsWith("base.default.metrics")) status = 37;
      } else {
        sampleCalls++;
        const name = invocation.argv[0]!.includes("/candidate.") ? "candidate" : "base";
        const key = `${invocation.argv[0]}:${args.at(-1)}`;
        const count = counts.get(key) ?? 0; counts.set(key, count + 1);
        stdout = options.mismatch && name === "candidate" && count === 2 ? "different\n42\n" : "full output\n42\n";
        if (name === "candidate" && options.regression && (options.regression === "always" || count <= 11)) durationNs = "11000";
        if (invocation.env.HIKO_HEAP_METRICS) {
          const artifact = { ...metricFixture(), nonce: invocation.env.HIKO_HEAP_METRICS_NONCE, profile: invocation.env.HIKO_HEAP_METRICS_PROFILE };
          await writeFile(invocation.env.HIKO_HEAP_METRICS, options.badMetrics ? "{}" : JSON.stringify(artifact));
          if (options.exportExit) status = 38;
        }
      }
      return { status, stdout: encoded(stdout), stderr: encoded(stderr), durationNs, timedOut };
    },
  };
  return { repo, history, io, calls, async run(environment: Record<string, string> = {}) {
    const report = await gate({ repo, history, base: "base", candidate: "candidate", environment }, io);
    // Publication failures intentionally retain evidence; remove only this test's owned directory.
    if (existsSync(report.recovery)) temporary.push(dirname(report.recovery));
    return report;
  } };
}

test("VM gate isolates every SHA/profile and records all pairs, inputs, metrics and cleanup", async () => {
  const f = await gateFixture(); const report = await f.run();
  expect(report.verdict).toBe("pass"); expect(report.errors).toEqual([]);
  expect(report.builds).toHaveLength(8); expect(report.comparisons).toHaveLength(8);
  expect(report.metrics).toHaveLength(16);
  expect(report.inputs).toHaveLength(7);
  expect(report.inputsRef).toBe("b".repeat(40));
  const frozen = f.calls.filter(c => c.argv[1] === "cat-file");
  expect(frozen).toHaveLength(7);
  expect(frozen.every(c => c.argv[3]!.startsWith(`${"b".repeat(40)}:`))).toBe(true);
  expect(report.calls.filter(c => c.argv[1] === "cat-file")
    .every(c => c.stdout === "" && c.stdoutSha256)).toBe(true);
  for (const row of report.comparisons) {
    expect(row.base).toHaveLength(11); expect(row.candidate).toHaveLength(11);
  }
  const builds = f.calls.filter(c => c.argv[0] === "fixture-cargo");
  // Profiles of one commit share a checkout; targets stay unique per profile.
  expect(new Set(builds.map(c => c.cwd)).size).toBe(2);
  expect(new Set(builds.map(c => c.argv[c.argv.indexOf("--target-dir") + 1])).size).toBe(8);
  expect(builds.every(c => c.argv.includes("--locked") && c.argv.includes("--offline"))).toBe(true);
  expect(builds.filter(c => c.argv.some(a => a.includes("heap-metrics")))).toHaveLength(4);
  const samples = f.calls.filter(c => c.argv[0]!.endsWith("hiko-cli") && !c.env.HIKO_HEAP_METRICS);
  expect(samples.slice(2, 6).map(c => c.argv[0]!.includes("/candidate."))).toEqual([false, true, true, false]);
  expect(samples.every(c => c.argv[c.argv.indexOf("--config") + 1]!.endsWith("/inputs/hiko.toml"))).toBe(true);
  expect(samples.every(c => c.argv[c.argv.indexOf("--policy") + 1] === "benchmark")).toBe(true);
  expect(f.calls.filter(c => c.argv.includes("remove"))).toHaveLength(2);
  expect(JSON.parse(await readFile(f.history, "utf8")).verdict).toBe("pass");
}, 30_000);

test("VM gate permits exactly one complete recheck only for valid measured regression", async () => {
  const once = await gateFixture({ regression: "once" }); const passed = await once.run();
  expect(passed.verdict).toBe("pass");
  expect(passed.comparisons).toHaveLength(16);
  expect(passed.comparisons[1]!.verdict).toBe("pass-with-recheck");
  const always = await gateFixture({ regression: "always" }); const failed = await always.run();
  expect(failed.verdict).toBe("fail"); expect(failed.comparisons).toHaveLength(16);
  expect(failed.errors.filter(e => e.includes("persists"))).toHaveLength(8);
}, 30_000);

test("VM gate rejects per-invocation output differences without retrying invalid attempts", async () => {
  const f = await gateFixture({ mismatch: true }); const report = await f.run();
  expect(report.verdict).toBe("fail");
  expect(report.comparisons).toHaveLength(8);
  expect(report.errors.some(e => e.includes("full stdout mismatch"))).toBe(true);
});

test("VM gate refuses power/load/probe failures, unsupported platforms and injected overrides", async () => {
  for (const options of [{ power: "Battery\n" }, { power: "" }, { load: NaN }, { load: 4 }, { platform: "linux" }, { hungProbe: true }]) {
    const f = await gateFixture(options); const report = await f.run();
    expect(report.verdict).toBe("fail");
    expect(f.calls.some(c => c.argv[0] === "fixture-cargo")).toBe(false);
  }
  for (const name of ["HIKO_BIN", "ALLOW_BATTERY", "CARGO_TARGET_DIR", "RUSTFLAGS", "RUSTC_WRAPPER"]) {
    const f = await gateFixture(); const report = await f.run({ [name]: "poison" });
    expect(report.verdict).toBe("fail"); expect(report.errors.join()).toContain(name);
    expect(f.calls).toHaveLength(0);
  }
}, 30_000);

test("VM gate invalidates mid-attempt power/load contamination without selectively retrying", async () => {
  for (const options of [{ powerSwitch: true }, { loadSwitch: true }]) {
    const f = await gateFixture(options); const report = await f.run();
    expect(report.verdict).toBe("fail"); expect(report.comparisons).toHaveLength(1);
    expect(report.comparisons[0]!.verdict).toBe("incomplete");
    expect(f.calls.filter(c => c.argv[0]!.endsWith("hiko-cli"))).toHaveLength(4);
    expect(report.errors.join()).toMatch(/changed|load exceeds/);
  }
}, 30_000);

test("VM gate cannot pass with invalid/missing metrics, failed builds or post-export failed exit", async () => {
  for (const options of [{ badMetrics: true }, { exportExit: true }, { buildFailure: true }, { badFeatures: true }]) {
    const f = await gateFixture(options); const report = await f.run();
    expect(report.verdict).toBe("fail"); expect(report.errors.length).toBeGreaterThan(0);
  }
}, 30_000);

test("VM gate refuses to overwrite history and retains large compiler transcripts losslessly", async () => {
  const existing = await gateFixture(); await writeFile(existing.history, "preserve");
  await expect(existing.run()).rejects.toThrow("already exists");
  expect(await readFile(existing.history, "utf8")).toBe("preserve");
  const f = await gateFixture({ largeTranscript: true }); const report = await f.run();
  expect(report.verdict).toBe("pass");
  const entries = report.calls.filter(c => c.stderrArtifact);
  expect(entries).toHaveLength(8);
  for (const entry of entries) {
    expect(entry.stderr).toBe("");
    expect(Buffer.from(await readFile(entry.stderrArtifact!, "utf8"), "base64").toString()).toBe("x".repeat(70_000));
  }
  expect(f.calls.filter(c => c.argv[1] === "rev-parse")).toHaveLength(2);
  expect(f.calls.filter(c => c.argv.includes("add")).every(c => /^[ab]{40}$/.test(c.argv.at(-1)!))).toBe(true);
}, 30_000);

test("VM gate preserves recoverable provenance when history publication fails", async () => {
  const f = await gateFixture({ historyFailure: true }); const report = await f.run();
  expect(report.verdict).toBe("fail"); expect(existsSync(report.recovery)).toBe(true);
  expect(JSON.parse(await readFile(report.recovery, "utf8")).errors.join()).toContain("history publication failed");
});

test("VM gate retains earlier successful phases and raw evidence after a later history-write failure", async () => {
  const f = await gateFixture({ historyAfter: 90 }); const report = await f.run();
  expect(report.verdict).toBe("fail"); expect(existsSync(report.recovery)).toBe(true);
  const published = JSON.parse(await readFile(f.history, "utf8"));
  const recovered = JSON.parse(await readFile(report.recovery, "utf8"));
  expect(published.calls.length).toBeGreaterThan(5);
  expect(recovered.calls.length).toBeGreaterThanOrEqual(published.calls.length);
  expect(recovered.errors.join()).toContain("history publication failed");
});

test("VM gate subprocess execution preserves separate bytes and kills/reaps a timeout", async () => {
  const env = { PATH: process.env.PATH ?? "" };
  const ok = await execute({ argv: [process.execPath, "-e", "process.stdout.write('a\\u0000b');process.stderr.write('error')"], cwd: tmpdir(), env, timeoutMs: 2_000 });
  expect(Buffer.from(ok.stdout, "base64").toString()).toBe("a\0b");
  expect(Buffer.from(ok.stderr, "base64").toString()).toBe("error");
  const timed = await execute({ argv: [process.execPath, "-e", "setInterval(()=>{},1000)"], cwd: tmpdir(), env, timeoutMs: 50 });
  expect(timed.timedOut).toBe(true); expect(timed.status).not.toBe(0);
});
