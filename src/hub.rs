use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::game::{Game, GameOptions, MAX_SLOTS};
use crate::proto::{self, ClientMsg};
use crate::util;

pub enum Out {
    App(Vec<u8>),
    Raw(Vec<u8>),
}

pub struct Conn {
    pub id: u64,
    pub tx: SyncSender<Out>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Lobby,
    Countdown,
    Playing,
    Over,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Lobby => "lobby",
            Phase::Countdown => "countdown",
            Phase::Playing => "playing",
            Phase::Over => "over",
        }
    }
}

pub struct Slot {
    pub id: u8,
    pub nick: String,
    pub token: String,
    pub conn: Option<Conn>,
    pub wins: u32,
    pub in_game: bool,
    pub ready: bool,
    pub last_seen: Instant,
}

pub struct Room {
    pub id: String,
    pub slots: [Option<Slot>; MAX_SLOTS],
    pub phase: Phase,
    pub game: Option<Game>,
    pub options: GameOptions,
    pub host: u8,
    maintenance_notified: bool,
    pub start_at: Option<Instant>,
    pub over_deadline: Option<Instant>,
    pub empty_since: Option<Instant>,
    pub lobby_dirty: bool,
    pub last_lobby_at: Instant,
    pub last_state_at: Instant,
    last_versions: [u32; MAX_SLOTS],
    pub doomed: bool,
}

impl Room {
    pub fn new(id: String) -> Room {
        Room {
            id,
            slots: Default::default(),
            phase: Phase::Lobby,
            game: None,
            options: GameOptions::default(),
            host: 0,
            maintenance_notified: false,
            start_at: None,
            over_deadline: None,
            empty_since: None,
            lobby_dirty: true,
            last_lobby_at: Instant::now(),
            last_state_at: Instant::now(),
            last_versions: [0; MAX_SLOTS],
            doomed: false,
        }
    }

    fn unique_nick(&self, base: &str, except: usize) -> String {
        let base = util::sanitize_nick(base);
        let taken = |candidate: &str| {
            self.slots.iter().enumerate().any(|(i, s)| {
                i != except
                    && s.as_ref()
                        .map(|s| s.nick.eq_ignore_ascii_case(candidate))
                        .unwrap_or(false)
            })
        };
        if !taken(&base) {
            return base;
        }
        for n in 2..100 {
            let candidate = format!("{}{}", base, n);
            if !taken(&candidate) {
                return candidate;
            }
        }
        format!("{}{}", base, util::random_token())
    }

    pub fn join(
        &mut self,
        nick: &str,
        token: Option<String>,
        conn: Conn,
    ) -> Result<(u8, String), String> {
        let now = Instant::now();

        if crate::shutdown::maintenance() {
            let reconnect = token
                .as_deref()
                .map(|tok| {
                    self.slots
                        .iter()
                        .any(|s| s.as_ref().map(|s| s.token == tok).unwrap_or(false))
                })
                .unwrap_or(false);
            if !reconnect {
                return Err(
                    "maintenance: new games are disabled while the server waits for running games"
                        .to_string(),
                );
            }
        }

        if let Some(tok) = token.as_deref() {
            if let Some(idx) = self
                .slots
                .iter()
                .position(|s| s.as_ref().map(|s| s.token == tok).unwrap_or(false))
            {
                let slot = self.slots[idx].as_mut().unwrap();
                let hello = proto::hello_frame(&self.id, slot.id, &slot.token);
                let _ = conn.tx.try_send(Out::App(hello));
                if let Some(game) = &self.game {
                    let _ = conn.tx.try_send(Out::App(proto::snapshot_frame(game)));
                }
                slot.conn = Some(conn);
                slot.last_seen = now;
                if let Some(game) = self.game.as_mut() {
                    if let Some(p) = game.slots[idx].as_mut() {
                        p.frozen = false;
                    }
                }
                if self.host == 0 {
                    self.host = slot.id;
                }
                self.empty_since = None;
                self.lobby_dirty = true;
                return Ok((slot.id, slot.token.clone()));
            }
        }

        let free = self
            .slots
            .iter()
            .position(|s| s.is_none())
            .ok_or_else(|| "game is full (6 players max)".to_string())?;
        let id = free as u8 + 1;
        let token_out = token.unwrap_or_else(util::random_token);
        let nick = self.unique_nick(nick, free);
        let hello = proto::hello_frame(&self.id, id, &token_out);
        let _ = conn.tx.try_send(Out::App(hello));
        if let Some(game) = &self.game {
            let _ = conn.tx.try_send(Out::App(proto::snapshot_frame(game)));
        }
        let in_game = matches!(self.phase, Phase::Lobby | Phase::Countdown);
        self.slots[free] = Some(Slot {
            id,
            nick,
            token: token_out.clone(),
            conn: Some(conn),
            wins: 0,
            in_game,
            ready: false,
            last_seen: now,
        });
        if self.host == 0 {
            self.host = id;
        }
        if self.phase == Phase::Countdown {
            self.phase = Phase::Lobby;
            self.start_at = None;
        }
        self.empty_since = None;
        self.lobby_dirty = true;
        Ok((id, token_out))
    }

