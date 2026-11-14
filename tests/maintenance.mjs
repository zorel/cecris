// SIGTERM maintenance/shutdown test. Spawns its own server on 127.0.0.1:8098.
import { spawn } from "node:child_process";

const PORT = Number(process.env.MAINT_PORT || 8098);
const BASE = `http://127.0.0.1:${PORT}`;
const WS_BASE = `ws://127.0.0.1:${PORT}`;
const SERVER = process.env.SERVER || "./target/release/cecris";
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let passed = 0;

function ok(name, cond, detail = "") {
  if (!cond) {
    console.error("FAIL:", name, detail);
    process.exitCode = 1;
    throw new Error(`assertion failed: ${name} ${detail}`);
  }
  passed++;
  console.log("ok -", name);
}

async function waitFor(fn, timeoutMs, label) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (await fn()) return;
    await sleep(25);
  }
  throw new Error("timeout waiting for " + label);
}

function startServer() {
  const proc = spawn(SERVER, ["--bind", "127.0.0.1", "--port", String(PORT)], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  let log = "";
  proc.stdout.on("data", (chunk) => (log += chunk.toString()));
  proc.stderr.on("data", (chunk) => (log += chunk.toString()));
  return { proc, log: () => log };
}

async function waitHealthy(timeoutMs = 4000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(`${BASE}/healthz`);
      if (res.ok) return;
    } catch (_) {}
    await sleep(50);
  }
  throw new Error("server did not become healthy");
}

function waitExit(proc, timeoutMs) {
  return new Promise((resolve, reject) => {
    if (proc.exitCode !== null) return resolve(proc.exitCode);
    const timer = setTimeout(() => reject(new Error("server did not exit")), timeoutMs);
    proc.once("exit", (code) => {
      clearTimeout(timer);
      resolve(code);
    });
  });
}

async function status() {
  return (await fetch(`${BASE}/api/status`)).json();
}

async function expectJoinError(game, nick, token) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(
      `${WS_BASE}/api/ws?g=${encodeURIComponent(game)}&nick=${encodeURIComponent(nick)}&token=${encodeURIComponent(token)}`
    );
    ws.binaryType = "arraybuffer";
    let done = false;
    const timer = setTimeout(() => {
      if (done) return;
      done = true;
      try {
        ws.close();
      } catch (_) {}
      reject(new Error("no maintenance error frame"));
    }, 3000);
    ws.onmessage = (ev) => {
      if (done) return;
      const bytes = new Uint8Array(ev.data);
      if (bytes[0] !== 0x83) return;
      const event = JSON.parse(new TextDecoder().decode(bytes.subarray(1)));
      if (event.t !== "error") return;
      done = true;
      clearTimeout(timer);
      try {
        ws.close();
      } catch (_) {}
      resolve(event.msg || "");
    };
    ws.onclose = () => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      reject(new Error("closed without an error frame"));
    };
    ws.onerror = () => {};
  });
}

class Client {
  constructor(name) {
    this.name = name;
    this.lobby = null;
    this.states = [];
    this.events = [];
  }

  async connect(game, token) {
    this.ws = new WebSocket(
      `${WS_BASE}/api/ws?g=${encodeURIComponent(game)}&nick=${encodeURIComponent(this.name)}&token=${encodeURIComponent(token)}`
    );
    this.ws.binaryType = "arraybuffer";
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("ws open timeout")), 3000);
      this.ws.onopen = () => {
        clearTimeout(timer);
        resolve();
      };
      this.ws.onerror = () => {
        clearTimeout(timer);
        reject(new Error("ws error"));
      };
    });
    this.ws.onmessage = (ev) => {
      const bytes = new Uint8Array(ev.data);
      const text = () => new TextDecoder().decode(bytes.subarray(1));
      if (bytes[0] === 0x80) this.lobby = JSON.parse(text());
      else if (bytes[0] === 0x81) this.states.push(bytes);
      else if (bytes[0] === 0x83) this.events.push(JSON.parse(text()));
    };
    return this;
  }

  send(bytes) {
    this.ws.send(bytes);
  }

  close() {
    try {
      this.ws.close();
    } catch (_) {}
  }
}

const ready = () => Uint8Array.of(9, 1);
const start = () => Uint8Array.of(3);

async function main() {
  try {
    const probe = await fetch(`${BASE}/healthz`).catch(() => null);
    if (probe) throw new Error(`port ${PORT} already has a server`);

    // No games: SIGTERM exits immediately.
    {
      const server = startServer();
      try {
        await waitHealthy();
        server.proc.kill("SIGTERM");
        const code = await waitExit(server.proc, 4000);
        ok("exits immediately with no games", code === 0, `code=${code}\n${server.log()}`);
      } finally {
        if (server.proc.exitCode === null) server.proc.kill("SIGKILL");
      }
    }

    // Active game: stay up, reject new games, keep playing, then exit after the round.
    const server = startServer();
    const alice = new Client("alice");
    const bob = new Client("bob");
    try {
      await waitHealthy();
      await alice.connect("maint-room", "maint-token-a");
      await bob.connect("maint-room", "maint-token-b");
      await waitFor(
        () => alice.lobby && bob.lobby && alice.lobby.players.length === 2,
        4000,
        "lobby"
      );
      alice.send(ready());
      bob.send(ready());
      await waitFor(() => alice.lobby.players.every((p) => p.ready), 3000, "ready");
      alice.send(start());
      await waitFor(() => alice.lobby.phase === "playing", 6000, "playing");
      const before = await status();
      ok("game counted before shutdown", before.activeGames >= 1, JSON.stringify(before));

      server.proc.kill("SIGTERM");
      await sleep(300);
      ok("server stays up for the running game", server.proc.exitCode === null);

      const during = await status();
      ok("maintenance flag exposed", during.maintenance === true, JSON.stringify(during));
      ok("active game still counted", during.activeGames >= 1, JSON.stringify(during));

      const rejection = await expectJoinError("maint-new-room", "mallory", "maint-token-new");
      ok("new game rejected with maintenance", /maintenance/i.test(rejection), rejection);

      const stateCount = alice.states.length;
      await sleep(500);
      ok(
        "existing game keeps streaming",
        alice.states.length > stateCount,
        `${stateCount} -> ${alice.states.length}`
      );

      const deadline = Date.now() + 25000;
      while (server.proc.exitCode === null && Date.now() < deadline) {
        alice.send(Uint8Array.of(1, 5));
        bob.send(Uint8Array.of(1, 5));
        await sleep(200);
      }
      const code = await waitExit(server.proc, 5000);
      ok("shuts down after the last game ends", code === 0, `code=${code}\n${server.log()}`);
    } finally {
      alice.close();
      bob.close();
      if (server.proc.exitCode === null) server.proc.kill("SIGKILL");
    }

    console.log(`\n${passed} checks passed`);
  } catch (err) {
    console.error(err.stack || err);
    process.exitCode = 1;
  }
}

main();
