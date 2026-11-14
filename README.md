# Modern Combat 2: Black Pegasus - browser runtime

A browser runtime for the Android v1.0.5 ARM game library. The game is not recompiled: a Rust emulator compiled to WebAssembly loads and executes the original `libsandstorm2.so`. Players provide their own APK and game-data ZIP; the site does not distribute game assets. See [SETUP.md](SETUP.md) for the expected files and multiplayer instructions.

## Architecture

- **Emulation:** `engine/` contains an ARMv7 interpreter with a decoded-block cache, an ELF loader, guest memory, and high-level implementations of native imports, JNI, libc, threading, and game-specific behavior. The `Platform` interface bridges the emulator to its host; `engine/src/wasm.rs` exposes the browser entry points.
- **Browser runtime:** `web/worker.js` runs the WASM engine and renders through an `OffscreenCanvas`. `web/gl.js` translates OpenGL ES 2 calls to WebGL2. The main thread handles the UI, Web Audio, and keyboard, mouse, and touch input; keyboard and mouse actions are mapped to the game's touch controls. Game-specific hooks update prompts and unlock missions.
- **Assets and saves:** `web/local-game.js` extracts the ARM library and game data from the player's files into Cache Storage. `web/sw.js` serves those files locally; `.pak` archives are loaded into worker memory for synchronous emulator reads. Game writes are persisted separately in the `mc2-saves` IndexedDB database. FFmpeg WebAssembly converts the intro locally and caches the result. Browser storage is scoped to the site origin and profile.
- **Multiplayer:** `web/network.js` presents virtual sockets to the game and carries room-scoped traffic over WebSockets. `tools/relay.mjs` provides the local Node relay. In production, `cloudflare/multiplayer.js` routes each room to a Cloudflare Durable Object, which relays discovery, datagrams, and streams. Room names isolate traffic but are not authentication.
- **Deployment:** `tools/build-site.mjs` builds the static runtime into `dist/` for Vercel, including FFmpeg dependencies and excluding `web/game/`. The multiplayer Worker is configured separately in `wrangler.multiplayer.jsonc`.

## Development

Requires Node.js 18 or later. To rebuild the engine, install Rust and the `wasm32-unknown-unknown` target.

```sh
npm install
npm run serve          # site and local WebSocket relay at localhost:8080
npm run build          # rebuild web/mc2engine.wasm
npm run build:site     # package the static site in dist/
npm run test:network   # local relay and guest socket tests
```

For runtime development, put your own Android v1.0.5 reference files in `reference/` at the repository root. Keep the APK and its separate data ZIP there for inspection and browser testing; see [SETUP.md](SETUP.md) for the expected contents. `reference/` is excluded from Git and deployment. The files are not required to build the runtime or run the network tests.

The native headless harness accepts an extracted `libsandstorm2.so` and a data root containing `GloftBPHP/`:

```sh
cargo run --release --manifest-path engine/Cargo.toml --bin mc2cli -- /path/to/libsandstorm2.so /path/to/data-root 300
```

Browser testing loads the APK and data ZIP selected on the startup screen; they can also be selected from another directory.

Open `http://localhost:8080/` in a desktop browser with WebGL2, OffscreenCanvas, service workers, and browser storage. Add `?room=<name>` to connect to the local relay. The production site build defaults to the deployed Cloudflare relay; set `MC2_RELAY_URL` to an HTTPS or WSS URL ending in `/relay` to override it. Configure permitted site origins in `wrangler.multiplayer.jsonc` before deploying the Worker.

Game assets and the original Android binary remain the property of their respective owners. They are not included in the site build.
