// End-to-end protocol test: WebSocket room, SSE fallback, parallel games.
// Run against a live server: BASE=http://127.0.0.1:8099 node tests/e2e.mjs
import net from "node:net";

const BASE = process.env.BASE || "http://127.0.0.1:8099";
const WS_BASE = BASE.replace(/^http/, "ws");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let passed = 0;

function rawHeaders(path) {
  const url = new URL(BASE);
  return new Promise((resolve, reject) => {
    const sock = net.connect(Number(url.port || 80), url.hostname, () => {
      sock.write(`GET ${path} HTTP/1.1\r\nHost: ${url.host}\r\n\r\n`);
    });
    let data = "";
    const timer = setTimeout(() => {
      sock.destroy();
      reject(new Error("header timeout"));
    }, 3000);
    sock.on("data", (chunk) => {
      data += chunk.toString("latin1");
      const end = data.indexOf("\r\n\r\n");
      if (end >= 0) {
        clearTimeout(timer);
        const head = data.slice(0, end);
        sock.destroy();
        resolve(head);
      }
    });
    sock.on("error", (err) => {
      clearTimeout(timer);
      reject(err);
    });
  });
}

function ok(name, cond, detail = "") {
  if (!cond) {
    console.error("FAIL:", name, detail);
    process.exitCode = 1;
    throw new Error("assertion failed: " + name + " " + detail);
  }
  passed++;
  console.log("ok -", name);
}

async function waitFor(fn, timeoutMs, label) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (fn()) return;
    await sleep(25);
  }
  throw new Error("timeout waiting for " + label);
}

function decodeState(bytes) {
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let o = 1;
  const tick = dv.getUint32(o, true);
  o += 4;
  const slots = new Array(6).fill(null);
  for (let i = 0; i < 6; i++) {
    if (!bytes[o++]) continue;
    const alive = bytes[o++] === 1;
    const level = bytes[o++];
    const lines = dv.getUint16(o, true);
    o += 2;
    const kind = bytes[o++];
    const x = dv.getInt8(o++);
    const y = dv.getInt8(o++);
    const rot = bytes[o++];
    const next = bytes[o++];
    const n = bytes[o++];
    const inv = Array.from(bytes.subarray(o, o + n));
    o += n;
    slots[i] = { alive, level, lines, kind: kind === 255 ? null : kind, x, y, rot, next, inv };
  }
  return { tick, slots };
}

function decodeSnapshot(bytes) {
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let o = 5;
  const boards = [];
  for (let i = 0; i < 6; i++) {
    if (!bytes[o++]) continue;
    const version = dv.getUint32(o, true);
    o += 4;
    boards.push({ slot: i + 1, version, packed: bytes.subarray(o, o + 132).slice() });
    o += 132;
  }
  return boards;
}

class Client {
  constructor(name) {
    this.name = name;
    this.lobby = null;
    this.states = [];
    this.events = [];
    this.snapshots = [];
    this.boards = [];
    this.chat = [];
  }

  onFrame(bytes) {
    const type = bytes[0];
    const text = () => new TextDecoder().decode(bytes.subarray(1));
    if (type === 0x80) this.lobby = JSON.parse(text());
    else if (type === 0x81) this.states.push(decodeState(bytes));
    else if (type === 0x82) this.boards.push({ slot: bytes[1], version: new DataView(bytes.buffer, bytes.byteOffset).getUint32(2, true) });
    else if (type === 0x83) {
      const ev = JSON.parse(text());
      this.events.push(ev);
      if (ev.t === "chat") this.chat.push(ev);
    } else if (type === 0x84) this.snapshots.push(decodeSnapshot(bytes));
  }

  send(bytes) {
    this.transportSend(bytes);
  }

  close() {
    if (this.transportClose) this.transportClose();
  }
}

class WSClient extends Client {
  async connect(game) {
    const url = `${WS_BASE}/api/ws?g=${encodeURIComponent(game)}&nick=${encodeURIComponent(this.name)}&token=${encodeURIComponent("tok-" + this.name)}`;
    this.ws = new WebSocket(url);
    this.ws.binaryType = "arraybuffer";
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("ws open timeout " + this.name)), 4000);
      this.ws.onopen = () => {
        clearTimeout(timer);
        resolve();
      };
      this.ws.onerror = () => {
        clearTimeout(timer);
        reject(new Error("ws error " + this.name));
      };
    });
    this.ws.onmessage = (ev) => {
      if (ev.data instanceof ArrayBuffer) this.onFrame(new Uint8Array(ev.data));
    };
    this.transportSend = (bytes) => this.ws.send(bytes);
    this.transportClose = () => this.ws.close();
    return this;
  }
}

