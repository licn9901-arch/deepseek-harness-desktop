import { build } from "esbuild";

await build({
  entryPoints: ["ui-src/plugin-manager.js"],
  bundle: true,
  format: "esm",
  minify: true,
  outfile: "ui/plugin-manager.js",
  platform: "browser",
  target: "es2022",
});
