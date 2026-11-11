// Usage: bun scripts/lint.ts
// RUST_TOOLCHAIN overrides the pinned toolchain. Explicit +toolchain avoids
// mise cargo-fmt recursion.
import { main, run, toolchain } from "./common.ts";

export function lintCommands(selected = process.env.RUST_TOOLCHAIN || toolchain) {
  const cargo = ["cargo", `+${selected}`];
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
