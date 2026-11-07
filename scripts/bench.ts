// Usage: bun scripts/bench.ts [output.md]
// RUNS defaults to 10; HIKO_BIN skips the release build. Requires hyperfine.
import { appendFile, cp, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { capture, command, hikoBinary, main, metadata, requireTools, root, run, runs } from "./common.ts";

export async function benchmarks(args = process.argv.slice(2)) {
  if (args.includes("--help")) {
    console.log("Usage: bun scripts/bench.ts [output.md]\nRUNS=n HIKO_BIN=path");
    return;
  }
  requireTools(["hyperfine"], "bench.ts");
  const count = runs();
  const binary = await hikoBinary(true);
  const out = resolve(root, args[0] || "target/bench.md");
  const hiko = [binary, "run", "--config", "hiko.toml", "--policy", "benchmark"];
  const hikoCommand = (...args: string[]) => command([...hiko, ...args]);
  await mkdir(dirname(out), { recursive: true });
  const temp = await mkdtemp(join(tmpdir(), "hiko-bench-"));
  try {
    await writeFile(out, `# Hiko benchmarks\n\n${await metadata()}- runs per benchmark: ${count}\n`);
    await appendFile(out, "\n## Process creation\n\n```text\n" +
      await capture(["cargo", "run", "-q", "--release", "-p", "hiko-vm", "--example", "process_creation_cost"]) +
      "```\n");

    async function bench(title: string, commands: string[], parameters: string[] = []) {
      const part = join(temp, "part.md");
      await run(["hyperfine", "-N", "--warmup", "2", "--runs", String(count),
        "--export-markdown", part, ...parameters, ...commands], 2);
      await appendFile(out, `\n## ${title}\n\n${await readFile(part, "utf8")}`);
    }

    await bench("Interpreter", ["ack", "tak", "nqueens", "primes", "ec_ack", "ec_fib",
      "early_exit", "global_builtin", "list_length"].map((name) => hikoCommand(`benchmarks/${name}.hml`)));
    await bench("Spawn and await 6,000 children", [hikoCommand("--workers", "{workers}",
      "benchmarks/spawn_await.hml")], ["-L", "workers", "1,8"]);
    await bench("One list shared by 200 children", [hikoCommand("benchmarks/spawn_shared.hml")]);
    await bench("Strings", ["string_concat", "string_args", "spawn_strings"]
      .map((name) => hikoCommand(`benchmarks/${name}.hml`)));
    await bench("Regex with one pattern, 12,000 calls", [hikoCommand("benchmarks/regex_loop.hml")]);
    await bench("Local recursive helpers", [hikoCommand("benchmarks/local_rec.hml")]);
    await bench("CPU fan-out, 64 children", [hikoCommand("--workers", "{workers}",
      "benchmarks/fanout_cpu.hml")], ["-L", "workers", "1,2,4,8"]);
    await bench("File-read fan-out, 1,000 children", [hikoCommand("--workers", "{workers}",
      "benchmarks/fanout_read.hml")], ["-L", "workers", "1,2,4,8"]);
    await bench("Sequential reads, 1,000 in one process", [hikoCommand("benchmarks/read_sequential.hml")]);
    await bench("Sequential-read fan-out, 32 children x 100 reads", [hikoCommand("--workers", "{workers}",
      "benchmarks/fanout_read_sequential.hml")], ["-L", "workers", "1,2,4,8"]);

    let gitDir = ".git";
    if (!(await stat(join(root, ".git"))).isDirectory()) {
      gitDir = "target/bench-git";
      const destination = join(root, gitDir);
      if (!existsSync(destination)) {
        const common = (await capture(["git", "rev-parse", "--git-common-dir"])).trim();
        await cp(resolve(root, common), destination, { recursive: true });
      }
    }
    await mkdir(join(root, "target"), { recursive: true });
    for (const file of ["benchmarks/fanout_hash.hml", "examples/benchmark.hml", "examples/benchmark.py"]) {
      const source = await readFile(join(root, file), "utf8");
      await writeFile(join(root, "target", `bench-${basename(file)}`), source.replaceAll('".git"', JSON.stringify(gitDir)));
    }
    await bench(`File-hash fan-out over ${gitDir}, 32 children`, [hikoCommand("--workers", "{workers}",
      "target/bench-fanout_hash.hml")], ["-L", "workers", "1,2,4,8"]);

    const python = Bun.which("python3") && Bun.spawnSync(["python3", "-c", "import blake3"],
      { cwd: root, stdout: "ignore", stderr: "ignore" }).exitCode === 0;
    if (python) {
      await bench(`Walk and hash ${gitDir} (vs Python)`, [
        command([binary, "run", "--config", "hiko.toml", "target/bench-benchmark.hml"]),
        command(["python3", "target/bench-benchmark.py"]),
      ]);
    } else {
      console.error("skipping the walk benchmark: python3 has no blake3 module");
    }
    console.error(`wrote ${out}`);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}

if (import.meta.main) await main(() => benchmarks());
