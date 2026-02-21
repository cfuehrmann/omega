#!/usr/bin/env bun
// Run this to authenticate with Claude Max: bun run src/login.ts

import { startOAuthFlow, getApiKey, getAuthToken } from "./auth.js";
import { createInterface } from "readline";

async function main() {
  // Check if already logged in with a working API key
  const existingKey = await getApiKey();
  if (existingKey) {
    console.log("✓ Already authenticated. Claude Max API key found.");
    console.log("  To re-authenticate:");
    console.log("  rm ~/.config/omega/api-key ~/.config/omega/oauth-token.json");
    return;
  }

  // If we have an OAuth token, try to create an API key from it
  const existingToken = await getAuthToken();
  if (existingToken) {
    console.log("Found OAuth token, creating API key...");
    const key = await getApiKey();
    if (key) {
      console.log("✓ API key created. Omega will bill through Claude Max.");
      return;
    }
    console.log("⚠ Could not create API key. Re-authenticating...\n");
  }

  console.log("Authenticating with Claude Max...\n");

  const { url, exchangeCode } = await startOAuthFlow();

  console.log("1. Open this URL in your browser:\n");
  console.log(`   ${url}\n`);
  console.log("2. Log in with your Claude Max account");
  console.log("3. You'll be redirected to a page with a code");
  console.log("4. Paste the code below\n");

  const rl = createInterface({ input: process.stdin, output: process.stdout });
  let code = await new Promise<string>((resolve) => {
    rl.question("Code: ", (answer) => {
      rl.close();
      resolve(answer.trim());
    });
  });

  if (code.includes("#")) {
    code = code.split("#")[0];
  }

  if (!code) {
    console.log("No code provided. Aborting.");
    process.exit(1);
  }

  try {
    await exchangeCode(code);
    console.log("\n✓ OAuth token saved.");

    console.log("  Creating API key for Claude Max...");
    const apiKey = await getApiKey();
    if (apiKey) {
      console.log("✓ Done! Omega will bill through your Claude Max subscription.");
    } else {
      console.log("⚠ Could not create API key. The OAuth token may lack the right scope.");
    }
  } catch (err: any) {
    console.error(`\n✗ Authentication failed: ${err.message}`);
    process.exit(1);
  }
}

main();