    fn send_to(&self, idx: usize, frame: Vec<u8>) {
        if let Some(c) = self.slots.get(idx).and_then(|s| s.as_ref()).and_then(|s| s.conn.as_ref()) {
            let _ = c.tx.try_send(Out::App(frame));
        }
    }

    fn send_error(&self, idx: usize, msg: &str) {
        let json = format!("{{\"t\":\"error\",\"msg\":\"{}\"}}", util::json_escape(msg));
        self.send_to(idx, proto::event_frame(&json));
    }

    fn broadcast(&mut self, frame: &[u8]) {
        let mut dead = Vec::new();
        for (i, slot) in self.slots.iter().enumerate() {
            if let Some(conn) = slot.as_ref().and_then(|s| s.conn.as_ref()) {
                match conn.tx.try_send(Out::App(frame.to_vec())) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => dead.push(i),
                }
            }
        }
        for idx in dead {
            self.note_conn_lost(idx);
        }
    }

    fn note_conn_lost(&mut self, idx: usize) {
        let now = Instant::now();
        let Some(slot) = self.slots.get_mut(idx).and_then(|s| s.as_mut()) else {
            return;
        };
        if slot.conn.take().is_none() {
            return;
        }
        slot.last_seen = now;
        if matches!(self.phase, Phase::Lobby | Phase::Countdown) {
            self.slots[idx] = None;
            self.cancel_countdown_if_unready();
        } else if let Some(game) = self.game.as_mut() {
            if let Some(p) = game.slots[idx].as_mut() {
                p.frozen = true;
            }
        }
        self.reassign_host();
        self.empty_since = None;
        self.check_empty(now);
        self.lobby_dirty = true;
    }

    fn connected_count(&self) -> usize {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.conn.is_some())
            .count()
    }

    pub fn is_active_game(&self) -> bool {
        self.phase == Phase::Playing && self.connected_count() > 0
    }

    pub fn enter_maintenance(&mut self) {
        if self.phase == Phase::Countdown {
            self.phase = Phase::Lobby;
            self.start_at = None;
            self.lobby_dirty = true;
        }
        if !self.maintenance_notified {
            self.maintenance_notified = true;
            self.lobby_dirty = true;
        }
    }

    fn all_ready(&self) -> bool {
        let mut connected = 0;
        for slot in self.slots.iter().flatten() {
            if slot.conn.is_some() {
                connected += 1;
                if !slot.ready {
                    return false;
                }
            }
        }
        connected > 0
    }

    fn cancel_countdown_if_unready(&mut self) {
        if self.phase == Phase::Countdown
            && (self.connected_count() < 2 || !self.all_ready())
        {
            self.phase = Phase::Lobby;
            self.start_at = None;
            self.lobby_dirty = true;
        }
    }

    fn reassign_host(&mut self) {
        let host_ok = self
            .slots
            .iter()
            .flatten()
            .any(|s| s.id == self.host && s.conn.is_some());
        if host_ok {
            return;
        }
        self.host = self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.conn.is_some())
            .map(|s| s.id)
            .min()
            .unwrap_or(0);
    }

    fn check_empty(&mut self, now: Instant) {
        let has_conn = self.slots.iter().flatten().any(|s| s.conn.is_some());
        if has_conn {
            self.empty_since = None;
            return;
        }
        let since = *self.empty_since.get_or_insert(now);
        let grace = match self.phase {
            Phase::Playing | Phase::Over => 180,
            _ => 30,
        };
        if now.duration_since(since) >= Duration::from_secs(grace) {
            self.doomed = true;
        }
    }

    fn lobby_json(&self) -> String {
        let winner = self.game.as_ref().map(|g| g.winner).unwrap_or(0);
        let cd = match (self.phase, self.start_at) {
            (Phase::Countdown, Some(at)) => at.saturating_duration_since(Instant::now()).as_millis() as u64,
            _ => 0,
        };
        let mut out = String::with_capacity(256);
        out.push_str(&format!(
            "{{\"game\":\"{}\",\"phase\":\"{}\",\"host\":{},\"winner\":{},\"cd\":{}",
            util::json_escape(&self.id),
            self.phase.as_str(),
            self.host,
            winner,
            cd
        ));
        let o = &self.options;
        out.push_str(&format!(
            ",\"options\":{{\"initialLevel\":{},\"startingLines\":{},\"linesPerLevel\":{},\"levelIncrement\":{},\"specialEvery\":{},\"specialCount\":{},\"specialCapacity\":{},\"lineAttacks\":{}}}",
            o.initial_level,
            o.starting_lines,
            o.lines_per_level,
            o.level_increment,
            o.special_every,
            o.special_count,
            o.special_capacity,
            if o.line_attacks { "true" } else { "false" }
        ));
        out.push_str(&format!(
            ",\"maintenance\":{}",
            if crate::shutdown::maintenance() {
                "true"
            } else {
                "false"
            }
        ));
        out.push_str(",\"players\":[");
        let mut first = true;
        for slot in self.slots.iter().flatten() {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"id\":{},\"nick\":\"{}\",\"wins\":{},\"connected\":{},\"inGame\":{},\"ready\":{}}}",
                slot.id,
                util::json_escape(&slot.nick),
                slot.wins,
                slot.conn.is_some(),
                slot.in_game,
                if slot.ready { "true" } else { "false" }
            ));
        }
        out.push_str("]}");
        out
    }

    fn broadcast_lobby(&mut self) {
        let frame = proto::lobby_frame(&self.lobby_json());
        self.broadcast(&frame);
    }

    pub fn on_input(&mut self, player: u8, conn_id: u64, msg: ClientMsg) {
        let idx = player as usize - 1;
        let matching = self
            .slots
            .get(idx)
            .and_then(|s| s.as_ref())
            .and_then(|s| s.conn.as_ref())
            .map(|c| c.id == conn_id)
            .unwrap_or(false);
        if !matching {
            return;
        }

        match msg {
            ClientMsg::Move(action) => {
                if self.phase == Phase::Playing {
                    if let Some(game) = self.game.as_mut() {
                        game.input(idx, action);
                    }
                }
            }
            ClientMsg::Special { sp, target } => {
                if self.phase != Phase::Playing {
                    self.send_error(idx, "game is not running");
                    return;
                }
                if target == 0 || target as usize > MAX_SLOTS {
                    self.send_error(idx, "pick a field first");
                    return;
                }
                if let Some(game) = self.game.as_mut() {
                    if let Err(err) = game.use_special(idx, sp, target as usize - 1) {
                        self.send_error(idx, &err);
                    }
                }
            }
            ClientMsg::Start => {
                if crate::shutdown::maintenance() {
                    self.send_error(
                        idx,
                        "maintenance: new games are disabled while the server waits for running games",
                    );
                    return;
                }
                if self.phase != Phase::Lobby {
                    self.send_error(idx, "game already starting");
                    return;
                }
                if player != self.host {
                    self.send_error(idx, "only the host can start");
                    return;
                }
                if self.connected_count() < 2 {
                    self.send_error(idx, "need at least 2 players");
                    return;
                }
                if !self.all_ready() {
                    self.send_error(
                        idx,
                        "everyone must be ready (reload the page if there is no Ready button)",
                    );
                    return;
                }
                self.phase = Phase::Countdown;
                self.start_at = Some(Instant::now() + Duration::from_millis(3000));
                self.lobby_dirty = true;
            }
            ClientMsg::Chat(text) => {
                let nick = self
                    .slots
                    .get(idx)
                    .and_then(|s| s.as_ref())
                    .map(|s| s.nick.clone())
                    .unwrap_or_default();
                let json = format!(
                    "{{\"t\":\"chat\",\"from\":{},\"nick\":\"{}\",\"text\":\"{}\"}}",
                    player,
                    util::json_escape(&nick),
                    util::json_escape(&text)
                );
                self.broadcast(&proto::event_frame(&json));
            }
            ClientMsg::Ping(nonce) => {
                self.send_to(idx, proto::pong_frame(&nonce));
            }
            ClientMsg::Resync => {
                if let Some(game) = &self.game {
                    self.send_to(idx, proto::snapshot_frame(game));
                }
            }
            ClientMsg::Leave => self.leave(idx),
            ClientMsg::Options(opts) => {
                if player != self.host {
                    self.send_error(idx, "only the host can change options");
                    return;
                }
                if self.phase != Phase::Lobby {
                    self.send_error(idx, "options lock when the round starts");
                    return;
                }
                self.options = opts.clamped();
                self.lobby_dirty = true;
            }
            ClientMsg::Ready(ready) => {
                if self.phase != Phase::Lobby {
                    self.send_error(idx, "readiness locks when the countdown starts");
                    return;
                }
                if let Some(slot) = self.slots.get_mut(idx).and_then(|s| s.as_mut()) {
                    slot.ready = ready;
                }
                self.lobby_dirty = true;
            }
        }
    }

    fn leave(&mut self, idx: usize) {
        if self.slots.get(idx).and_then(|s| s.as_ref()).is_none() {
            return;
        }
        if let Some(game) = self.game.as_mut() {
            if game.slots[idx].is_some() {
                game.force_lost(idx);
            }
            let events = std::mem::take(&mut game.events);
            for ev in events {
                self.broadcast(&proto::event_frame(&ev));
            }
        }
        self.slots[idx] = None;
        self.reassign_host();
        self.cancel_countdown_if_unready();
        let now = Instant::now();
        self.empty_since = None;
        self.check_empty(now);
        self.lobby_dirty = true;
    }

    pub fn detach(&mut self, player: u8, conn_id: u64) {
        let idx = player as usize - 1;
        let matches = self
            .slots
            .get(idx)
            .and_then(|s| s.as_ref())
            .and_then(|s| s.conn.as_ref())
            .map(|c| c.id == conn_id)
            .unwrap_or(false);
        if matches {
            self.note_conn_lost(idx);
        }
    }

    fn begin_game(&mut self, now: Instant) {
        if crate::shutdown::maintenance() {
            self.phase = Phase::Lobby;
            self.start_at = None;
            self.lobby_dirty = true;
            return;
        }
        let ids: Vec<u8> = self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.conn.is_some())
            .map(|s| s.id)
            .collect();
        if ids.len() < 2 || !self.all_ready() {
            self.phase = Phase::Lobby;
            self.start_at = None;
            self.lobby_dirty = true;
            return;
        }
        for slot in self.slots.iter_mut().flatten() {
            slot.in_game = slot.conn.is_some();
            slot.ready = false;
        }
        let seed = util::now_nanos()
            ^ self.id.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        let game = Game::new(&ids, seed, self.options.clone());
        let snapshot = proto::snapshot_frame(&game);
        let state = proto::state_frame(&game);
        self.game = Some(game);
        self.last_versions = [0; MAX_SLOTS];
        self.phase = Phase::Playing;
        self.start_at = None;
        self.over_deadline = None;
        self.last_state_at = now;
        self.lobby_dirty = true;
        self.broadcast(&snapshot);
        self.broadcast(&state);
    }

    fn reset_to_lobby(&mut self) {
        self.game = None;
        self.phase = Phase::Lobby;
        self.start_at = None;
        self.over_deadline = None;
        self.last_versions = [0; MAX_SLOTS];
        for slot in self.slots.iter_mut().flatten() {
            slot.in_game = false;
            slot.ready = false;
        }
        self.lobby_dirty = true;
    }

    pub fn tick(&mut self, now: Instant) {
        match self.phase {
            Phase::Countdown => {
                if crate::shutdown::maintenance() {
                    self.phase = Phase::Lobby;
                    self.start_at = None;
                    self.lobby_dirty = true;
                } else if let Some(at) = self.start_at {
                    if now >= at {
                        self.begin_game(now);
                    } else {
                        self.lobby_dirty = true;
                    }
                } else {
                    self.phase = Phase::Lobby;
                }
            }
            Phase::Playing => {
                let (events, boards, state, over, winner) = {
                    let Some(game) = self.game.as_mut() else {
                        self.phase = Phase::Lobby;
                        self.lobby_dirty = true;
                        return;
                    };
                    game.tick(now);
                    let events = std::mem::take(&mut game.events);
                    let mut boards = Vec::new();
                    for idx in 0..MAX_SLOTS {
                        if let Some(p) = &game.slots[idx] {
                            if p.board.version != self.last_versions[idx] {
                                self.last_versions[idx] = p.board.version;
                                boards.push(proto::board_frame(idx as u8 + 1, &p.board));
                            }
                        }
                    }
                    let state = if now.duration_since(self.last_state_at) >= Duration::from_millis(45) {
                        Some(proto::state_frame(game))
                    } else {
                        None
                    };
                    (events, boards, state, game.over, game.winner)
                };
                for ev in events {
                    self.broadcast(&proto::event_frame(&ev));
                }
                for b in boards {
                    self.broadcast(&b);
                }
                if let Some(s) = state {
                    self.broadcast(&s);
                    self.last_state_at = now;
                }
                if over {
                    self.phase = Phase::Over;
                    self.over_deadline = Some(now + Duration::from_millis(7000));
                    if winner != 0 {
                        if let Some(slot) = self
                            .slots
                            .get_mut(winner as usize - 1)
                            .and_then(|s| s.as_mut())
                        {
                            slot.wins += 1;
                        }
                    }
                    self.lobby_dirty = true;
                }
            }
            Phase::Over => {
                if let Some(deadline) = self.over_deadline {
                    if now >= deadline {
                        self.reset_to_lobby();
                    }
                }
            }
            Phase::Lobby => {}
        }

        if self.lobby_dirty {
            let interval = if self.phase == Phase::Countdown { 80 } else { 30 };
            if now.duration_since(self.last_lobby_at) >= Duration::from_millis(interval) {
                self.broadcast_lobby();
                self.last_lobby_at = now;
                self.lobby_dirty = false;
            }
        }
        self.check_empty(now);
    }

    pub fn summary_json(&self) -> String {
        let players = self.connected_count();
        format!(
            "{{\"id\":\"{}\",\"phase\":\"{}\",\"players\":{},\"slots\":{}}}",
            util::json_escape(&self.id),
            self.phase.as_str(),
            players,
            self.slots.iter().filter(|s| s.is_some()).count()
        )
    }
}

