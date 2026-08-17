// A deliberately capacity-limited HTTP API, for demonstrating Gust.
//
// Real services fall apart because a bounded resource (DB connection pool,
// worker threads) saturates: once arrivals exceed capacity, requests queue and
// latency stops tracking service time. This server reproduces that honestly —
// requests wait for a pool slot, so the knee it exhibits is genuine queueing,
// not an artificial sleep.
//
//   node examples/demo-api.js [--port 8080] [--pool 8] [--service-ms 10]
//
// Theoretical ceiling = pool / service_ms * 1000 req/s (default 8 / 10ms = 800).
// Measured capacity is ~10% lower (~720 req/s): Node's single-threaded event
// loop adds ~1ms of overhead per request on top of the service sleep, so the
// effective service time is ~11ms, not 10ms.
//
// Auth helpers for dogfooding Gust's --bearer / --basic-auth / --cookie-jar:
//   POST /login          → Set-Cookie: session=ok (body optional; accepts Basic)
//   GET  /api/me         → 200 if session cookie, Bearer demotoken, or Basic demo:demo
//   otherwise            → 401

const http = require("http");

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : Number(process.argv[i + 1]);
}

const PORT = arg("port", 8080);
const POOL = arg("pool", 8);
const SERVICE_MS = arg("service-ms", 10);

const BEARER = "demotoken";
const BASIC_USER = "demo";
const BASIC_PASS = "demo";

let available = POOL;
const waiters = [];

function acquire() {
  if (available > 0) {
    available -= 1;
    return Promise.resolve();
  }
  return new Promise((resolve) => waiters.push(resolve));
}

function release() {
  const next = waiters.shift();
  if (next) {
    next();
  } else {
    available += 1;
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
  });
}

function parseCookies(header) {
  const out = {};
  if (!header) return out;
  for (const part of header.split(";")) {
    const [k, ...rest] = part.trim().split("=");
    if (k) out[k] = rest.join("=");
  }
  return out;
}

function checkBasic(header) {
  if (!header || !header.startsWith("Basic ")) return false;
  try {
    const decoded = Buffer.from(header.slice(6), "base64").toString("utf8");
    const [user, pass] = decoded.split(":");
    return user === BASIC_USER && pass === BASIC_PASS;
  } catch {
    return false;
  }
}

function isAuthorized(req) {
  const auth = req.headers.authorization || "";
  if (auth === `Bearer ${BEARER}`) return true;
  if (checkBasic(auth)) return true;
  const cookies = parseCookies(req.headers.cookie);
  return cookies.session === "ok";
}

// Each endpoint holds a pool slot for a different amount of time, so a
// weighted scenario shows how a slow endpoint starves the cheap ones.
const ROUTES = {
  "/": 1,
  "/api/items": 1,
  "/api/search": 3,
  "/api/checkout": 5,
};

const server = http.createServer(async (req, res) => {
  const path = req.url.split("?")[0];

  if (path === "/login" && req.method === "POST") {
    await readBody(req);
    res.writeHead(200, {
      "content-type": "application/json",
      "set-cookie": "session=ok; Path=/; HttpOnly",
    });
    res.end('{"ok":true}');
    return;
  }

  if (path === "/api/me") {
    req.resume();
    if (!isAuthorized(req)) {
      res.writeHead(401, { "content-type": "application/json" });
      res.end('{"error":"unauthorized"}');
      return;
    }
    await acquire();
    try {
      await sleep(SERVICE_MS);
      res.writeHead(200, { "content-type": "application/json" });
      res.end('{"user":"demo"}');
    } finally {
      release();
    }
    return;
  }

  const cost = ROUTES[path];
  if (cost === undefined) {
    req.resume();
    res.writeHead(404, { "content-type": "application/json" });
    res.end('{"error":"not found"}');
    return;
  }

  // Drain request bodies so POST keepalive connections stay reusable.
  req.resume();

  await acquire();
  try {
    await sleep(cost * SERVICE_MS);
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ path, cost }));
  } finally {
    release();
  }
});

// Keep the accept queue deep enough that saturation shows up as latency
// (queueing) rather than immediate connection refusals.
server.maxConnections = 100_000;
server.keepAliveTimeout = 60_000;

server.listen(PORT, "127.0.0.1", () => {
  const ceiling = Math.round((POOL / SERVICE_MS) * 1000);
  const measured = Math.round((POOL / (SERVICE_MS + 1)) * 1000);
  console.log(`demo-api listening on http://127.0.0.1:${PORT}`);
  console.log(
    `pool=${POOL} service=${SERVICE_MS}ms → theoretical ceiling ${ceiling} req/s, ` +
      `measured knee ≈ ${measured} req/s (event-loop overhead)`,
  );
  console.log(`routes: ${Object.keys(ROUTES).join(", ")} (cost multiplier 1..5)`);
  console.log(
    `auth: POST /login → Set-Cookie; GET /api/me needs session cookie, Bearer ${BEARER}, or Basic ${BASIC_USER}:${BASIC_PASS}`,
  );
});
