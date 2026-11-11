// Builds the compatibility fixture apps into ../fixtures/compat/<app>/.
//
// The output is committed, so the Rust tests never need Node. Rerun this
// (`npm ci && npm run build`) only to change an app or update a library;
// production builds of the real libraries are used throughout, as a
// scraper would meet them.
import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import * as esbuild from "esbuild";
import { build as viteBuild } from "vite";

const here = dirname(fileURLToPath(import.meta.url));
const apps = join(here, "apps");
const out = join(here, "..", "fixtures", "compat");

const bundleOptions = {
  bundle: true,
  minify: true,
  format: "iife",
  target: "es2020",
  jsx: "automatic",
  legalComments: "eof",
  define: { "process.env.NODE_ENV": '"production"' },
  logLevel: "warning",
};

async function fresh(app) {
  const dir = join(out, app);
  await rm(dir, { recursive: true, force: true });
  await mkdir(dir, { recursive: true });
  return dir;
}

async function copyStatic(app, dir) {
  await cp(join(apps, app, "static"), dir, { recursive: true });
}

async function copyVendor(dir, files) {
  await mkdir(join(dir, "vendor"), { recursive: true });
  for (const [from, to] of files) {
    await cp(join(here, "node_modules", from), join(dir, "vendor", to));
  }
}

// A client-rendered React SPA, and the same with history routing.
for (const app of ["react-spa", "router"]) {
  const dir = await fresh(app);
  await copyStatic(app, dir);
  await cp(join(apps, app, "index.html"), join(dir, "index.html"));
  await esbuild.build({
    ...bundleOptions,
    entryPoints: [join(apps, app, "src", "main.jsx")],
    outfile: join(dir, "assets", "app.js"),
  });
}

// A real Next.js static export: an App Router page with a client
// navigation, and a Pages Router page, hydrated by Next's own runtime.
{
  const app = "next";
  const dir = await fresh(app);
  const root = join(apps, app);
  execFileSync(join(here, "node_modules", ".bin", "next"), ["build"], {
    cwd: root,
    stdio: ["ignore", "ignore", "inherit"],
    env: { ...process.env, NEXT_TELEMETRY_DISABLED: "1" },
  });
  await cp(join(root, "out"), dir, { recursive: true });
}

// Vue 3 from its global build, with axios.
{
  const app = "vue";
  const dir = await fresh(app);
  await copyStatic(app, dir);
  await cp(join(apps, app, "index.html"), join(dir, "index.html"));
  await copyVendor(dir, [
    ["vue/dist/vue.global.prod.js", "vue.global.prod.js"],
    ["axios/dist/axios.min.js", "axios.min.js"],
  ]);
}

// jQuery with XHR, .load() and JSONP.
{
  const app = "jquery";
  const dir = await fresh(app);
  await copyStatic(app, dir);
  await copyVendor(dir, [["jquery/dist/jquery.min.js", "jquery-3.7.1.min.js"]]);
}

// A Vite production build: module scripts, modulepreload, code splitting.
{
  const app = "vite";
  const dir = await fresh(app);
  await viteBuild({
    configFile: false,
    root: join(apps, app),
    base: "/compat/vite/",
    logLevel: "warn",
    esbuild: { jsx: "automatic", jsxImportSource: "preact" },
    build: {
      outDir: dir,
      emptyOutDir: true,
      rollupOptions: {
        output: {
          manualChunks: (id) => (id.includes("node_modules") ? "vendor" : undefined),
        },
      },
    },
  });
}
