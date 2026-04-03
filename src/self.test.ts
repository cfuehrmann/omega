import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { runTests, getChangedFiles, commitOrRevert, gitCommit, gitRevert } from "./self.js";
import { spawnSync } from "child_process";
import { writeFileSync, mkdirSync, rmSync, existsSync } from "fs";
import { join } from "path";

// ---------------------------------------------------------------------------
// Tests for self.ts
//
// All git operations are tested in an isolated git repo at TMP_REPO so we
// never touch the real repo. We pass cwd explicitly to all helpers.
// ---------------------------------------------------------------------------

const TMP_REPO = "/tmp/omega-self-test-repo";
const SHELL = "/usr/bin/bash";

function sh(cmd: string, cwd: string): string {
  // Clear git plumbing variables so commands run in the isolated temp repo
  // are not affected by GIT_INDEX_FILE or GIT_DIR from the parent process
  // (e.g. when this test suite runs inside a git pre-commit hook).
  const env = { ...process.env };
  delete env["GIT_INDEX_FILE"];
  delete env["GIT_DIR"];
  delete env["GIT_WORK_TREE"];
  const r = spawnSync(SHELL, ["-c", cmd], { cwd, encoding: "utf-8", env });
  if (r.status !== 0) {
    throw new Error(`Command failed (${cmd}): ${r.stderr}`);
  }
  return r.stdout;
}

function setupTmpRepo(): void {
  if (existsSync(TMP_REPO)) rmSync(TMP_REPO, { recursive: true, force: true });
  mkdirSync(TMP_REPO, { recursive: true });

  sh("git init", TMP_REPO);
  sh('git config user.email "test@omega"', TMP_REPO);
  sh('git config user.name "Omega Test"', TMP_REPO);

  // Create package.json for bun test
  writeFileSync(
    join(TMP_REPO, "package.json"),
    JSON.stringify({ name: "test", type: "module" })
  );

  // Create a passing test file
  writeFileSync(
    join(TMP_REPO, "passing.test.ts"),
    `import { describe, it, expect } from "bun:test";\n` +
    `describe("suite", () => { it("passes", () => { expect(1).toBe(1); }); });\n`
  );

  sh("git add -A", TMP_REPO);
  sh('git commit -m "initial"', TMP_REPO);
}

function teardownTmpRepo(): void {
  if (existsSync(TMP_REPO)) rmSync(TMP_REPO, { recursive: true, force: true });
}

// --- runTests ---

describe("runTests", () => {
  beforeEach(() => { setupTmpRepo(); });
  afterEach(() => { teardownTmpRepo(); });

  it("returns passed=true when tests pass", async () => {
    const { passed, output } = await runTests(TMP_REPO);
    expect(passed).toBe(true);
    expect(output).toContain("pass");
  }, 30_000);

  it("returns passed=false when tests fail", async () => {
    writeFileSync(
      join(TMP_REPO, "failing.test.ts"),
      `import { it, expect } from "bun:test";\nit("fails", () => { expect(1).toBe(2); });\n`
    );
    const { passed, output } = await runTests(TMP_REPO);
    expect(passed).toBe(false);
    expect(output).toContain("fail");
  }, 30_000);

  it("captures test output as non-empty string", async () => {
    const { output } = await runTests(TMP_REPO);
    expect(typeof output).toBe("string");
    expect(output.length).toBeGreaterThan(0);
  }, 30_000);
});

// --- getChangedFiles ---

describe("getChangedFiles", () => {
  it("returns an array", () => {
    // Just verify the return type — real repo may have uncommitted changes
    const changed = getChangedFiles();
    expect(Array.isArray(changed)).toBe(true);
  });

  it("returns empty array when isolated repo is clean", () => {
    setupTmpRepo();
    try {
      const changed = getChangedFiles(TMP_REPO);
      expect(changed).toEqual([]);
    } finally {
      teardownTmpRepo();
    }
  });

  it("returns changed files in isolated repo", () => {
    setupTmpRepo();
    try {
      writeFileSync(join(TMP_REPO, "newfile.ts"), "// new\n");
      sh("git add newfile.ts", TMP_REPO);
      const changed = getChangedFiles(TMP_REPO);
      expect(changed).toContain("newfile.ts");
    } finally {
      teardownTmpRepo();
    }
  });
});