class SSEClient extends Client {
  async connect(game) {
    const controller = new AbortController();
    this.controller = controller;
    this.sid = null;
    const url = `${BASE}/api/sse?g=${encodeURIComponent(game)}&nick=${encodeURIComponent(this.name)}&token=${encodeURIComponent("tok-" + this.name)}`;
    const res = await fetch(url, { signal: controller.signal });
    ok("sse content-type", (res.headers.get("content-type") || "").includes("text/event-stream"));
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    this.readTask = (async () => {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        let idx;
        while ((idx = buf.indexOf("\n\n")) >= 0) {
          const block = buf.slice(0, idx);
          buf = buf.slice(idx + 2);
          let event = "message";
          const data = [];
          for (const line of block.split("\n")) {
            if (line.startsWith("event:")) event = line.slice(6).trim();
            else if (line.startsWith("data:")) data.push(line.slice(5).trim());
          }
          if (!data.length) continue;
          const payload = data.join("\n");
          if (event === "hello") {
            this.sid = JSON.parse(payload).sid;
          } else if (event === "bin") {
            this.onFrame(Uint8Array.from(Buffer.from(payload, "base64")));
          }
        }
      }
    })().catch(() => {});
    this.transportSend = (bytes) => {
      fetch(`${BASE}/api/input?sid=${encodeURIComponent(this.sid)}`, {
        method: "POST",
        headers: { "Content-Type": "application/octet-stream" },
        body: bytes,
      }).then((r) => {
        if (r.status !== 204) console.error("post input status", r.status);
      }).catch(() => {});
    };
    this.transportClose = () => {
      controller.abort();
    };
    await waitFor(() => this.sid, 4000, "sse hello sid");
    return this;
  }
}

function chatFrame(text) {
  const bytes = new TextEncoder().encode(text);
  const frame = new Uint8Array(1 + bytes.length);
  frame[0] = 4;
  frame.set(bytes, 1);
  return frame;
}

function readyFrame(ready = true) {
  return Uint8Array.of(9, ready ? 1 : 0);
}

function optionsFrame(o) {
  return Uint8Array.of(
    8,
    o.initialLevel,
    o.startingLines,
    o.linesPerLevel,
    o.levelIncrement,
    o.specialEvery,
    o.specialCount,
    o.specialCapacity,
    o.lineAttacks ? 1 : 0
  );
}

function rowsFromPacked(packed) {
  const rows = [];
  for (let y = 0; y < 22; y++) {
    let filled = 0;
    for (let x = 0; x < 12; x++) {
      const i = y * 12 + x;
      const byte = packed[i >> 1];
      const value = i & 1 ? byte & 0x0f : byte >> 4;
      if (value) filled++;
    }
    rows.push(filled);
  }
  return rows;
}

