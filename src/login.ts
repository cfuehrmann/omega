#!/usr/bin/env bun
// Run this to authenticate with Claude Max: bun run src/login.ts

import { startOAuthFlow, getApiKey, getAuthToken } from "./auth.js";
import { createInterface } from "readline";

async function main() {
  // Check if already logged in with a working API key
  const existingKey = await getApiKey();
  if (existingKey) {
    console.log("✓ Already authenticated. Claude Max API key found.");
    console.log("  To re-authenticate, delete ~/.config/omega/api-key and oauth-token.json");
    return;
  }

  // Check if we have an OAuth token but no API key (can try to create one)
  const existingToken = await getAuthToken();
  if (existingToken) {
    console.log("Found OAuth token, creating API key for Claude Max billing...");
    const key = await getApiKey();
    if (key) {
      console.log("✓ API key created. Omega will now bill through Claude Max.");
      return;
    }
    console.log("⚠ Could not create API key from existing token. Re-authenticating...\n");
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
    const token = await exchangeCode(code);
    console.log("\n✓ OAuth token saved.");

    // Now create an API key from the token (Claude Max billing)
    console.log("  Creating API key for Claude Max billing...");
    const apiKey = await getApiKey();
    if (apiKey) {
      console.log("✓ API key created. Omega will bill through Claude Max.");
    } else {
      console.log("⚠ Could not create API key. Will use OAuth token directly (per-token billing).");
    }
  } catch (err: any) {
    console.error(`\n✗ Authentication failed: ${err.message}`);
    process.exit(1);
  }
}

main();
