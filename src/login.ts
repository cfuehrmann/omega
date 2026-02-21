#!/usr/bin/env bun
// Run this to authenticate with Claude Max: bun run src/login.ts

import { startOAuthFlow, getAuthToken } from "./auth.js";
import { createInterface } from "readline";

async function main() {
  // Check if already logged in
  const existing = await getAuthToken();
  if (existing) {
    console.log("✓ Already authenticated. Token found.");
    console.log("  To re-authenticate, delete ~/.config/omega/oauth-token.json");
    return;
  }

  console.log("Authenticating with Claude Max...\n");

  const { url, exchangeCode } = await startOAuthFlow();

  console.log("1. Open this URL in your browser:\n");
  console.log(`   ${url}\n`);
  console.log("2. Log in and authorize Omega");
  console.log("3. You'll be redirected to a page with a code");
  console.log("4. Paste the code below\n");

  const rl = createInterface({ input: process.stdin, output: process.stdout });
  let code = await new Promise<string>((resolve) => {
    rl.question("Code: ", (answer) => {
      rl.close();
      resolve(answer.trim());
    });
  });

  // Strip the #state suffix if the user pasted the full redirect value
  if (code.includes("#")) {
    code = code.split("#")[0];
  }

  if (!code) {
    console.log("No code provided. Aborting.");
    process.exit(1);
  }

  try {
    await exchangeCode(code);
    console.log("\n✓ Authenticated successfully! Token saved.");
    console.log("  Omega will now use your Claude Max subscription.");
  } catch (err: any) {
    console.error(`\n✗ Authentication failed: ${err.message}`);
    process.exit(1);
  }
}

main();