pub struct SseLogin {
    pub room: Arc<Mutex<Room>>,
    pub player: u8,
    pub conn: u64,
}

pub struct Hub {
    pub rooms: Mutex<HashMap<String, Arc<Mutex<Room>>>>,
    pub sse: Mutex<HashMap<String, SseLogin>>,
    seq: AtomicU64,
}

impl Hub {
    pub fn new() -> Hub {
        Hub {
            rooms: Mutex::new(HashMap::new()),
            sse: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(1),
        }
    }

    pub fn next_conn_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    pub fn channel(&self) -> (SyncSender<Out>, std::sync::mpsc::Receiver<Out>) {
        sync_channel(1024)
    }

    pub fn join(
        &self,
        game_id: &str,
        nick: &str,
        token: Option<String>,
        conn: Conn,
    ) -> Result<(Arc<Mutex<Room>>, u8, String), String> {
        let maintenance = crate::shutdown::maintenance();
        let room = {
            let mut rooms = self.rooms.lock().unwrap();
            if maintenance && !rooms.contains_key(game_id) {
                return Err(
                    "maintenance: new games are disabled while the server waits for running games"
                        .to_string(),
                );
            }
            rooms
                .entry(game_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(Room::new(game_id.to_string()))))
                .clone()
        };
        let mut room_guard = room.lock().unwrap();
        let (player, token_out) = room_guard.join(nick, token, conn)?;
        drop(room_guard);
        Ok((room, player, token_out))
    }

    pub fn tick(&self) -> bool {
        let maintenance = crate::shutdown::maintenance();
        let rooms: Vec<Arc<Mutex<Room>>> = {
            let map = self.rooms.lock().unwrap();
            map.values().cloned().collect()
        };
        let now = Instant::now();
        let mut doomed = Vec::new();
        let mut active = 0;
        for arc in rooms {
            let mut room = arc.lock().unwrap();
            if maintenance {
                room.enter_maintenance();
            }
            room.tick(now);
            if room.is_active_game() {
                active += 1;
            }
            if room.doomed {
                doomed.push(room.id.clone());
            }
        }
        if !doomed.is_empty() {
            let mut map = self.rooms.lock().unwrap();
            for id in doomed {
                map.remove(&id);
            }
        }
        maintenance && active == 0
    }

    pub fn status_json(&self) -> String {
        let rooms: Vec<Arc<Mutex<Room>>> = {
            let map = self.rooms.lock().unwrap();
            map.values().cloned().collect()
        };
        let mut active = 0;
        for arc in rooms {
            let room = arc.lock().unwrap();
            if room.is_active_game() {
                active += 1;
            }
        }
        format!(
            "{{\"maintenance\":{},\"activeGames\":{}}}",
            if crate::shutdown::maintenance() {
                "true"
            } else {
                "false"
            },
            active
        )
    }

    pub fn rooms_json(&self) -> String {
        let rooms: Vec<Arc<Mutex<Room>>> = {
            let map = self.rooms.lock().unwrap();
            map.values().cloned().collect()
        };
        let mut out = String::from("{\"rooms\":[");
        let mut first = true;
        for arc in rooms {
            let room = arc.lock().unwrap();
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&room.summary_json());
        }
        out.push_str("]}");
        out
    }
}