async function main() {
  const health = await fetch(`${BASE}/healthz`);
  ok("healthz", health.ok && (await health.text()) === "ok");
  const initialStatus = await (await fetch(`${BASE}/api/status`)).json();
  ok(
    "status endpoint",
    initialStatus.maintenance === false && initialStatus.activeGames === 0,
    JSON.stringify(initialStatus)
  );

  const index = await fetch(`${BASE}/g/probe-room`);
  const html = await index.text();
  ok("index served for invite path", index.ok && html.includes('<div id="app">'));
  const js = await fetch(`${BASE}/app.js`);
  ok("client js served", js.ok && (await js.text()).length > 1000);
  ok(
    "index versions assets",
    /app\.js\?v=[0-9a-f]{12}/.test(html) && /style\.css\?v=[0-9a-f]{12}/.test(html)
  );
  const indexHeaders = await rawHeaders("/");
  ok("index is not cached", /^cache-control: no-store/im.test(indexHeaders), indexHeaders);
  const jsHeaders = await rawHeaders("/app.js?v=probe");
  ok("client js is not cached", /^cache-control: no-store/im.test(jsHeaders), jsHeaders);

  const sseHeaders = await rawHeaders(`/api/sse?g=header-probe&nick=probe&token=probe-token`);
  ok("sse content type", /^content-type: text\/event-stream/im.test(sseHeaders));
  ok("sse has no Connection header", !/^connection:/im.test(sseHeaders), sseHeaders);
  ok("sse has no Content-Length header", !/^content-length:/im.test(sseHeaders), sseHeaders);
  ok("sse disables proxy buffering", /^x-accel-buffering: no/im.test(sseHeaders));
  ok("sse stays chunked", /^transfer-encoding: chunked/im.test(sseHeaders));

  const gameA = "e2e-ws-" + Date.now().toString(36);
  const a1 = await new WSClient("alpha").connect(gameA);
  const a2 = await new WSClient("bravo").connect(gameA);

  await waitFor(() => a1.lobby && a2.lobby && a1.lobby.players.length === 2, 4000, "lobby with 2 players");
  ok("host assigned", a1.lobby.host === 1 && a2.lobby.host === 1);
  ok("invite game id echoed", a1.lobby.game === gameA);
  ok("both players listed", a1.lobby.players.map((p) => p.nick).join(",") === "alpha,bravo");
  ok("players start unready", a1.lobby.players.every((p) => p.ready === false));

  a1.send(Uint8Array.of(3));
  await waitFor(
    () => a1.events.some((e) => e.t === "error" && /ready/i.test(e.msg)),
    2000,
    "start rejected before ready"
  );
  ok("start blocked until everyone is ready", true);

  a1.send(readyFrame());
  await waitFor(
    () => a1.lobby.players.find((p) => p.id === 1)?.ready === true,
    2000,
    "host ready"
  );
  const errorsBefore = a1.events.filter((e) => e.t === "error").length;
  a1.send(Uint8Array.of(3));
  await waitFor(
    () => a1.events.filter((e) => e.t === "error").length > errorsBefore,
    2000,
    "start rejected with one player unready"
  );
  ok("host alone is not enough", true);

  a2.send(readyFrame());
  await waitFor(
    () => a1.lobby.players.every((p) => p.ready) && a2.lobby.players.every((p) => p.ready),
    2000,
    "all players ready"
  );
  ok("readiness is shared", true);

  a1.send(Uint8Array.of(3));
  await waitFor(() => a1.lobby.phase === "countdown", 2000, "countdown");
  ok("countdown has millis", a1.lobby.cd > 0 && a1.lobby.cd <= 3000);
  await waitFor(() => a1.lobby.phase === "playing" && a2.lobby.phase === "playing", 6000, "playing phase");
  await waitFor(() => a1.snapshots.length >= 1, 2000, "start snapshot");
  ok("snapshot boards empty", a1.snapshots[0].every((b) => b.packed.every((x) => x === 0)));
  const playingStatus = await (await fetch(`${BASE}/api/status`)).json();
  ok("active games counted", playingStatus.activeGames >= 1, JSON.stringify(playingStatus));

  const before = a1.states.length;
  await sleep(700);
  ok("state frames flowing", a1.states.length - before >= 8, `got ${a1.states.length - before} in 700ms`);
  const lastState = a1.states[a1.states.length - 1];
  ok("two active boards in state", lastState.slots.filter(Boolean).length === 2);
  ok("piece positions in bounds", lastState.slots.filter(Boolean).every((p) => p.x >= 0 && p.x < 12 && p.y >= 0 && p.y < 22));

  a2.send(chatFrame("hello from bravo"));
  await waitFor(() => a1.chat.some((c) => c.text === "hello from bravo" && c.nick === "bravo"), 2000, "chat relay");
  ok("chat relayed", true);

  // Hard-drop until alpha tops out and the round ends.
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    if (a1.events.some((e) => e.t === "over")) break;
    a1.send(Uint8Array.of(1, 5));
    await sleep(180);
  }
  ok("round produced over event", a1.events.some((e) => e.t === "over"), JSON.stringify(a1.events.slice(-5)));
  ok("game over reported in lobby", a1.lobby.phase === "over");
  ok("board frames were sent", a1.boards.length > 0, "boards=" + a1.boards.length);

  // Parallel game: one WebSocket player, one SSE player.
  const gameB = "e2e-sse-" + Date.now().toString(36);
  const b1 = await new WSClient("charlie").connect(gameB);
  const b2 = await new SSEClient("delta").connect(gameB);
  await waitFor(() => b1.lobby && b2.lobby && b2.lobby.players.length === 2, 4000, "sse lobby");
  ok("sse player sees lobby", b2.lobby.players.some((p) => p.nick === "delta"));
  ok("sse sid obtained", typeof b2.sid === "string" && b2.sid.length > 8);

  b1.send(readyFrame());
  b2.send(readyFrame());
  await waitFor(
    () => b1.lobby.players.every((p) => p.ready) && b2.lobby.players.every((p) => p.ready),
    2500,
    "game B ready"
  );
  ok("sse player can ready up", true);

  b1.send(Uint8Array.of(3));
  await waitFor(() => b1.lobby.phase === "playing", 6000, "game B playing");
  await waitFor(() => b2.snapshots.length >= 1, 3000, "sse snapshot");
  const sseBefore = b2.states.length;
  await sleep(700);
  ok("sse receives state frames", b2.states.length - sseBefore >= 8, `got ${b2.states.length - sseBefore}`);

  b2.send(chatFrame("sse says hi"));
  await waitFor(() => b1.chat.some((c) => c.text === "sse says hi"), 2500, "sse chat relay");
  ok("sse post input relayed", true);

  b2.send(Uint8Array.of(1, 5));
  await waitFor(() => b2.boards.some((x) => x.slot === 2), 2500, "sse move applied");
  ok("sse hard drop produced board frame", true);

  // Host-editable options visible to joiners and applied to the round.
  const gameD = "e2e-options-" + Date.now().toString(36);
  const d1 = await new WSClient("lead").connect(gameD);
  const d2 = await new WSClient("guest").connect(gameD);
  await waitFor(() => d1.lobby && d2.lobby && d1.lobby.players.length === 2, 4000, "game D lobby");
  ok(
    "defaults published",
    d2.lobby.options &&
      d2.lobby.options.initialLevel === 1 &&
      d2.lobby.options.startingLines === 0 &&
      d2.lobby.options.linesPerLevel === 10 &&
      d2.lobby.options.specialEvery === 5 &&
      d2.lobby.options.specialCapacity === 5 &&
      d2.lobby.options.lineAttacks === true
  );

  d2.send(optionsFrame({ ...d2.lobby.options, initialLevel: 9, startingLines: 5 }));
  await sleep(300);
  ok(
    "non-host cannot change options",
    d1.lobby.options.initialLevel === 1 && d1.lobby.options.startingLines === 0
  );
  ok(
    "non-host gets an error",
    d2.events.some((e) => e.t === "error" && /host/i.test(e.msg))
  );

  const modified = {
    initialLevel: 5,
    startingLines: 3,
    linesPerLevel: 8,
    levelIncrement: 2,
    specialEvery: 2,
    specialCount: 2,
    specialCapacity: 7,
    lineAttacks: false,
  };
  d1.send(optionsFrame(modified));
  await waitFor(
    () =>
      d1.lobby.options.initialLevel === 5 &&
      d2.lobby.options.initialLevel === 5 &&
      d2.lobby.options.startingLines === 3,
    2500,
    "options broadcast"
  );
  ok(
    "joiner sees modified options",
    d2.lobby.options.lineAttacks === false &&
      d2.lobby.options.specialCapacity === 7 &&
      d2.lobby.options.levelIncrement === 2 &&
      d2.lobby.options.specialEvery === 2
  );

  d1.send(readyFrame());
  d2.send(readyFrame());
  await waitFor(() => d1.lobby.players.every((p) => p.ready), 2500, "game D ready");

  d1.send(Uint8Array.of(3));
  await waitFor(() => d1.lobby.phase === "playing", 6000, "game D playing");
  await waitFor(
    () => d1.snapshots.length >= 1 && d1.states.some((s) => s.slots[0] && s.slots[0].level === 5),
    3000,
    "game D state"
  );
  const snap = d1.snapshots[d1.snapshots.length - 1].find((b) => b.slot === 1);
  ok("snapshot board present", !!snap);
  const rows = rowsFromPacked(snap.packed);
  ok(
    "starting garbage applied",
    rows[19] === 11 && rows[20] === 11 && rows[21] === 11,
    JSON.stringify(rows.slice(16))
  );
  ok("rows above garbage empty", rows.slice(0, 19).every((n) => n === 0));

  // Reconnect into a running game with the same token.
  const gameC = "e2e-reconnect-" + Date.now().toString(36);
  const c1 = await new WSClient("hosty").connect(gameC);
  const c2 = await new WSClient("echo2").connect(gameC);
  await waitFor(() => c1.lobby && c2.lobby && c1.lobby.players.length === 2, 4000, "game C lobby");
  c1.send(readyFrame());
  c2.send(readyFrame());
  await waitFor(() => c1.lobby.players.every((p) => p.ready), 2500, "game C ready");
  c1.send(Uint8Array.of(3));
  await waitFor(() => c1.lobby.phase === "playing", 6000, "game C playing");
  await waitFor(() => c2.snapshots.length >= 1, 2000, "game C snapshot");
  c2.close();
  await sleep(250);
  const c2b = await new WSClient("echo2").connect(gameC);
  await waitFor(
    () => c2b.snapshots.length >= 1 && c2b.lobby && c2b.lobby.phase === "playing",
    4000,
    "reconnect snapshot"
  );
  ok("reconnect restores running game", c2b.snapshots[0].length >= 2);
  c2b.send(chatFrame("back online"));
  await waitFor(() => c1.chat.some((c) => c.text === "back online"), 2500, "reconnect chat");
  ok("reconnected player can act", true);

  const rooms = await (await fetch(`${BASE}/api/rooms`)).json();
  const ids = rooms.rooms.map((r) => r.id);
  ok("parallel rooms exist", ids.includes(gameA) && ids.includes(gameB), JSON.stringify(ids));

  a1.close();
  a2.close();
  b1.close();
  b2.close();
  c1.close();
  c2b.close();
  d1.close();
  d2.close();
  console.log(`\n${passed} checks passed`);
}

main().catch((err) => {
  console.error(err.stack || err);
  process.exitCode = 1;
});
