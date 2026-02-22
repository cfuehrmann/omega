import type { KnipConfig } from "knip";

const config: KnipConfig = {
  entry: [
    "src/**/*.test.ts",   // bun test picks these up automatically
  ],
  project: ["src/**/*.ts"],
};

export default config;
