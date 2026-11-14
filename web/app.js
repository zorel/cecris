"use strict";

(() => {
  const W = 12;
  const H = 22;
  const CELLS = W * H;
  const ZERO_BOARD = new Uint8Array(CELLS);

  const PIECE_STRS = [
    ["1111/0000/0000/0000", "0010/0010/0010/0010"],
    ["1100/1100/0000/0000"],
    ["0010/0010/0110/0000", "0100/0111/0000/0000", "0110/0100/0100/0000", "0111/0001/0000/0000"],
    ["0100/0100/0110/0000", "0111/0100/0000/0000", "0110/0010/0010/0000", "0001/0111/0000/0000"],
    ["0010/0110/0100/0000", "0110/0011/0000/0000"],
    ["0100/0110/0010/0000", "0011/0110/0000/0000"],
    ["0010/0110/0010/0000", "0010/0111/0000/0000", "0100/0110/0100/0000", "0111/0010/0000/0000"],
  ];

  const PIECE_DEFS = PIECE_STRS.map((rotations) =>
    rotations.map((shape) => {
      const cells = [];
      shape.split("/").forEach((row, y) => {
        [...row].forEach((v, x) => {
          if (v !== "0") cells.push([x, y]);
        });
      });
      return cells;
    })
  );

  const BLOCK_COLORS = ["", "#22d3ee", "#facc15", "#c084fc", "#fb923c", "#4ade80", "#f87171"];
  const SPECIALS = [
    { letter: "A", name: "Add Line", color: "#ef4444" },
    { letter: "C", name: "Clear Line", color: "#22c55e" },
    { letter: "N", name: "Nuke Field", color: "#a855f7" },
    { letter: "R", name: "Clear Random", color: "#f97316" },
    { letter: "S", name: "Switch Fields", color: "#06b6d4" },
    { letter: "B", name: "Clear Specials", color: "#3b82f6" },
    { letter: "G", name: "Block Gravity", color: "#84cc16" },
    { letter: "Q", name: "Blockquake", color: "#eab308" },
    { letter: "O", name: "Block Bomb", color: "#f43f5e" },
  ];

  const DEFAULT_OPTIONS = {
    initialLevel: 1,
    startingLines: 0,
    linesPerLevel: 10,
    levelIncrement: 1,
    specialEvery: 5,
    specialCount: 1,
    specialCapacity: 5,
    lineAttacks: true,
  };

  const app = document.getElementById("app");
  const $ = (id) => document.getElementById(id);

  const state = {
    gameId: "",
    nick: "",
    token: "",
    you: 0,
    phase: "lobby",
    players: [],
    host: 0,
    winner: 0,
    cd: 0,
    options: Object.assign({}, DEFAULT_OPTIONS),
    transport: "offline",
    ws: null,
    es: null,
    sid: null,
    pending: [],
    reconnectTimer: null,
    retry: 0,
    game: {
      slots: new Array(6).fill(null),
      boards: new Array(6).fill(null),
      versions: new Array(6).fill(-1),
      tick: 0,
    },
    pred: null,
    lastInputAt: 0,
    selected: null,
    pingAt: 0,
    pingTimer: null,
    log: [],
    dirty: true,
    roomBuilt: false,
    oppSig: null,
    specSig: null,
    playerSig: null,
    opp: {},
    keys: { left: false, right: false, down: false, nextLeft: 0, nextRight: 0, nextDown: 0 },
    forceSse: false,
    maintenance: false,
    statusTimer: null,
  };

  const ui = {};

  // ---------------------------------------------------------------- utilities

  function esc(s) {
    return String(s).replace(/[&<>"']/g, (c) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    }[c]));
  }

  function markDirty() {
    state.dirty = true;
  }

  function sanitizeGameId(raw) {
    return String(raw || "").trim().toLowerCase().replace(/[^a-z0-9_-]/g, "").slice(0, 32);
  }

  function sanitizeNick(raw) {
    const s = String(raw || "").replace(/[\u0000-\u001f<>"'&]/g, "").trim().slice(0, 16);
    return s || "player";
  }

  async function fetchStatus() {
    const res = await fetch("/api/status", { cache: "no-store" });
    if (!res.ok) return null;
    return res.json();
  }

  function stopStatusPolling() {
    if (state.statusTimer) {
      clearInterval(state.statusTimer);
      state.statusTimer = null;
    }
  }

  async function pollStatus() {
    const countEl = $("liveCount");
    if (!countEl) {
      stopStatusPolling();
      return;
    }
    try {
      const data = await fetchStatus();
      if (!data) throw new Error("status unavailable");
      state.maintenance = !!data.maintenance;
      const games = data.activeGames || 0;
      countEl.textContent =
        games === 0 ? "no games in progress" :
        games === 1 ? "1 game in progress" :
        `${games} games in progress`;
      const maintenanceEl = $("liveMaintenance");
      if (maintenanceEl) maintenanceEl.classList.toggle("hidden", !state.maintenance);
      const submit = $("landingSubmit");
      if (submit) submit.disabled = state.maintenance;
    } catch (_) {
      countEl.textContent = "server unreachable";
    }
  }

  function startStatusPolling() {
    stopStatusPolling();
    pollStatus();
    state.statusTimer = setInterval(pollStatus, 2000);
  }

  function tokenKey(game) {
    return "cecris_token_" + game;
  }

  function getToken(game) {
    let t = sessionStorage.getItem(tokenKey(game));
    if (!t) {
      // Tokens are tab-scoped: two tabs must join as two different players.
      // An old shared localStorage token is deliberately not reused.
      t = makeToken();
      sessionStorage.setItem(tokenKey(game), t);
    }
    localStorage.removeItem(tokenKey(game));
    return t;
  }

  function makeToken() {
    if (window.crypto && crypto.randomUUID) return crypto.randomUUID().replace(/-/g, "").slice(0, 24);
    const bytes = new Uint8Array(12);
    if (window.crypto && crypto.getRandomValues) crypto.getRandomValues(bytes);
    else for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
    return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
  }

  function b64ToBytes(text) {
    const bin = atob(text);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  function mySlot() {
    return state.game.slots[state.you - 1] || null;
  }

  function nickOf(id) {
    const p = state.players.find((p) => p.id === id);
    return p ? p.nick : "P" + id;
  }

  function connectedCount() {
    return state.players.filter((p) => p.connected).length;
  }

  function aliveCount() {
    return state.game.slots.filter((s) => s && s.present && s.alive).length;
  }

  // ------------------------------------------------------------------ routing

  window.addEventListener("popstate", () => location.reload());

  function boot() {
    const m = location.pathname.match(/^\/g\/([^/]+)\/?$/);
    if (!m) {
      renderLanding();
      return;
    }
    const gameId = sanitizeGameId(decodeURIComponent(m[1]));
    if (!gameId) {
      history.replaceState(null, "", "/");
      renderLanding();
      return;
    }
    state.gameId = gameId;
    state.nick = localStorage.getItem("cecris_nick") || "";
    state.token = getToken(gameId);
    state.forceSse = new URLSearchParams(location.search).get("transport") === "sse";
    if (!state.nick) renderJoinPrompt();
    else {
      buildRoom();
      connect();
    }
  }

  function renderLanding() {
    document.title = "cecris";
    app.innerHTML = `
      <div class="landing">
        <div class="landing-card">
          <div class="logo">CE<span>CRIS</span></div>
          <p class="tag">Real-time multiplayer block battle. 2–6 players per room, special attacks, invite links.</p>
          <div class="live-status">
            <span class="live-dot"></span>
            <span id="liveCount">checking server…</span>
            <span id="liveMaintenance" class="live-maintenance hidden">maintenance — new games paused</span>
          </div>
          <form id="landingForm">
            <label>Nickname
              <input id="landingNick" maxlength="16" autocomplete="nickname" placeholder="your name">
            </label>
            <label>Game code
              <input id="landingGame" maxlength="32" autocomplete="off" placeholder="e.g. friday-night" pattern="[A-Za-z0-9_-]{1,32}">
            </label>
            <div class="row">
              <button class="primary" type="submit" id="landingSubmit">Enter room</button>
              <button type="button" id="randomGame" class="ghost">Random code</button>
            </div>
          </form>
          <p class="fine">Same game code = same room. Share the invite link to play together.</p>
        </div>
      </div>`;

    $("landingNick").value = localStorage.getItem("cecris_nick") || "";
    $("randomGame").addEventListener("click", () => {
      const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
      const bytes = new Uint8Array(6);
      (window.crypto || {}).getRandomValues
        ? crypto.getRandomValues(bytes)
        : bytes.forEach((_, i) => (bytes[i] = Math.floor(Math.random() * 256)));
      $("landingGame").value = [...bytes].map((b) => alphabet[b % alphabet.length]).join("");
    });
    $("landingForm").addEventListener("submit", (e) => {
      e.preventDefault();
      if (state.maintenance) return;
      const nick = sanitizeNick($("landingNick").value);
      const game = sanitizeGameId($("landingGame").value);
      if (!game) {
        $("landingGame").focus();
        return;
      }
      if (!localStorage.getItem("cecris_nick")) localStorage.setItem("cecris_nick", nick);
      else if (nick) localStorage.setItem("cecris_nick", nick);
      location.href = "/g/" + encodeURIComponent(game);
    });
    startStatusPolling();
  }

  function renderJoinPrompt() {
    stopStatusPolling();
    document.title = "Join " + state.gameId + " · cecris";
    app.innerHTML = `
      <div class="landing">
        <div class="landing-card">
          <div class="logo">CE<span>CRIS</span></div>
          <p class="tag">Joining room <b class="accent">${esc(state.gameId)}</b></p>
          <form id="nickForm">
            <label>Nickname
              <input id="nickInput" maxlength="16" autocomplete="nickname" placeholder="your name">
            </label>
            <button class="primary" type="submit">Enter room</button>
          </form>
        </div>
      </div>`;
    const input = $("nickInput");
    input.focus();
    $("nickForm").addEventListener("submit", (e) => {
      e.preventDefault();
      const nick = sanitizeNick(input.value);
      localStorage.setItem("cecris_nick", nick);
      state.nick = nick;
      buildRoom();
      connect();
    });
  }

  // ------------------------------------------------------------------ room UI

  function buildRoom() {
    if (state.roomBuilt) return;
    stopStatusPolling();
    state.roomBuilt = true;
    document.title = state.gameId + " · cecris";
    app.innerHTML = `
      <div class="room">
        <header class="topbar">
          <a class="logo small" href="/">CE<span>CRIS</span></a>
          <div class="room-tag">
            <span>room <b id="roomIdLabel"></b></span>
            <button id="copyBtn" class="ghost small-btn" type="button">copy invite</button>
            <a class="ghost small-btn" href="/">leave</a>
          </div>
          <div class="status">
            <span id="transport" class="badge">offline</span>
            <span id="ping" class="badge">-- ms</span>
          </div>
        </header>
        <section id="lobbyView" class="lobby">
          <div class="panel lobby-panel">
            <div class="lobby-head">
              <h2 id="lobbyTitle">Lobby</h2>
              <p id="lobbySub">connecting…</p>
            </div>
            <div class="invite-row">
              <input id="inviteLink" readonly>
              <button id="inviteCopy" class="primary" type="button">Copy</button>
            </div>
            <div class="instructions">
              <strong>How to play</strong>
              <p>Clear full rows to stay alive. Lettered blocks are specials: clear the row
              containing one to collect it, then select it from your inventory and click a
              field to use it. The last player able to place pieces wins.</p>
              <p class="keys">←/→ move · ↑/X rotate · Z rotate back · ↓ soft drop · Space hard drop · 1–5 special</p>
            </div>
            <details id="optionsPanel" class="options-panel">
              <summary>Game options <span id="optionsHint"></span></summary>
              <div class="options-grid">
                <label>Start level<input id="optInitial" type="number" min="1" max="15" step="1"></label>
                <label>Garbage rows<input id="optGarbage" type="number" min="0" max="12" step="1"></label>
                <label>Lines per level<input id="optPerLevel" type="number" min="1" max="50" step="1"></label>
                <label>Level step<input id="optLevelStep" type="number" min="1" max="5" step="1"></label>
                <label>Special every N lines<input id="optSpecialEvery" type="number" min="0" max="50" step="1"></label>
                <label>Specials per trigger<input id="optSpecialCount" type="number" min="0" max="5" step="1"></label>
                <label>Inventory size<input id="optCapacity" type="number" min="1" max="8" step="1"></label>
                <label class="check wide"><input id="optAttacks" type="checkbox"> Send garbage on 2/3/4-line clears</label>
              </div>
            </details>
            <ul id="playerList" class="player-list"></ul>
            <div class="lobby-actions">
              <button id="readyBtn" class="ghost" type="button">Ready</button>
              <button id="startBtn" class="primary" type="button" disabled>Start game</button>
              <span id="lobbyHint"></span>
            </div>
          </div>
        </section>
        <section id="gameView" class="game hidden">
          <div class="own-col">
            <div class="panel board-panel">
              <div class="board-wrap" id="ownWrap"><canvas id="ownBoard"></canvas></div>
              <div class="own-side">
                <div class="mini-block"><span class="label">next</span><canvas id="nextPiece"></canvas></div>
                <div class="mini-block"><span class="label">stats</span><div id="stats" class="stats"></div></div>
                <div class="mini-block grow"><span class="label">specials</span><div id="specials" class="specials"></div></div>
              </div>
            </div>
            <div id="hint" class="hint"></div>
          </div>
          <div class="opp-col">
            <div id="rulesSummary" class="rules-summary"></div>
            <div id="banner" class="banner hidden"></div>
            <div id="opponents" class="opponents"></div>
          </div>
          <aside class="side-col">
            <div class="panel log-panel"><div id="log" class="log"></div></div>
            <form id="chatForm" class="chat-form">
              <input id="chatInput" maxlength="200" autocomplete="off" placeholder="chat…">
              <button class="primary" type="submit">send</button>
            </form>
          </aside>
        </section>
        <div id="overlay" class="overlay hidden"></div>
      </div>`;

    ui.roomIdLabel = $("roomIdLabel");
    ui.transport = $("transport");
    ui.ping = $("ping");
    ui.lobbyView = $("lobbyView");
    ui.gameView = $("gameView");
    ui.lobbyTitle = $("lobbyTitle");
    ui.lobbySub = $("lobbySub");
    ui.inviteLink = $("inviteLink");
    ui.playerList = $("playerList");
    ui.startBtn = $("startBtn");
    ui.readyBtn = $("readyBtn");
    ui.lobbyHint = $("lobbyHint");
    ui.optionsPanel = $("optionsPanel");
    ui.optionsHint = $("optionsHint");
    ui.optInitial = $("optInitial");
    ui.optGarbage = $("optGarbage");
    ui.optPerLevel = $("optPerLevel");
    ui.optLevelStep = $("optLevelStep");
    ui.optSpecialEvery = $("optSpecialEvery");
    ui.optSpecialCount = $("optSpecialCount");
    ui.optCapacity = $("optCapacity");
    ui.optAttacks = $("optAttacks");
    ui.rulesSummary = $("rulesSummary");
    ui.ownBoard = $("ownBoard");
    ui.ownWrap = $("ownWrap");
    ui.nextPiece = $("nextPiece");
    ui.stats = $("stats");
    ui.specials = $("specials");
    ui.hint = $("hint");
    ui.opponents = $("opponents");
    ui.banner = $("banner");
    ui.log = $("log");
    ui.overlay = $("overlay");
    ui.chatInput = $("chatInput");

    ui.roomIdLabel.textContent = state.gameId;
    ui.inviteLink.value = location.origin + "/g/" + state.gameId;
    ui.ownBoard.width = 288;
    ui.ownBoard.height = 528;
    ui.nextPiece.width = 88;
    ui.nextPiece.height = 88;

    $("copyBtn").addEventListener("click", copyInvite);
    $("inviteCopy").addEventListener("click", copyInvite);
    ui.startBtn.addEventListener("click", () => send(Uint8Array.of(3)));
    ui.readyBtn.addEventListener("click", toggleReady);
    ui.ownWrap.addEventListener("click", () => useSpecialOn(state.you));
    for (const el of [
      ui.optInitial,
      ui.optGarbage,
      ui.optPerLevel,
      ui.optLevelStep,
      ui.optSpecialEvery,
      ui.optSpecialCount,
      ui.optCapacity,
      ui.optAttacks,
    ]) {
      el.addEventListener("change", sendOptions);
    }

    ui.specials.addEventListener("click", (e) => {
      const btn = e.target.closest(".sp");
      if (!btn) return;
      const idx = Number(btn.dataset.i);
      state.selected = state.selected === idx ? null : idx;
      markDirty();
    });

    $("chatForm").addEventListener("submit", (e) => {
      e.preventDefault();
      const text = ui.chatInput.value.trim();
      if (!text) return;
      const bytes = new TextEncoder().encode(text);
      const frame = new Uint8Array(1 + bytes.length);
      frame[0] = 4;
      frame.set(bytes, 1);
      send(frame);
      ui.chatInput.value = "";
    });

    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", () => {
      state.keys.left = state.keys.right = state.keys.down = false;
    });
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible") send(Uint8Array.of(7));
    });
    state.pingTimer = setInterval(ping, 2000);
    requestAnimationFrame(frameLoop);
    renderLobby();
  }

  function copyInvite() {
    const link = location.origin + "/g/" + state.gameId;
    const done = () => {
      const btn = $("inviteCopy");
      const old = btn.textContent;
      btn.textContent = "Copied";
      setTimeout(() => (btn.textContent = old), 1200);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(link).then(done).catch(() => fallbackCopy(link));
    } else {
      fallbackCopy(link);
    }
  }

  function fallbackCopy(text) {
    ui.inviteLink.value = text;
    ui.inviteLink.select();
    try {
      document.execCommand("copy");
    } catch (_) {}
  }

  // --------------------------------------------------------------- networking

  function setTransport(kind) {
    state.transport = kind;
    if (!ui.transport) return;
    ui.transport.textContent = kind === "ws" ? "WS" : kind === "sse" ? "SSE" : kind === "connecting" ? "…" : "OFF";
    ui.transport.className = "badge " + (kind === "ws" ? "ok" : kind === "sse" ? "warn" : kind === "offline" ? "bad" : "");
  }

  async function connect() {
    clearTimeout(state.reconnectTimer);
    state.reconnectTimer = null;
    closeTransports();
    const status = await fetchStatus().catch(() => null);
    if (status && status.maintenance) {
      showMaintenance();
      return;
    }
    state.maintenance = false;
    if (state.forceSse) startSSE();
    else tryWebSocket();
  }

  function showMaintenance() {
    state.maintenance = true;
    closeTransports();
    setTransport("offline");
    if (!ui.overlay) return;
    ui.overlay.classList.remove("hidden");
    ui.overlay.innerHTML = `
      <div class="overlay-card">
        <div class="eyebrow">maintenance</div>
        <h2>New games paused</h2>
        <p>The server is waiting for running rounds to finish. Reload later to play.</p>
      </div>`;
  }

  function closeTransports() {
    if (state.ws) {
      const ws = state.ws;
      state.ws = null;
      try {
        ws.close();
      } catch (_) {}
    }
    if (state.es) {
      const es = state.es;
      state.es = null;
      try {
        es.close();
      } catch (_) {}
    }
    state.sid = null;
  }

  function tryWebSocket() {
    if (state.ws || state.es) return;
    setTransport("connecting");
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    const url =
      `${scheme}://${location.host}/api/ws?g=${encodeURIComponent(state.gameId)}` +
      `&nick=${encodeURIComponent(state.nick)}&token=${encodeURIComponent(state.token)}`;
    let ws;
    try {
      ws = new WebSocket(url);
    } catch (_) {
      void handleTransportFailure(false);
      return;
    }
    ws.binaryType = "arraybuffer";
    state.ws = ws;
    let opened = false;
    const timer = setTimeout(() => {
      if (!opened) {
        try {
          ws.close();
        } catch (_) {}
      }
    }, 2500);

    ws.onopen = () => {
      opened = true;
      clearTimeout(timer);
      state.retry = 0;
      setTransport("ws");
      flushPending();
    };
    ws.onmessage = (ev) => {
      if (ev.data instanceof ArrayBuffer) handleFrame(new Uint8Array(ev.data));
    };
    ws.onclose = () => {
      clearTimeout(timer);
      if (state.ws !== ws) return;
      state.ws = null;
      if (opened) {
        setTransport("offline");
        scheduleReconnect(700);
      } else {
        void handleTransportFailure(false);
      }
    };
    ws.onerror = () => {};
  }

  function startSSE() {
    if (state.es) return;
    setTransport("connecting");
    const url =
      `/api/sse?g=${encodeURIComponent(state.gameId)}` +
      `&nick=${encodeURIComponent(state.nick)}&token=${encodeURIComponent(state.token)}`;
    let es;
    try {
      es = new EventSource(url);
    } catch (_) {
      scheduleReconnect(1500);
      return;
    }
    state.es = es;
    es.addEventListener("hello", (ev) => {
      try {
        const data = JSON.parse(ev.data);
        state.sid = data.sid;
      } catch (_) {}
      state.retry = 0;
      setTransport("sse");
      flushPending();
    });
    es.addEventListener("bin", (ev) => {
      try {
        handleFrame(b64ToBytes(ev.data));
      } catch (_) {}
    });
    es.addEventListener("fatal", (ev) => {
      pushLog("error: " + ev.data, "err");
    });
    es.addEventListener("error", () => {
      if (state.es !== es) return;
      try {
        es.close();
      } catch (_) {}
      state.es = null;
      state.sid = null;
      if (state.transport === "sse") setTransport("offline");
      void handleTransportFailure(true);
    });
  }

  async function handleTransportFailure(fromSse) {
    const status = await fetchStatus().catch(() => null);
    if (status && status.maintenance) {
      showMaintenance();
      return;
    }
    if (!fromSse && !state.forceSse && state.retry === 0) startSSE();
    else scheduleReconnect(1500);
  }

  function scheduleReconnect(delay) {
    if (state.reconnectTimer) return;
    const wait = Math.min(6000, Math.round(delay * Math.pow(1.5, state.retry)));
    state.retry = Math.min(state.retry + 1, 6);
    state.reconnectTimer = setTimeout(() => {
      state.reconnectTimer = null;
      void connect();
    }, wait);
  }

  function flushPending() {
    const pending = state.pending;
    state.pending = [];
    for (const frame of pending) send(frame);
  }

  function send(bytes) {
    if (state.ws && state.ws.readyState === WebSocket.OPEN) {
      state.ws.send(bytes);
      return;
    }
    if (state.sid) {
      postInput(bytes);
      return;
    }
    if (state.pending.length < 128) state.pending.push(bytes);
  }

  function postInput(bytes) {
    fetch(`/api/input?sid=${encodeURIComponent(state.sid)}`, {
      method: "POST",
      headers: { "Content-Type": "application/octet-stream" },
      body: bytes,
      keepalive: true,
    }).catch(() => {});
  }

  function ping() {
    if (state.transport !== "ws" && state.transport !== "sse") return;
    const frame = new Uint8Array(9);
    frame[0] = 5;
    new DataView(frame.buffer).setFloat64(1, performance.now(), true);
    state.pingAt = performance.now();
    send(frame);
  }

  // ----------------------------------------------------------------- protocol

  function handleFrame(bytes) {
    if (!bytes || bytes.length === 0) return;
    const type = bytes[0];
    const text = () => new TextDecoder().decode(bytes.subarray(1));
    switch (type) {
      case 0x86: {
        const data = JSON.parse(text());
        state.you = data.you;
        state.token = data.token;
        sessionStorage.setItem(tokenKey(state.gameId), data.token);
        break;
      }
      case 0x80: {
        applyLobby(JSON.parse(text()));
        break;
      }
      case 0x81:
        decodeState(bytes);
        break;
      case 0x82:
        decodeBoard(bytes);
        break;
      case 0x83:
        applyEvent(JSON.parse(text()));
        break;
      case 0x84:
        decodeSnapshot(bytes);
        break;
      case 0x85: {
        const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        if (bytes.length >= 9) {
          const rtt = Math.max(0, performance.now() - dv.getFloat64(1, true));
          if (ui.ping) ui.ping.textContent = Math.round(rtt) + " ms";
        }
        break;
      }
      default:
        break;
    }
  }

  function decodeState(bytes) {
    const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    let o = 1;
    const tick = dv.getUint32(o, true);
    o += 4;
    const slots = new Array(6).fill(null);
    for (let i = 0; i < 6; i++) {
      const present = bytes[o++];
      if (!present) continue;
      const alive = bytes[o++] === 1;
      const level = bytes[o++];
      const lines = dv.getUint16(o, true);
      o += 2;
      const kindRaw = bytes[o++];
      const x = dv.getInt8(o++);
      const y = dv.getInt8(o++);
      const rot = bytes[o++];
      const next = bytes[o++];
      const n = bytes[o++];
      const inv = Array.from(bytes.subarray(o, o + n));
      o += n;
      slots[i] = {
        present: true,
        alive,
        level,
        lines,
        kind: kindRaw === 255 ? null : kindRaw,
        x,
        y,
        rot,
        next,
        inv,
      };
    }
    state.game.tick = tick;
    state.game.slots = slots;
    const me = slots[state.you - 1];
    if (!me || !me.alive || me.kind == null) {
      state.pred = null;
    } else if (state.pred && state.pred.kind === me.kind && performance.now() - state.lastInputAt < 180) {
      if (me.y > state.pred.y) state.pred.y = me.y;
    } else {
      state.pred = { kind: me.kind, x: me.x, y: me.y, rot: me.rot };
    }
    markDirty();
  }

  function decodeBoard(bytes) {
    if (bytes.length < 6) return;
    const slot = bytes[1];
    if (slot < 1 || slot > 6) return;
    const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    state.game.versions[slot - 1] = dv.getUint32(2, true);
    state.game.boards[slot - 1] = unpackBoard(bytes.subarray(6));
    markDirty();
  }

  function decodeSnapshot(bytes) {
    const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    let o = 1;
    state.game.tick = dv.getUint32(o, true);
    o += 4;
    for (let i = 0; i < 6; i++) {
      const present = bytes[o++];
      if (!present) {
        state.game.boards[i] = null;
        state.game.versions[i] = -1;
        continue;
      }
      state.game.versions[i] = dv.getUint32(o, true);
      o += 4;
      state.game.boards[i] = unpackBoard(bytes.subarray(o, o + 132));
      o += 132;
    }
    markDirty();
  }

  function unpackBoard(packed) {
    const out = new Uint8Array(CELLS);
    for (let i = 0; i < packed.length; i++) {
      out[i * 2] = packed[i] >> 4;
      out[i * 2 + 1] = packed[i] & 0x0f;
    }
    return out;
  }

  function applyLobby(data) {
    const prev = state.phase;
    state.phase = data.phase;
    state.players = data.players || [];
    state.host = data.host || 0;
    state.winner = data.winner || 0;
    state.cd = data.cd || 0;
    state.maintenance = !!data.maintenance;
    state.options = Object.assign({}, DEFAULT_OPTIONS, data.options || {});

    if (data.phase === "lobby" || data.phase === "countdown") {
      if (prev !== "lobby" && prev !== "countdown") resetGame();
      ui.lobbyView.classList.remove("hidden");
      ui.gameView.classList.add("hidden");
      ui.overlay.classList.add("hidden");
    } else {
      ui.lobbyView.classList.add("hidden");
      ui.gameView.classList.remove("hidden");
      if (prev !== "playing" && prev !== "over") {
        ui.banner.classList.add("hidden");
      }
    }
    renderLobby();
    updateOverlay();
    markDirty();
  }

  function applyEvent(ev) {
    switch (ev.t) {
      case "chat":
        pushLog(`${ev.nick}: ${ev.text}`, "chat");
        break;
      case "lines":
        pushLog(`${nickOf(ev.p)} cleared ${ev.n} line${ev.n > 1 ? "s" : ""}${ev.sp ? ` (+${ev.sp} special${ev.sp > 1 ? "s" : ""})` : ""}`, "line");
        break;
      case "atk":
        pushLog(`${nickOf(ev.from)} attacks ${nickOf(ev.to)} with ${ev.n} line${ev.n > 1 ? "s" : ""}`, "atk");
        break;
      case "sp": {
        const sp = SPECIALS[ev.sp];
        pushLog(`${nickOf(ev.from)} used ${sp ? sp.name : "a special"} on ${nickOf(ev.to)}`, "sp");
        break;
      }
      case "lost":
        pushLog(`${nickOf(ev.p)} topped out`, "lost");
        break;
      case "over":
        pushLog(ev.winner ? `${nickOf(ev.winner)} wins the round` : "round over — draw", "over");
        break;
      case "error":
        state.retry = Math.max(state.retry, 4);
        pushLog(ev.msg, "err");
        break;
      default:
        break;
    }
  }

  function resetGame() {
    state.game.slots = new Array(6).fill(null);
    state.game.boards = new Array(6).fill(null);
    state.game.versions = new Array(6).fill(-1);
    state.game.tick = 0;
    state.pred = null;
    state.selected = null;
    state.oppSig = null;
    state.specSig = null;
    state.opp = {};
    if (ui.opponents) ui.opponents.innerHTML = "";
  }

  // --------------------------------------------------------------- lobby view

  function optionNumber(el, lo, hi, fallback) {
    const value = Math.round(Number(el.value));
    if (!Number.isFinite(value)) return fallback;
    return Math.max(lo, Math.min(hi, value));
  }

  function optionsAreDefault(options) {
    return Object.keys(DEFAULT_OPTIONS).every(
      (key) => options[key] === DEFAULT_OPTIONS[key]
    );
  }

  function readOptions() {
    return {
      initialLevel: optionNumber(ui.optInitial, 1, 15, state.options.initialLevel),
      startingLines: optionNumber(ui.optGarbage, 0, 12, state.options.startingLines),
      linesPerLevel: optionNumber(ui.optPerLevel, 1, 50, state.options.linesPerLevel),
      levelIncrement: optionNumber(ui.optLevelStep, 1, 5, state.options.levelIncrement),
      specialEvery: optionNumber(ui.optSpecialEvery, 0, 50, state.options.specialEvery),
      specialCount: optionNumber(ui.optSpecialCount, 0, 5, state.options.specialCount),
      specialCapacity: optionNumber(ui.optCapacity, 1, 8, state.options.specialCapacity),
      lineAttacks: !!ui.optAttacks.checked,
    };
  }

  function sendOptions() {
    if (state.you !== state.host || state.phase !== "lobby") {
      syncOptions();
      return;
    }
    const o = readOptions();
    state.options = o;
    send(
      Uint8Array.of(
        8,
        o.initialLevel,
        o.startingLines,
        o.linesPerLevel,
        o.levelIncrement,
        o.specialEvery,
        o.specialCount,
        o.specialCapacity,
        o.lineAttacks ? 1 : 0
      )
    );
    syncOptions();
    syncRulesSummary();
    markDirty();
  }

  function syncOptions() {
    if (!ui.optInitial) return;
    const o = state.options;
    const lock = !(state.you === state.host && state.phase === "lobby");
    const bind = (el, value) => {
      if (document.activeElement !== el) el.value = value;
      el.disabled = lock;
    };
    bind(ui.optInitial, o.initialLevel);
    bind(ui.optGarbage, o.startingLines);
    bind(ui.optPerLevel, o.linesPerLevel);
    bind(ui.optLevelStep, o.levelIncrement);
    bind(ui.optSpecialEvery, o.specialEvery);
    bind(ui.optSpecialCount, o.specialCount);
    bind(ui.optCapacity, o.specialCapacity);
    if (document.activeElement !== ui.optAttacks) ui.optAttacks.checked = !!o.lineAttacks;
    ui.optAttacks.disabled = lock;
    const modified = !optionsAreDefault(o);
    const isHost = state.you === state.host;
    ui.optionsPanel.hidden = !isHost && !modified;
    ui.optionsPanel.classList.toggle("custom", modified);
    ui.optionsHint.textContent = lock
      ? isHost
        ? "locked during the round"
        : "modified by host"
      : "host settings · applied next round";
  }

  function toggleReady() {
    if (state.phase !== "lobby") return;
    const me = state.players.find((p) => p.id === state.you);
    if (!me) return;
    me.ready = !me.ready;
    send(Uint8Array.of(9, me.ready ? 1 : 0));
    renderLobby();
    markDirty();
  }

  function rulesSummaryText() {
    const o = state.options;
    const lines = (n) => `${n} line${n === 1 ? "" : "s"}`;
    const rows = `${o.startingLines} garbage row${o.startingLines === 1 ? "" : "s"}`;
    const specials =
      o.specialEvery > 0
        ? `special every ${lines(o.specialEvery)}${o.specialCount !== 1 ? ` ×${o.specialCount}` : ""}`
        : "no field specials";
    return `start L${o.initialLevel} · ${rows} · ${specials} · inventory ${o.specialCapacity} · attacks ${o.lineAttacks ? "on" : "off"}`;
  }

  function syncRulesSummary() {
    if (!ui.rulesSummary) return;
    const text = rulesSummaryText();
    if (ui.rulesSummary.textContent !== text) ui.rulesSummary.textContent = text;
  }

  function renderLobby() {
    if (!ui.lobbyTitle) return;
    ui.lobbyTitle.textContent =
      state.phase === "countdown" ? `Starting in ${Math.max(1, Math.ceil(state.cd / 1000))}` :
      state.phase === "over" ? "Round over" : "Lobby";

    const isHost = state.you === state.host;
    const connected = state.players.filter((p) => p.connected);
    const readyCount = connected.filter((p) => p.ready).length;
    const allReady = connected.length >= 2 && readyCount === connected.length;
    const me = state.players.find((p) => p.id === state.you);
    const iAmReady = !!(me && me.ready);

    ui.lobbySub.textContent =
      state.phase === "countdown" ? "get ready…" :
      `${connectedCount()}/6 connected · ${readyCount}/${connectedCount()} ready`;

    ui.readyBtn.disabled = state.phase !== "lobby" || state.maintenance;
    ui.readyBtn.textContent = iAmReady ? "Cancel ready" : "Ready";
    ui.readyBtn.classList.toggle("on", iAmReady);
    ui.startBtn.disabled = !(isHost && state.phase === "lobby" && allReady && !state.maintenance);
    ui.startBtn.textContent = state.phase === "countdown" ? "Starting…" : "Start game";

    let hint = "";
    if (state.maintenance) {
      hint = "maintenance: new games are disabled while running rounds finish";
    } else if (state.phase === "countdown") {
      hint = "";
    } else if (!isHost) {
      hint = allReady
        ? "all ready — waiting for the host"
        : `waiting for everyone to ready (${readyCount}/${connected.length})`;
    } else if (connected.length < 2) {
      hint = "need at least 2 players";
    } else if (!allReady) {
      hint = `everyone must be ready (${readyCount}/${connected.length})`;
    } else {
      hint = "all ready — start when you want";
    }
    ui.lobbyHint.textContent = hint;

    syncOptions();
    syncRulesSummary();
    renderPlayers();
  }

  function renderPlayers() {
    const sig = state.players
      .map((p) => `${p.id}:${p.nick}:${p.wins}:${p.connected ? 1 : 0}:${p.inGame ? 1 : 0}:${p.ready ? 1 : 0}`)
      .join("|") + "|host" + state.host;
    if (sig === state.playerSig) return;
    state.playerSig = sig;
    ui.playerList.innerHTML = "";
    for (const p of state.players) {
      const li = document.createElement("li");
      li.className = "player" + (p.connected ? "" : " disconnected");
      if (p.id === state.host) li.classList.add("host");
      const dot = document.createElement("span");
      dot.className = "dot";
      const name = document.createElement("span");
      name.className = "pname";
      name.textContent = p.nick + (p.id === state.you ? " (you)" : "");
      const ready = document.createElement("span");
      ready.className = "pready";
      ready.textContent = p.ready ? "READY" : "";
      const wins = document.createElement("span");
      wins.className = "pwins";
      wins.textContent = p.wins ? `${p.wins} W` : "";
      li.append(dot, name, ready, wins);
      ui.playerList.appendChild(li);
    }
  }

  function updateOverlay() {
    if (!ui.overlay) return;
    if (state.phase === "over") {
      ui.overlay.classList.remove("hidden");
      ui.overlay.innerHTML = `
        <div class="overlay-card">
          <div class="eyebrow">round over</div>
          <h2>${state.winner ? esc(nickOf(state.winner)) + " wins" : "Draw"}</h2>
          <p>Next round starts automatically…</p>
        </div>`;
    } else {
      ui.overlay.classList.add("hidden");
      ui.overlay.innerHTML = "";
    }
  }

  // ---------------------------------------------------------------- rendering

  function frameLoop(now) {
    requestAnimationFrame(frameLoop);
    tickKeys(now);
    if (!state.dirty) return;
    state.dirty = false;
    drawGame();
  }

  function drawGame() {
    if (!ui.gameView || ui.gameView.classList.contains("hidden")) return;
    syncRulesSummary();
    const me = mySlot();
    const myIdx = state.you - 1;
    const board = (myIdx >= 0 && state.game.boards[myIdx]) || ZERO_BOARD;
    const playing = state.phase === "playing" || state.phase === "over";
    const piece = me && me.present && me.alive && playing ? state.pred : null;

    drawBoard(ui.ownBoard, board, piece, {
      ghost: true,
      gray: me && me.present && !me.alive,
      alpha: me && me.present && !me.alive ? 0.45 : 1,
    });
    drawNext(ui.nextPiece, me && me.present ? me.next : null);

    ui.stats.innerHTML = me && me.present
      ? `level <b>${me.level}</b><br>lines <b>${me.lines}</b><br>alive <b>${aliveCount()}</b>`
      : "waiting";

    syncSpecials(me);
    syncOpponents();

    const waiting = state.phase === "playing" && (!me || !me.present);
    if (waiting) {
      ui.banner.classList.remove("hidden");
      ui.banner.textContent = "Round in progress — you join the next round.";
    } else {
      ui.banner.classList.add("hidden");
    }

    const targetable = state.selected != null && me && me.present && me.alive && state.phase === "playing";
    ui.ownWrap.classList.toggle("targetable", targetable);
    for (const o of Object.values(state.opp)) o.root.classList.toggle("targetable", targetable);

    if (state.selected != null && me && me.inv[state.selected] != null) {
      const sp = SPECIALS[me.inv[state.selected]];
      ui.hint.textContent = `Using ${sp.name} — click a field (esc to cancel)`;
      ui.hint.classList.add("active");
    } else {
      ui.hint.textContent = "←/→ move · ↑/X rotate · Z ccw · ↓ soft drop · space hard drop · 1–5 pick special";
      ui.hint.classList.remove("active");
    }
  }

  function syncSpecials(me) {
    const inv = me && me.present ? me.inv : [];
    const sig = inv.join(",") + "|" + (me && me.alive ? 1 : 0) + "|" + state.phase;
    if (sig === state.specSig) return;
    state.specSig = sig;
    ui.specials.innerHTML = "";
    if (!inv.length) {
      const empty = document.createElement("div");
      empty.className = "spec-empty";
      empty.textContent = "clear lines with special blocks to collect them";
      ui.specials.appendChild(empty);
      state.selected = null;
      return;
    }
    inv.forEach((sp, i) => {
      const def = SPECIALS[sp] || SPECIALS[0];
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "sp" + (state.selected === i ? " selected" : "");
      btn.dataset.i = String(i);
      btn.style.setProperty("--c", def.color);
      btn.title = def.name;
      btn.innerHTML = `<b>${def.letter}</b><span>${def.name}</span><em>${i + 1}</em>`;
      ui.specials.appendChild(btn);
    });
    if (state.selected != null && state.selected >= inv.length) state.selected = null;
  }

  function syncOpponents() {
    const wanted = [];
    for (let i = 0; i < 6; i++) {
      const slot = state.game.slots[i];
      if (slot && slot.present && i !== state.you - 1) wanted.push(i + 1);
    }
    const sig = wanted.join(",");
    if (sig !== state.oppSig) {
      state.oppSig = sig;
      state.opp = {};
      ui.opponents.innerHTML = "";
      if (!wanted.length) {
        const empty = document.createElement("div");
        empty.className = "empty";
        empty.textContent = "waiting for opponents…";
        ui.opponents.appendChild(empty);
      }
      for (const id of wanted) {
        const root = document.createElement("div");
        root.className = "panel opp";
        root.dataset.id = String(id);
        root.innerHTML = `
          <div class="opp-head"><span class="opp-name"></span><span class="opp-stats"></span></div>
          <canvas width="120" height="220"></canvas>
          <div class="opp-out hidden">OUT</div>`;
        root.addEventListener("click", () => useSpecialOn(id));
        ui.opponents.appendChild(root);
        state.opp[id] = {
          root,
          canvas: root.querySelector("canvas"),
          name: root.querySelector(".opp-name"),
          stats: root.querySelector(".opp-stats"),
          out: root.querySelector(".opp-out"),
        };
      }
    }

    for (const [idText, view] of Object.entries(state.opp)) {
      const id = Number(idText);
      const slot = state.game.slots[id - 1];
      const board = state.game.boards[id - 1] || ZERO_BOARD;
      const piece = slot && slot.present && slot.alive ? { kind: slot.kind, x: slot.x, y: slot.y, rot: slot.rot } : null;
      drawBoard(view.canvas, board, piece, { ghost: false, gray: !slot || !slot.alive, alpha: slot && slot.alive ? 1 : 0.5 });
      view.name.textContent = nickOf(id);
      view.stats.textContent = slot && slot.present ? `L${slot.level} · ${slot.lines}` : "";
      if (slot && !slot.alive) view.out.classList.remove("hidden");
      else view.out.classList.add("hidden");
    }
  }

  function drawBoard(canvas, cells, piece, opts) {
    const ctx = canvas.getContext("2d");
    const cw = canvas.width / W;
    const ch = canvas.height / H;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = "#070b16";
    ctx.fillRect(0, 0, canvas.width, canvas.height);

    ctx.strokeStyle = "rgba(148,163,184,0.07)";
    ctx.lineWidth = 1;
    for (let x = 1; x < W; x++) {
      ctx.beginPath();
      ctx.moveTo(Math.round(x * cw) + 0.5, 0);
      ctx.lineTo(Math.round(x * cw) + 0.5, canvas.height);
      ctx.stroke();
    }
    for (let y = 1; y < H; y++) {
      ctx.beginPath();
      ctx.moveTo(0, Math.round(y * ch) + 0.5);
      ctx.lineTo(canvas.width, Math.round(y * ch) + 0.5);
      ctx.stroke();
    }

    const alpha = opts.alpha == null ? 1 : opts.alpha;
    if (cells) {
      for (let y = 0; y < H; y++) {
        for (let x = 0; x < W; x++) {
          const v = cells[y * W + x];
          if (v) drawCell(ctx, x, y, cw, ch, v, alpha);
        }
      }
    }
    if (piece && piece.kind != null && piece.kind >= 0 && piece.kind <= 6) {
      if (opts.ghost) {
        let gy = piece.y;
        while (gy < H && !collidesFor(cells, piece.kind, piece.rot, piece.x, gy + 1)) gy++;
        if (gy !== piece.y) drawPiece(ctx, piece.kind, piece.rot, piece.x, gy, cw, ch, true, alpha);
      }
      drawPiece(ctx, piece.kind, piece.rot, piece.x, piece.y, cw, ch, false, alpha);
    }
    if (opts.gray) {
      ctx.fillStyle = "rgba(2,6,23,0.55)";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
    }
  }

  function roundRect(ctx, x, y, w, h, r) {
    if (ctx.roundRect) {
      ctx.beginPath();
      ctx.roundRect(x, y, w, h, r);
      return;
    }
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  function drawCell(ctx, x, y, cw, ch, value, alpha) {
    ctx.globalAlpha = alpha;
    if (value < 7) {
      ctx.fillStyle = BLOCK_COLORS[value] || "#94a3b8";
      roundRect(ctx, x * cw + 1.5, y * ch + 1.5, cw - 3, ch - 3, Math.max(2, cw * 0.18));
      ctx.fill();
      ctx.fillStyle = "rgba(255,255,255,0.22)";
      roundRect(ctx, x * cw + 3.5, y * ch + 3.5, cw - 7, Math.max(2, (ch - 7) * 0.28), 2);
      ctx.fill();
    } else {
      const def = SPECIALS[value - 7] || SPECIALS[0];
      ctx.fillStyle = "rgba(15,23,42,0.92)";
      roundRect(ctx, x * cw + 1.5, y * ch + 1.5, cw - 3, ch - 3, Math.max(2, cw * 0.18));
      ctx.fill();
      ctx.strokeStyle = def.color;
      ctx.lineWidth = Math.max(1, cw * 0.07);
      roundRect(ctx, x * cw + 1.5, y * ch + 1.5, cw - 3, ch - 3, Math.max(2, cw * 0.18));
      ctx.stroke();
      ctx.fillStyle = def.color;
      ctx.font = `bold ${Math.floor(cw * 0.6)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(def.letter, x * cw + cw / 2, y * ch + ch / 2 + 1);
    }
    ctx.globalAlpha = 1;
  }

  function drawPiece(ctx, kind, rot, px, py, cw, ch, ghost, alpha) {
    const cells = PIECE_DEFS[kind][rot % PIECE_DEFS[kind].length];
    const color = BLOCK_COLORS[[1, 2, 3, 4, 5, 6, 3][kind] || 1];
    ctx.globalAlpha = ghost ? 0.28 : alpha;
    for (const [dx, dy] of cells) {
      const x = (px + dx) * cw;
      const y = (py + dy) * ch;
      if (ghost) {
        ctx.strokeStyle = color;
        ctx.lineWidth = Math.max(1, cw * 0.08);
        roundRect(ctx, x + 2, y + 2, cw - 4, ch - 4, Math.max(2, cw * 0.16));
        ctx.stroke();
      } else {
        ctx.fillStyle = color;
        roundRect(ctx, x + 1.5, y + 1.5, cw - 3, ch - 3, Math.max(2, cw * 0.18));
        ctx.fill();
        ctx.fillStyle = "rgba(255,255,255,0.22)";
        roundRect(ctx, x + 3.5, y + 3.5, cw - 7, Math.max(2, (ch - 7) * 0.28), 2);
        ctx.fill();
      }
    }
    ctx.globalAlpha = 1;
  }

  function drawNext(canvas, kind) {
    const ctx = canvas.getContext("2d");
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (kind == null || kind < 0 || kind > 6) return;
    const cells = PIECE_DEFS[kind][0];
    const xs = cells.map((c) => c[0]);
    const ys = cells.map((c) => c[1]);
    const minx = Math.min(...xs);
    const maxx = Math.max(...xs);
    const miny = Math.min(...ys);
    const maxy = Math.max(...ys);
    const cs = 16;
    const ox = (canvas.width - (maxx - minx + 1) * cs) / 2 - minx * cs;
    const oy = (canvas.height - (maxy - miny + 1) * cs) / 2 - miny * cs;
    const color = BLOCK_COLORS[[1, 2, 3, 4, 5, 6, 3][kind] || 1];
    for (const [dx, dy] of cells) {
      const x = ox + dx * cs;
      const y = oy + dy * cs;
      ctx.fillStyle = color;
      roundRect(ctx, x + 1.5, y + 1.5, cs - 3, cs - 3, 3);
      ctx.fill();
      ctx.fillStyle = "rgba(255,255,255,0.22)";
      roundRect(ctx, x + 3.5, y + 3.5, cs - 7, 4, 1.5);
      ctx.fill();
    }
  }

  // ----------------------------------------------------------- client prediction

  function collidesFor(cells, kind, rot, x, y) {
    for (const [dx, dy] of PIECE_DEFS[kind][rot % PIECE_DEFS[kind].length]) {
      const px = x + dx;
      const py = y + dy;
      if (px < 0 || px >= W || py < 0 || py >= H) return true;
      if (cells && cells[py * W + px]) return true;
    }
    return false;
  }

  function applyLocal(action) {
    const p = state.pred;
    if (!p || p.kind == null) return;
    const idx = state.you - 1;
    const board = state.game.boards[idx] || ZERO_BOARD;
    if (action === 0) {
      if (!collidesFor(board, p.kind, p.rot, p.x - 1, p.y)) p.x--;
    } else if (action === 1) {
      if (!collidesFor(board, p.kind, p.rot, p.x + 1, p.y)) p.x++;
    } else if (action === 2 || action === 3) {
      const count = PIECE_DEFS[p.kind].length;
      const dir = action === 2 ? 1 : -1;
      const rot = ((p.rot + dir) % count + count) % count;
      for (const dx of [0, -1, 1, -2, 2]) {
        if (!collidesFor(board, p.kind, rot, p.x + dx, p.y)) {
          p.x += dx;
          p.rot = rot;
          break;
        }
      }
    } else if (action === 4) {
      if (!collidesFor(board, p.kind, p.rot, p.x, p.y + 1)) p.y++;
    } else if (action === 5) {
      while (!collidesFor(board, p.kind, p.rot, p.x, p.y + 1)) p.y++;
    }
    markDirty();
  }

  function doMove(action) {
    const me = mySlot();
    if (state.phase !== "playing" || !me || !me.present || !me.alive) return;
    send(Uint8Array.of(1, action));
    state.lastInputAt = performance.now();
    applyLocal(action);
  }

  function useSpecialOn(targetId) {
    const me = mySlot();
    if (state.phase !== "playing" || !me || !me.present || !me.alive) return;
    if (state.selected == null) return;
    const sp = me.inv[state.selected];
    if (sp == null) return;
    if (!targetId) return;
    send(Uint8Array.of(2, sp, targetId));
    state.selected = null;
    markDirty();
  }

  // -------------------------------------------------------------------- input

  function isTyping() {
    const el = document.activeElement;
    return el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
  }

  function onKeyDown(e) {
    if (isTyping()) return;
    if (state.phase !== "playing") return;
    const k = e.key;
    if (k === "ArrowLeft" || k === "a") {
      e.preventDefault();
      if (!state.keys.left) {
        state.keys.left = true;
        state.keys.nextLeft = performance.now() + 140;
        doMove(0);
      }
    } else if (k === "ArrowRight" || k === "d") {
      e.preventDefault();
      if (!state.keys.right) {
        state.keys.right = true;
        state.keys.nextRight = performance.now() + 140;
        doMove(1);
      }
    } else if (k === "ArrowDown" || k === "s") {
      e.preventDefault();
      if (!state.keys.down) {
        state.keys.down = true;
        state.keys.nextDown = performance.now() + 45;
        doMove(4);
      }
    } else if (k === "ArrowUp" || k === "x" || k === "X") {
      e.preventDefault();
      if (!e.repeat) doMove(2);
    } else if (k === "z" || k === "Z") {
      e.preventDefault();
      if (!e.repeat) doMove(3);
    } else if (k === " ") {
      e.preventDefault();
      if (!e.repeat) doMove(5);
    } else if (k === "Escape") {
      state.selected = null;
      markDirty();
    } else if (k >= "1" && k <= "5") {
      const me = mySlot();
      if (!me || !me.present) return;
      const idx = Number(k) - 1;
      if (idx < me.inv.length) {
        state.selected = state.selected === idx ? null : idx;
        markDirty();
      }
    }
  }

  function onKeyUp(e) {
    if (e.key === "ArrowLeft" || e.key === "a") state.keys.left = false;
    else if (e.key === "ArrowRight" || e.key === "d") state.keys.right = false;
    else if (e.key === "ArrowDown" || e.key === "s") state.keys.down = false;
  }

  function tickKeys(now) {
    if (state.phase !== "playing") {
      state.keys.left = state.keys.right = state.keys.down = false;
      return;
    }
    if (state.keys.left && now >= state.keys.nextLeft) {
      doMove(0);
      state.keys.nextLeft = now + 45;
    }
    if (state.keys.right && now >= state.keys.nextRight) {
      doMove(1);
      state.keys.nextRight = now + 45;
    }
    if (state.keys.down && now >= state.keys.nextDown) {
      doMove(4);
      state.keys.nextDown = now + 45;
    }
  }

  // --------------------------------------------------------------------- logs

  function pushLog(text, cls) {
    state.log.push({ text, cls: cls || "" });
    if (state.log.length > 120) state.log.shift();
    renderLog();
  }

  function renderLog() {
    if (!ui.log) return;
    ui.log.innerHTML = "";
    for (const entry of state.log) {
      const div = document.createElement("div");
      div.className = "log-line " + entry.cls;
      div.textContent = entry.text;
      ui.log.appendChild(div);
    }
    ui.log.scrollTop = ui.log.scrollHeight;
  }

  boot();
})();
