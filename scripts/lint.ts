// Usage: bun scripts/lint.ts
// RUST_TOOLCHAIN overrides 1.99.1. Explicit +toolchain avoids mise cargo-fmt recursion.
import { main, run } from "./common.ts";

export function lintCommands(toolchain = process.env.RUST_TOOLCHAIN || "1.99.1") {
  const cargo = ["cargo", `+${toolchain}`];
  return [
    [...cargo, "fmt", "--all", "--", "--check"],
    [...cargo, "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
    [...cargo, "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"],
    [...cargo, "clippy", "--workspace", "--all-targets", "--no-default-features", "--", "-D", "warnings"],
  ];
}

if (import.meta.main) await main(async () => {
  for (const args of lintCommands()) await run(args);
});
