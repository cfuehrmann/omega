import { runApp } from "./ui-raw.js";

runApp().catch((err) => {
  console.error(err);
  process.exit(1);
});
