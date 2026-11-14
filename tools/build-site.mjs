// Package the precompiled browser runtime independently of the Rust toolchain.
import { access, cp, mkdir, readdir, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const project = fileURLToPath(new URL("../", import.meta.url));
const source = path.join(project, "web");
const output = path.join(project, "dist");
await access(path.join(source, "mc2engine.wasm"));
await rm(output, { recursive: true, force: true });
await mkdir(output);
for (const entry of await readdir(source)) {
  if (entry === "game") continue;
  await cp(path.join(source, entry), path.join(output, entry), { recursive: true });
}
const relay = process.env.MC2_RELAY_URL?.trim() || "https://mc2-multiplayer.aryanbaburajan2007.workers.dev/relay";
if (relay) {
  const url = new URL(relay);
  if (!['https:', 'wss:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/relay') throw new Error('MC2_RELAY_URL must be an HTTPS or WSS URL ending in /relay');
}
await writeFile(path.join(output, 'config.js'), `globalThis.MC2_RELAY_URL = ${JSON.stringify(relay)};\n`);
await mkdir(path.join(output, "vendor"), { recursive: true });
await cp(path.join(project, "node_modules/@ffmpeg/ffmpeg/dist/esm"), path.join(output, "vendor/ffmpeg"), { recursive: true });
await cp(path.join(project, "node_modules/@ffmpeg/core/dist/esm"), path.join(output, "vendor/ffmpeg-core"), { recursive: true });
console.log("Built dist/ without game assets");
