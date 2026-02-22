/**
 * Self-modification orchestration for Omega.
 *
 * Provides the workflow:
 *   1. Edit source files (agent writes them)
 *   2. Run `bun test` to validate
 *   3. On pass:  git add -A && git commit -m "..."
 *   4. On fail:  git checkout . (revert all changes)
 *   5. Restart the agent process (spawn new bun process)
 *
 * Usage:
 *   a) Write changed source files (via write_file tool)
 *   b) Call commitOrRevert() with a description
 *   c) If committed, call restart() to reload the new code
 *
 * The git working tree is the experiment. Committed = stable.
 */

import { spawn, execSync } from "child_process";

interface SelfModifyResult {
  success: boolean;
  testsPassed: boolean;
  committed: boolean;
  commitHash?: string;
  testOutput: string;
  revertReason?: string;
}

/**
 * Run `bun test` in the given directory and return results.
 * @param cwd - Working directory to run tests in (defaults to process.cwd())
 * @param files - Optional array of file paths to run (bun test patterns)
 */
export async function runTests(
  cwd?: string,
  files?: string[]
): Promise<{ passed: boolean; output: string }> {
  const dir = cwd ?? process.cwd();
  return new Promise((resolve) => {
    let output = "";
    const args = ["test", ...(files ?? [])];
    const proc = spawn("bun", args, {
      stdio: ["ignore", "pipe", "pipe"],
      cwd: dir,
    });

    proc.stdout.on("data", (d: Buffer) => { output += d.toString(); });
    proc.stderr.on("data", (d: Buffer) => { output += d.toString(); });

    proc.on("close", (code) => {
      resolve({ passed: code === 0, output: output.trim() });
    });

    proc.on("error", (err) => {
      resolve({ passed: false, output: `Failed to run tests: ${err.message}` });
    });
  });
}

/**
 * Get the list of files changed in the working tree vs HEAD.
 * @param cwd - Working directory (defaults to process.cwd())
 */
export function getChangedFiles(cwd?: string): string[] {
  const dir = cwd ?? process.cwd();
  try {
    const out = execSync("git diff --name-only HEAD", { encoding: "utf-8", cwd: dir });
    return out.trim().split("\n").filter(Boolean);
  } catch {
    return [];
  }
}

/**
 * Commit all working-tree changes with the given message.
 * Returns the short commit hash on success.
 * @param message - Commit message
 * @param cwd - Working directory (defaults to process.cwd())
 */
export function gitCommit(message: string, cwd?: string): string {
  const dir = cwd ?? process.cwd();
  execSync("git add -A", { cwd: dir });
  execSync(`git commit -m ${JSON.stringify(message)}`, { cwd: dir });
  const hash = execSync("git rev-parse --short HEAD", {
    encoding: "utf-8",
    cwd: dir,
  }).trim();
  return hash;
}

/**
 * Revert all working-tree changes (staged, unstaged, and untracked files).
 * Does NOT touch committed history.
 * @param cwd - Working directory (defaults to process.cwd())
 */
export function gitRevert(cwd?: string): void {
  const dir = cwd ?? process.cwd();
  // Unstage staged changes first
  execSync("git reset HEAD .", { cwd: dir });
  // Revert tracked file modifications
  execSync("git checkout .", { cwd: dir });
  // Remove untracked files that were added
  execSync("git clean -fd", { cwd: dir });
}

/**
 * Run tests in the given directory, then commit on pass or revert on fail.
 *
 * @param commitMessage - The commit message if tests pass.
 * @param cwd - Working directory (defaults to process.cwd())
 * @returns SelfModifyResult with full details.
 */
export async function commitOrRevert(
  commitMessage: string,
  cwd?: string
): Promise<SelfModifyResult> {
  const dir = cwd ?? process.cwd();
  const { passed, output: testOutput } = await runTests(dir);

  if (passed) {
    try {
      const commitHash = gitCommit(commitMessage, dir);
      return {
        success: true,
        testsPassed: true,
        committed: true,
        commitHash,
        testOutput,
      };
    } catch (err: any) {
      return {
        success: false,
        testsPassed: true,
        committed: false,
        testOutput,
        revertReason: `Git commit failed: ${err.message}`,
      };
    }
  } else {
    try {
      gitRevert(dir);
    } catch (revertErr: any) {
      return {
        success: false,
        testsPassed: false,
        committed: false,
        testOutput,
        revertReason: `Tests failed AND git revert failed: ${revertErr.message}`,
      };
    }
    return {
      success: false,
      testsPassed: false,
      committed: false,
      testOutput,
      revertReason: "Tests failed. Changes reverted.",
    };
  }
}

/**
 * Restart the agent by spawning a new bun process with the same args.
 * The current process exits; the new process takes over the terminal.
 *
 * Only call this after a successful commit. The new process will run the
 * freshly committed code.
 */
function restart(): never {
  const args = process.argv.slice(1); // e.g. ["src/main.tsx"]
  const bun = process.execPath;       // path to the bun binary

  const child = spawn(bun, args, {
    stdio: "inherit",
    detached: false,
    env: process.env,
    cwd: process.cwd(),
  });

  child.on("error", (err) => {
    process.stderr.write(`Restart failed: ${err.message}\n`);
    process.exit(1);
  });

  // Exit the current process — the child takes over the terminal
  process.exit(0);
}
