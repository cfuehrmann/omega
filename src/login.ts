#!/usr/bin/env bun
// Run this to authenticate with Claude Max: bun run src/login.ts

import { startOAuthFlow, getAuthToken } from "./auth.js";
import { createInterface } from "readline";

async function main() {
  const existing = await getAuthToken();
  if (existing) {
    console.log("✓ Already authenticated.");
    console.log("  To re-authenticate: rm ~/.config/omega/oauth-token.json");
    return;
  }

  console.log("Authenticating with Claude Max...\n");

  const { url, exchangeCode } = await startOAuthFlow();

  console.log("1. Open this URL in your browser:\n");
  console.log(`   ${url}\n`);
  console.log("2. Log in with your Claude Max account");
  console.log("3. Paste the code below (including the #state part)\n");

  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const code = await new Promise<string>((resolve) => {
    rl.question("Code: ", (answer) => {
      rl.close();
      resolve(answer.trim());
    });
  });

  if (!code) {
    console.log("No code provided. Aborting.");
    process.exit(1);
  }

  try {
    await exchangeCode(code);
    console.log("\n✓ Authenticated! Omega will use your Claude Max subscription.");
  } catch (err: any) {
    console.error(`\n✗ Authentication failed: ${err.message}`);
    process.exit(1);
  }
}

main();
