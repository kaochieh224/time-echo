// 網頁版的本機開發伺服器：node scripts/web-dev-server.mjs [port]
// 只是把 web/ 當靜態網站送出去（localhost 算安全來源，攝影機與 WebGPU 都能用）。

import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../web/', import.meta.url));
const port = Number(process.argv[2] ?? process.env.PORT ?? 8765);
const types = {
    '.html': 'text/html; charset=utf-8',
    '.js': 'text/javascript; charset=utf-8',
    '.mjs': 'text/javascript; charset=utf-8',
    '.css': 'text/css; charset=utf-8',
    '.json': 'application/json',
    '.wgsl': 'text/plain; charset=utf-8',
};

createServer(async (req, res) => {
    const path = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    const file = normalize(join(root, path.endsWith('/') ? path + 'index.html' : path));
    if (!file.startsWith(root)) {
        res.writeHead(403).end();
        return;
    }
    try {
        const body = await readFile(file);
        res.writeHead(200, { 'Content-Type': types[extname(file)] ?? 'application/octet-stream', 'Cache-Control': 'no-store' });
        res.end(body);
    } catch {
        res.writeHead(404).end('找不到');
    }
}).listen(port, () => console.log(`TIME ECHO 網頁版：http://localhost:${port}/`));
