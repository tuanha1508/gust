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
// Theoretical capacity = pool / service_ms * 1000 req/s
// (default: 8 / 10ms = ~800 req/s)

const http = require("http");

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : Number(process.argv[i + 1]);
}

const PORT = arg("port", 8080);
const POOL = arg("pool", 8);
const SERVICE_MS = arg("service-ms", 10);

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
  const cost = ROUTES[path];

  if (cost === undefined) {
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
  const capacity = Math.round((POOL / SERVICE_MS) * 1000);
  console.log(`demo-api listening on http://127.0.0.1:${PORT}`);
  console.log(`pool=${POOL} service=${SERVICE_MS}ms → capacity ≈ ${capacity} req/s for /`);
  console.log(`routes: ${Object.keys(ROUTES).join(", ")} (cost multiplier 1..5)`);
});
