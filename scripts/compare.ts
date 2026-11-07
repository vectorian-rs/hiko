// Usage: bun scripts/compare.ts [output.md] [benchmark ...]
// RUNS defaults to 10; HIKO_BIN skips the build; ALLOW_BATTERY=1 permits macOS battery runs.
import { appendFile, mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { capture, command, ExitError, hikoBinary, main, metadata, requireTools, root, run, runs } from "./common.ts";

export function lastLine(output: string) {
  return output.replace(/\n$/, "").split("\n").at(-1) ?? "";
}

export function comparisonRow(name: string, means: number[]) {
  if ((means.length !== 3 && means.length !== 4) || means.some((n) => !Number.isFinite(n) || n <= 0)) {
    throw new ExitError(`invalid hyperfine results for ${name}`);
  }
  const [hiko, lua, python, jit] = means as [number, number, number, number?];
  const ms = (seconds: number) => `${(seconds * 1000).toFixed(0)} ms`;
  return `| \`${name}\` | ${ms(hiko)} | ${ms(lua)} | ${ms(python)} | ${jit === undefined ? "-" : ms(jit)} | ` +
    `${(hiko / lua).toFixed(1)}x | ${(hiko / python).toFixed(1)}x |\n`;
}

export async function compare(args = process.argv.slice(2)) {
  if (args.includes("--help")) {
    console.log("Usage: bun scripts/compare.ts [output.md] [benchmark ...]\nRUNS=n HIKO_BIN=path ALLOW_BATTERY=1");
    return;
  }
  requireTools(["hyperfine", "lua", "python3"], "compare.ts");
  const count = runs();
  const binary = await hikoBinary();
  const out = resolve(root, args[0] || "target/compare.md");
  const names = args.slice(1);
  if (!names.length) {
    names.push(...(await readdir(join(root, "benchmarks/compare")))
      .filter((name) => name.endsWith(".hml")).sort().map((name) => name.slice(0, -4)));
  }
  const jit = Boolean(Bun.which("luajit"));
  const version = async (args: string[]) => (await capture(args, "capture")).trim().split("\n")[0]!;
  await mkdir(dirname(out), { recursive: true });
  const temp = await mkdtemp(join(tmpdir(), "hiko-compare-"));
  try {
    await writeFile(out, `# Cross-language comparison\n\n${await metadata()}` +
      `- ${(await version(["lua", "-v"])).split(" ").slice(0, 2).join(" ")}; ${await version(["python3", "--version"])}\n` +
      (jit ? `- ${(await version(["luajit", "-v"])).split(" ").slice(0, 2).join(" ")}\n` : "") +
      `- runs per implementation: ${count} (hyperfine mean)\n\n` +
      "| Program | Hiko | Lua | Python | LuaJIT | Hiko / Lua | Hiko / Python |\n" +
      "|---|---|---|---|---|---|---|\n");

    for (const name of names) {
      if (Bun.which("pmset") && !process.env.ALLOW_BATTERY) {
        const battery = Bun.spawnSync(["pmset", "-g", "batt"], { stdout: "pipe", stderr: "ignore" });
        if (!new TextDecoder().decode(battery.stdout).split("\n")[0]!.includes("AC Power")) {
          throw new ExitError("on battery power: plug in, or set ALLOW_BATTERY=1", 2);
        }
      }
      const programs = [
        [binary, "run", "--config", "hiko.toml", "--policy", "benchmark", `benchmarks/compare/${name}.hml`],
        ["lua", `benchmarks/compare/${name}.lua`],
        ["python3", `benchmarks/compare/${name}.py`],
        ...(jit ? [["luajit", `benchmarks/compare/${name}.lua`]] : []),
      ];
      const expected = lastLine(await capture(programs[0]!, "ignore"));
      for (const program of programs) {
        const output = lastLine(await capture(program, "ignore"));
        if (output !== expected) {
          throw new ExitError(`${name}: '${command(program)}' printed '${output}', Hiko printed '${expected}'`);
        }
      }
      const report = join(temp, `${name}.json`);
      await run(["hyperfine", "-N", "--warmup", "1", "--runs", String(count), "--export-json", report,
        ...programs.map(command)], 2);
      const { results } = await Bun.file(report).json() as { results: { mean: number }[] };
      await appendFile(out, comparisonRow(name, results.map((result) => result.mean)));
    }
    console.error(`wrote ${out}`);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}

if (import.meta.main) await main(() => compare());
