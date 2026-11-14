// Static file server for web/ with HTTP Range support (needed for on-demand
// reads of the large game archives).
//
// usage: node tools/serve.mjs [port] [--host address] [--port number]
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { attachRelay } from "./relay.mjs";

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..", "web");
const args = process.argv.slice(2);
let host = process.env.HOST || "127.0.0.1";
let portText = process.env.PORT || "8080";
for (let i = 0; i < args.length; i++) {
  const arg = args[i];
  if (arg === "--host") host = args[++i] || host;
  else if (arg.startsWith("--host=")) host = arg.slice("--host=".length) || host;
  else if (arg === "--port") portText = args[++i] || portText;
  else if (arg.startsWith("--port=")) portText = arg.slice("--port=".length) || portText;
  else if (!arg.startsWith("-")) portText = arg;
}
const port = Number(portText);
if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error(`Invalid port: ${portText}`);
const types = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".wasm": "application/wasm", ".json": "application/json", ".ogg": "audio/ogg", ".mp4": "video/mp4", ".css": "text/css" };

const server = http.createServer((req, res) => {
    if (!["GET", "HEAD"].includes(req.method)) return res.writeHead(405, { Allow: "GET, HEAD" }).end();
    let p;
    try { p = decodeURIComponent(new URL(req.url, "http://x").pathname); }
    catch { return res.writeHead(400).end("bad path"); }
    if (p.includes("\0")) return res.writeHead(400).end("bad path");
    if (p.endsWith("/")) p += "index.html";
    const vendor = /^\/vendor\/(ffmpeg|ffmpeg-core)\/(.+)$/.exec(p);
    const base = vendor ? path.resolve(root, "../node_modules/@ffmpeg", vendor[1] === "ffmpeg" ? "ffmpeg/dist/esm" : "core/dist/esm") : root;
    const file = path.join(base, path.normalize(vendor ? vendor[2] : p));
    if (!file.startsWith(base + path.sep)) return res.writeHead(403).end();
    fs.stat(file, (err, st) => {
      if (err || !st.isFile()) return res.writeHead(404).end("not found");
      const headers = { "Content-Type": types[path.extname(file)] || "application/octet-stream", "Accept-Ranges": "bytes", "Cache-Control": "no-cache" };
      const m = /^bytes=(\d+)-(\d*)$/.exec(req.headers.range || "");
      if (m) {
        const start = parseInt(m[1], 10);
        const end = m[2] ? Math.min(parseInt(m[2], 10), st.size - 1) : st.size - 1;
        if (start >= st.size || start > end) return res.writeHead(416, { "Content-Range": `bytes */${st.size}` }).end();
        res.writeHead(206, { ...headers, "Content-Range": `bytes ${start}-${end}/${st.size}`, "Content-Length": end - start + 1 });
        if (req.method === "HEAD") return res.end();
        fs.createReadStream(file, { start, end }).pipe(res);
      } else {
        res.writeHead(200, { ...headers, "Content-Length": st.size });
        if (req.method === "HEAD") return res.end();
        fs.createReadStream(file).pipe(res);
      }
    });
  });
attachRelay(server);
server.listen(port, host, () => console.log(`serving ${root} and room relay on http://${host}:${port}/`));
