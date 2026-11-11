import { cpus, type, arch } from "node:os";
import { resolve } from "node:path";

export const root = resolve(import.meta.dir, "..");

// The repository's pinned Rust toolchain, shared by lint and the VM gate.
export const toolchain = "1.99.1";

export class ExitError extends Error {
  constructor(message: string, public exitCode = 1) {
    super(message);
  }
}

export async function run(command: readonly string[], stdout: "inherit" | 2 = "inherit") {
  const child = Bun.spawn([...command], { cwd: root, stdin: "inherit", stdout, stderr: "inherit" });
  const code = await child.exited;
  if (code !== 0) throw new ExitError(`${command.join(" ")} exited with status ${code}`, code);
}

export async function capture(
  command: readonly string[],
  errors: "inherit" | "ignore" | "capture" = "inherit",
) {
  const child = Bun.spawn([...command], {
    cwd: root,
    stdout: "pipe",
    stderr: errors === "capture" ? "pipe" : errors,
  });
  const [out, err, code] = await Promise.all([
    new Response(child.stdout).text(),
    child.stderr ? new Response(child.stderr).text() : "",
    child.exited,
  ]);
  if (code !== 0) throw new ExitError(`${command.join(" ")} exited with status ${code}`, code);
  return out + err;
}

export function requireTools(tools: string[], script: string) {
  for (const tool of tools) {
    if (!Bun.which(tool)) throw new ExitError(`${script} needs ${tool}`);
  }
}

export function runs() {
  const value = Number(process.env.RUNS || "10");
  if (!Number.isSafeInteger(value) || value < 1) throw new ExitError("RUNS must be a positive integer");
  return value;
}

export async function hikoBinary(allBuiltins = false) {
  if (process.env.HIKO_BIN) return process.env.HIKO_BIN;
  await run(["cargo", "build", "--release", "-p", "hiko-cli",
    ...(allBuiltins ? ["--features", "builtin-all"] : [])]);
  return "target/release/hiko-cli";
}

// hyperfine -N parses an argument string itself; it never launches a shell.
export function command(args: readonly string[]) {
  return args.map((arg) => /^[A-Za-z0-9_@%+=:,./{}-]+$/.test(arg)
    ? arg : `'${arg.replaceAll("'", "'\\''")}'`).join(" ");
}

export async function metadata() {
  return `- date: ${new Date().toISOString().slice(0, 10)}\n` +
    `- commit: ${(await capture(["git", "rev-parse", "--short", "HEAD"])).trim()}\n` +
    `- machine: ${type()} ${arch()}, ${cpus()[0]?.model || "unknown CPU"}\n`;
}

export async function main(action: () => Promise<void>) {
  try {
    process.chdir(root);
    await action();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = error instanceof ExitError ? error.exitCode : 1;
  }
}
