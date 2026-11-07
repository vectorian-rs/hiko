import { afterEach, expect, test } from "bun:test";
import { chmod, copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { comparisonRow, lastLine } from "./compare.ts";

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