// --- gitCommit ---

describe("gitCommit", () => {
  beforeEach(() => { setupTmpRepo(); });
  afterEach(() => { teardownTmpRepo(); });

  it("commits staged changes and returns 7-char hash", () => {
    writeFileSync(join(TMP_REPO, "new.ts"), "// new file\n");
    sh("git add new.ts", TMP_REPO);

    const hash = gitCommit("test: add new.ts", TMP_REPO);
    expect(hash).toMatch(/^[0-9a-f]{7}$/);

    const log = sh("git log --oneline -1", TMP_REPO);
    expect(log).toContain("test: add new.ts");
  });

  it("commits untracked new files via git add -A", () => {
    writeFileSync(join(TMP_REPO, "untracked.ts"), "// untracked\n");
    // Don't stage manually — gitCommit does git add -A
    const hash = gitCommit("test: via add -A", TMP_REPO);
    expect(hash).toMatch(/^[0-9a-f]{7}$/);
  });
});

// --- gitRevert ---

describe("gitRevert", () => {
  beforeEach(() => { setupTmpRepo(); });
  afterEach(() => { teardownTmpRepo(); });

  it("removes untracked files", () => {
    writeFileSync(join(TMP_REPO, "untracked.ts"), "// untracked\n");
    gitRevert(TMP_REPO);
    const status = sh("git status --porcelain", TMP_REPO);
    expect(status.trim()).toBe("");
  });

  it("reverts staged modifications", () => {
    writeFileSync(join(TMP_REPO, "passing.test.ts"), "// modified\n");
    sh("git add passing.test.ts", TMP_REPO);
    gitRevert(TMP_REPO);
    const status = sh("git status --porcelain", TMP_REPO);
    expect(status.trim()).toBe("");
  });

  it("reverts unstaged modifications", () => {
    writeFileSync(join(TMP_REPO, "passing.test.ts"), "// modified unstaged\n");
    gitRevert(TMP_REPO);
    const status = sh("git status --porcelain", TMP_REPO);
    expect(status.trim()).toBe("");
  });
});

// --- commitOrRevert full workflow ---

describe("commitOrRevert", () => {
  beforeEach(() => { setupTmpRepo(); });
  afterEach(() => { teardownTmpRepo(); });

  it("commits and returns success when tests pass", async () => {
    writeFileSync(join(TMP_REPO, "new.ts"), "// new file\n");

    const result = await commitOrRevert("feat: add new.ts", TMP_REPO);

    expect(result.testsPassed).toBe(true);
    expect(result.committed).toBe(true);
    expect(result.commitHash).toMatch(/^[0-9a-f]{7}$/);
    expect(result.success).toBe(true);
    expect(typeof result.testOutput).toBe("string");
  }, 30_000);

  it("reverts and returns failure when tests fail", async () => {
    writeFileSync(
      join(TMP_REPO, "failing.test.ts"),
      `import { it, expect } from "bun:test";\nit("fails", () => { expect(1).toBe(2); });\n`
    );

    const result = await commitOrRevert("feat: broken change", TMP_REPO);

    expect(result.testsPassed).toBe(false);
    expect(result.committed).toBe(false);
    expect(result.success).toBe(false);
    expect(result.revertReason).toContain("Tests failed");

    // Working tree should be clean after revert
    const status = sh("git status --porcelain", TMP_REPO);
    expect(status.trim()).toBe("");
  }, 30_000);

  it("includes test output in result", async () => {
    const result = await commitOrRevert("chore: no changes", TMP_REPO);
    expect(typeof result.testOutput).toBe("string");
    expect(result.testOutput.length).toBeGreaterThan(0);
  }, 30_000);
});
