//! Server-authoritative cecris rules.
//!
//! Field: 12 columns x 22 rows. Cell values: 0 empty, 1..=6 normal blocks,
//! 7..=15 special blocks (SPECIAL_BASE + special index).

use std::time::Instant;

pub const W: usize = 12;
pub const H: usize = 22;
pub const CELLS: usize = W * H;
pub const MAX_SLOTS: usize = 6;
pub const MAX_INVENTORY: usize = 8;
pub const NONE: u8 = 0xFF;
pub const SPECIAL_BASE: u8 = 7;
pub const SPECIAL_KINDS: u8 = 9;

pub const SP_ADD_LINE: u8 = 0;
pub const SP_CLEAR_LINE: u8 = 1;
pub const SP_NUKE: u8 = 2;
pub const SP_CLEAR_RANDOM: u8 = 3;
pub const SP_SWITCH: u8 = 4;
pub const SP_CLEAR_SPECIALS: u8 = 5;
pub const SP_GRAVITY: u8 = 6;
pub const SP_BLOCKQUAKE: u8 = 7;
pub const SP_BLOCKBOMB: u8 = 8;

const SPECIAL_WEIGHTS: [u32; 9] = [30, 14, 3, 13, 5, 12, 9, 7, 5];

#[derive(Clone, Debug)]
pub struct GameOptions {
    pub initial_level: u32,
    pub starting_lines: u32,
    pub lines_per_level: u32,
    pub level_increment: u32,
    pub special_every: u32,
    pub special_count: u32,
    pub special_capacity: u32,
    pub line_attacks: bool,
}

impl Default for GameOptions {
    fn default() -> Self {
        GameOptions {
            initial_level: 1,
            starting_lines: 0,
            lines_per_level: 10,
            level_increment: 1,
            special_every: 5,
            special_count: 1,
            special_capacity: 5,
            line_attacks: true,
        }
    }
}

impl GameOptions {
    pub fn clamped(mut self) -> Self {
        self.initial_level = self.initial_level.clamp(1, 15);
        self.starting_lines = self.starting_lines.clamp(0, 12);
        self.lines_per_level = self.lines_per_level.clamp(1, 50);
        self.level_increment = self.level_increment.clamp(1, 5);
        self.special_every = self.special_every.clamp(0, 50);
        self.special_count = self.special_count.clamp(0, 5);
        self.special_capacity = self.special_capacity.clamp(1, MAX_INVENTORY as u32);
        self
    }
}

pub fn special_cell(sp: u8) -> u8 {
    SPECIAL_BASE + sp
}

pub fn is_special(cell: u8) -> bool {
    (SPECIAL_BASE..SPECIAL_BASE + SPECIAL_KINDS).contains(&cell)
}

pub fn is_block(cell: u8) -> bool {
    cell > 0 && cell < SPECIAL_BASE
}

#[rustfmt::skip]
static PIECES: [[[[u8; 4]; 4]; 4]; 7] = [
    [ // I
        [[1,1,1,1],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
        [[0,0,1,0],[0,0,1,0],[0,0,1,0],[0,0,1,0]],
        [[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
        [[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
    ],
    [ // O
        [[1,1,0,0],[1,1,0,0],[0,0,0,0],[0,0,0,0]],
        [[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
        [[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
        [[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
    ],
    [ // L
        [[0,0,1,0],[0,0,1,0],[0,1,1,0],[0,0,0,0]],
        [[0,1,0,0],[0,1,1,1],[0,0,0,0],[0,0,0,0]],
        [[0,1,1,0],[0,1,0,0],[0,1,0,0],[0,0,0,0]],
        [[0,1,1,1],[0,0,0,1],[0,0,0,0],[0,0,0,0]],
    ],
    [ // J
        [[0,1,0,0],[0,1,0,0],[0,1,1,0],[0,0,0,0]],
        [[0,1,1,1],[0,1,0,0],[0,0,0,0],[0,0,0,0]],
        [[0,1,1,0],[0,0,1,0],[0,0,1,0],[0,0,0,0]],
        [[0,0,0,1],[0,1,1,1],[0,0,0,0],[0,0,0,0]],
    ],
    [ // S
        [[0,0,1,0],[0,1,1,0],[0,1,0,0],[0,0,0,0]],
        [[0,1,1,0],[0,0,1,1],[0,0,0,0],[0,0,0,0]],
        [[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
        [[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
    ],
    [ // Z
        [[0,1,0,0],[0,1,1,0],[0,0,1,0],[0,0,0,0]],
        [[0,0,1,1],[0,1,1,0],[0,0,0,0],[0,0,0,0]],
        [[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
        [[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],
    ],
    [ // T
        [[0,0,1,0],[0,1,1,0],[0,0,1,0],[0,0,0,0]],
        [[0,0,1,0],[0,1,1,1],[0,0,0,0],[0,0,0,0]],
        [[0,1,0,0],[0,1,1,0],[0,1,0,0],[0,0,0,0]],
        [[0,1,1,1],[0,0,1,0],[0,0,0,0],[0,0,0,0]],
    ],
];

pub const ORIENT_COUNT: [u8; 7] = [2, 1, 4, 4, 2, 2, 4];
const PIECE_COLORS: [u8; 7] = [1, 2, 3, 4, 5, 6, 3];

pub fn piece_matrix(kind: u8, rot: u8) -> &'static [[u8; 4]; 4] {
    &PIECES[kind as usize][(rot % ORIENT_COUNT[kind as usize]) as usize]
}

pub fn piece_color(kind: u8) -> u8 {
    PIECE_COLORS[kind as usize]
}

pub fn gravity_ms(level: u32) -> u32 {
    let lvl = level.max(1).min(30);
    850u32.saturating_sub((lvl - 1) * 55).max(60)
}

#[derive(Clone)]
pub struct Board {
    pub cells: [u8; CELLS],
    pub version: u32,
}

impl Board {
    pub fn new() -> Self {
        Board {
            cells: [0; CELLS],
            version: 0,
        }
    }
}

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as u32
    }
}

#[derive(Clone)]
struct PieceBag {
    order: [u8; 7],
    pos: u8,
}

impl PieceBag {
    fn new(rng: &mut Rng) -> Self {
        let mut order = [0u8, 1, 2, 3, 4, 5, 6];
        for i in (1..order.len()).rev() {
            let j = rng.below((i + 1) as u32) as usize;
            order.swap(i, j);
        }
        PieceBag { order, pos: 0 }
    }

    fn next(&mut self, rng: &mut Rng) -> u8 {
        if self.pos as usize >= self.order.len() {
            *self = PieceBag::new(rng);
        }
        let piece = self.order[self.pos as usize];
        self.pos += 1;
        piece
    }
}

pub fn collides(board: &Board, kind: u8, rot: u8, x: i32, y: i32) -> bool {
    let matrix = piece_matrix(kind, rot);
    for (dy, row) in matrix.iter().enumerate() {
        for (dx, cell) in row.iter().enumerate() {
            if *cell == 0 {
                continue;
            }
            let px = x + dx as i32;
            let py = y + dy as i32;
            if px < 0 || px >= W as i32 || py < 0 || py >= H as i32 {
                return true;
            }
            if board.cells[py as usize * W + px as usize] != 0 {
                return true;
            }
        }
    }
    false
}

fn place(board: &mut Board, kind: u8, rot: u8, x: i32, y: i32) {
    let matrix = piece_matrix(kind, rot);
    let color = piece_color(kind);
    for (dy, row) in matrix.iter().enumerate() {
        for (dx, cell) in row.iter().enumerate() {
            if *cell == 0 {
                continue;
            }
            let px = x + dx as i32;
            let py = y + dy as i32;
            if px >= 0 && px < W as i32 && py >= 0 && py < H as i32 {
                board.cells[py as usize * W + px as usize] = color;
            }
        }
    }
}

/// Clears full rows. Returns (rows cleared, specials found in them).
pub fn clear_lines(board: &mut Board, collect_specials: bool) -> (usize, Vec<u8>) {
    let mut cleared = 0;
    let mut gained = Vec::new();
    let mut y = H;
    while y > 0 {
        y -= 1;
        if (0..W).all(|x| board.cells[y * W + x] != 0) {
            if collect_specials {
                for x in 0..W {
                    let cell = board.cells[y * W + x];
                    if is_special(cell) {
                        gained.push(cell - SPECIAL_BASE);
                    }
                }
            }
            for row in (1..=y).rev() {
                for x in 0..W {
                    board.cells[row * W + x] = board.cells[(row - 1) * W + x];
                }
            }
            for x in 0..W {
                board.cells[x] = 0;
            }
            cleared += 1;
            y += 1; // re-check the row that shifted down
        }
    }
    (cleared, gained)
}

fn weighted_special(rng: &mut Rng) -> u8 {
    let total: u32 = SPECIAL_WEIGHTS.iter().sum();
    let mut n = rng.below(total);
    for (sp, weight) in SPECIAL_WEIGHTS.iter().enumerate() {
        if n < *weight {
            return sp as u8;
        }
        n -= *weight;
    }
    SP_ADD_LINE
}

fn place_special(board: &mut Board, rng: &mut Rng) {
    let mut candidates = Vec::new();
    for (i, cell) in board.cells.iter().enumerate() {
        if is_block(*cell) {
            candidates.push(i);
        }
    }
    if candidates.is_empty() {
        return;
    }
    let idx = candidates[rng.below(candidates.len() as u32) as usize];
    board.cells[idx] = special_cell(weighted_special(rng));
    board.version = board.version.wrapping_add(1);
}

fn compact_gravity(board: &mut Board) {
    for x in 0..W {
        let mut write = H;
        for y in (0..H).rev() {
            let cell = board.cells[y * W + x];
            if cell != 0 {
                write -= 1;
                board.cells[write * W + x] = cell;
                if write != y {
                    board.cells[y * W + x] = 0;
                }
            }
        }
        for y in (0..write).rev() {
            board.cells[y * W + x] = 0;
        }
    }
    board.version = board.version.wrapping_add(1);
}

#[derive(Clone)]
pub struct PlayerState {
    pub id: u8,
    pub board: Board,
    pub kind: u8,
    pub x: i32,
    pub y: i32,
    pub rot: u8,
    pub next: u8,
    bag: PieceBag,
    pub level: u32,
    pub lines: u32,
    pub alive: bool,
    pub frozen: bool,
    pub inventory: Vec<u8>,
    pub spawn_wait: u32,
    pub drop_acc: u32,
    pub special_acc: u32,
}

impl PlayerState {
    fn new(id: u8, next: u8, level: u32, bag: PieceBag) -> Self {
        PlayerState {
            id,
            board: Board::new(),
            kind: NONE,
            x: 0,
            y: 0,
            rot: 0,
            next,
            bag,
            level,
            lines: 0,
            alive: true,
            frozen: false,
            inventory: Vec::new(),
            spawn_wait: 0,
            drop_acc: 0,
            special_acc: 0,
        }
    }
}

pub struct Game {
    pub slots: [Option<PlayerState>; MAX_SLOTS],
    pub rng: Rng,
    pub tick: u32,
    pub events: Vec<String>,
    pub over: bool,
    pub winner: u8,
    pub options: GameOptions,
    last_update: Instant,
}

impl Game {
    pub fn new(ids: &[u8], seed: u64, options: GameOptions) -> Game {
        let mut game = Game {
            slots: Default::default(),
            rng: Rng::new(seed),
            tick: 0,
            events: Vec::new(),
            over: false,
            winner: 0,
            options: options.clamped(),
            last_update: Instant::now(),
        };
        for &id in ids {
            if id == 0 || id as usize > MAX_SLOTS {
                continue;
            }
            let idx = id as usize - 1;
            let level = game.options.initial_level;
            let mut bag = PieceBag::new(&mut game.rng);
            let first = bag.next(&mut game.rng);
            game.slots[idx] = Some(PlayerState::new(id, first, level, bag));
            game.seed_starting_rows(idx);
            game.spawn(idx);
        }
        game
    }

    fn seed_starting_rows(&mut self, idx: usize) {
        let rows = self.options.starting_lines.min(H as u32) as usize;
        if rows == 0 {
            return;
        }
        let Some(p) = self.slots[idx].as_mut() else {
            return;
        };
        for row in 0..rows {
            let y = H - 1 - row;
            for x in 0..W {
                p.board.cells[y * W + x] = 1 + self.rng.below(6) as u8;
            }
            let hole = self.rng.below(W as u32) as usize;
            p.board.cells[y * W + hole] = 0;
        }
    }

    fn spawn(&mut self, idx: usize) {
        let mut lost = false;
        if let Some(p) = self.slots[idx].as_mut() {
            if !p.alive {
                return;
            }
            p.kind = p.next;
            p.next = p.bag.next(&mut self.rng);
            p.rot = 0;
            p.x = 4;
            p.y = 0;
            p.drop_acc = 0;
            if collides(&p.board, p.kind, 0, 4, 0) {
                p.alive = false;
                p.kind = NONE;
                lost = true;
            }
        }
        if lost {
            let id = idx as u8 + 1;
            self.events
                .push(format!("{{\"t\":\"lost\",\"p\":{}}}", id));
            self.check_over();
        }
    }

    fn check_over(&mut self) {
        if self.over {
            return;
        }
        let alive: Vec<u8> = self
            .slots
            .iter()
            .flatten()
            .filter(|p| p.alive)
            .map(|p| p.id)
            .collect();
        if alive.len() <= 1 {
            self.over = true;
            self.winner = alive.first().copied().unwrap_or(0);
            self.events
                .push(format!("{{\"t\":\"over\",\"winner\":{}}}", self.winner));
        }
    }

    fn kill(&mut self, idx: usize) {
        let mut was_alive = false;
        if let Some(p) = self.slots[idx].as_mut() {
            if p.alive {
                p.alive = false;
                p.kind = NONE;
                was_alive = true;
            }
        }
        if was_alive {
            let id = idx as u8 + 1;
            self.events
                .push(format!("{{\"t\":\"lost\",\"p\":{}}}", id));
            self.check_over();
        }
    }

    fn revalidate_piece(&mut self, idx: usize) {
        let mut kill = false;
        if let Some(p) = self.slots[idx].as_mut() {
            if !p.alive || p.kind == NONE {
                return;
            }
            if !collides(&p.board, p.kind, p.rot, p.x, p.y) {
                return;
            }
            let mut moved = false;
            for ny in (0..p.y).rev() {
                if !collides(&p.board, p.kind, p.rot, p.x, ny) {
                    p.y = ny;
                    moved = true;
                    break;
                }
            }
            if !moved {
                kill = true;
            }
        }
        if kill {
            self.kill(idx);
        }
    }

    pub fn add_lines(&mut self, idx: usize, n: u32, _source: Option<u8>) {
        let mut lost = false;
        {
            let Some(p) = self.slots[idx].as_mut() else {
                return;
            };
            if !p.alive {
                return;
            }
            for _ in 0..n {
                if (0..W).any(|x| p.board.cells[x] != 0) {
                    lost = true;
                    break;
                }
                for y in 0..H - 1 {
                    for x in 0..W {
                        p.board.cells[y * W + x] = p.board.cells[(y + 1) * W + x];
                    }
                }
                for x in 0..W {
                    p.board.cells[(H - 1) * W + x] = 1 + self.rng.below(6) as u8;
                }
                let hole = self.rng.below(W as u32) as usize;
                p.board.cells[(H - 1) * W + hole] = 0;
                p.board.version = p.board.version.wrapping_add(1);
            }
        }
        if lost {
            self.kill(idx);
        } else {
            self.revalidate_piece(idx);
        }
    }

    fn lock_piece(&mut self, idx: usize) {
        let pid = idx as u8 + 1;
        let initial_level = self.options.initial_level;
        let lines_per_level = self.options.lines_per_level.max(1);
        let level_increment = self.options.level_increment.max(1);
        let capacity = self.options.special_capacity.max(1) as usize;
        let special_every = self.options.special_every;
        let special_count = self.options.special_count;
        let (cleared, gained) = {
            let Some(p) = self.slots[idx].as_mut() else {
                return;
            };
            if !p.alive || p.kind == NONE {
                return;
            }
            place(&mut p.board, p.kind, p.rot, p.x, p.y);
            p.kind = NONE;
            p.board.version = p.board.version.wrapping_add(1);
            let (cleared, gained) = clear_lines(&mut p.board, true);
            if cleared > 0 {
                p.lines += cleared as u32;
                p.level = (initial_level + (p.lines / lines_per_level) * level_increment).min(30);
                if special_every > 0 {
                    p.special_acc += cleared as u32;
                }
                for sp in &gained {
                    if p.inventory.len() < capacity {
                        p.inventory.push(*sp);
                    }
                }
            }
            p.spawn_wait = 180;
            p.drop_acc = 0;
            (cleared, gained)
        };

        if cleared > 0 {
            let mut to_place = 0u32;
            if special_every > 0 {
                if let Some(p) = self.slots[idx].as_mut() {
                    while p.special_acc >= special_every {
                        p.special_acc -= special_every;
                        to_place += special_count;
                    }
                }
            }
            for _ in 0..to_place {
                if let Some(p) = self.slots[idx].as_mut() {
                    place_special(&mut p.board, &mut self.rng);
                }
            }

            self.events.push(format!(
                "{{\"t\":\"lines\",\"p\":{},\"n\":{},\"sp\":{}}}",
                pid,
                cleared,
                gained.len()
            ));

            let attack = match cleared {
                2 => 1,
                3 => 2,
                4 => 4,
                _ => 0,
            };
            if attack > 0 && self.options.line_attacks {
                let targets: Vec<usize> = (0..MAX_SLOTS)
                    .filter(|&i| i != idx)
                    .filter(|&i| self.slots[i].as_ref().map(|p| p.alive).unwrap_or(false))
                    .collect();
                for other in targets {
                    self.events.push(format!(
                        "{{\"t\":\"atk\",\"from\":{},\"to\":{},\"n\":{}}}",
                        pid,
                        other as u8 + 1,
                        attack
                    ));
                    self.add_lines(other, attack, Some(pid));
                }
            }
        }
    }

    fn step_player(&mut self, idx: usize, dt: u32) {
        let mut spawn_now = false;
        let mut lock_now = false;
        {
            let Some(p) = self.slots[idx].as_mut() else {
                return;
            };
            if !p.alive || p.frozen {
                return;
            }
            if p.spawn_wait > 0 {
                p.spawn_wait = p.spawn_wait.saturating_sub(dt);
                if p.spawn_wait == 0 {
                    spawn_now = true;
                }
            } else if p.kind == NONE {
                spawn_now = true;
            } else {
                p.drop_acc += dt;
                let gravity = gravity_ms(p.level);
                while p.drop_acc >= gravity {
                    p.drop_acc -= gravity;
                    if !collides(&p.board, p.kind, p.rot, p.x, p.y + 1) {
                        p.y += 1;
                    } else {
                        lock_now = true;
                        break;
                    }
                }
            }
        }
        if spawn_now {
            self.spawn(idx);
        }
        if lock_now {
            self.lock_piece(idx);
        }
    }

    pub fn tick(&mut self, now: Instant) {
        let dt = now
            .saturating_duration_since(self.last_update)
            .as_millis()
            .min(150) as u32;
        self.last_update = now;
        if self.over || dt == 0 {
            return;
        }
        self.tick = self.tick.wrapping_add(1);
        for idx in 0..MAX_SLOTS {
            if self.over {
                break;
            }
            if self.slots[idx].is_some() {
                self.step_player(idx, dt);
            }
        }
    }

    pub fn force_lost(&mut self, idx: usize) {
        self.kill(idx);
    }

    pub fn input(&mut self, idx: usize, action: u8) {
        if self.over {
            return;
        }
        let mut lock_now = false;
        {
            let Some(p) = self.slots[idx].as_mut() else {
                return;
            };
            if !p.alive || p.frozen || p.spawn_wait > 0 || p.kind == NONE {
                return;
            }
            match action {
                0 => {
                    if !collides(&p.board, p.kind, p.rot, p.x - 1, p.y) {
                        p.x -= 1;
                    }
                }
                1 => {
                    if !collides(&p.board, p.kind, p.rot, p.x + 1, p.y) {
                        p.x += 1;
                    }
                }
                2 | 3 => {
                    let count = ORIENT_COUNT[p.kind as usize] as i32;
                    let dir = if action == 2 { 1 } else { -1 };
                    let new_rot = ((p.rot as i32 + dir).rem_euclid(count)) as u8;
                    for dx in [0, -1, 1, -2, 2] {
                        if !collides(&p.board, p.kind, new_rot, p.x + dx, p.y) {
                            p.x += dx;
                            p.rot = new_rot;
                            break;
                        }
                    }
                }
                4 => {
                    if !collides(&p.board, p.kind, p.rot, p.x, p.y + 1) {
                        p.y += 1;
                    }
                }
                5 => {
                    while !collides(&p.board, p.kind, p.rot, p.x, p.y + 1) {
                        p.y += 1;
                    }
                    lock_now = true;
                }
                _ => {}
            }
        }
        if lock_now {
            self.lock_piece(idx);
        }
    }

    pub fn use_special(&mut self, from: usize, sp: u8, target: usize) -> Result<(), String> {
        if self.over {
            return Err("game is over".into());
        }
        if sp >= SPECIAL_KINDS {
            return Err("unknown special".into());
        }
        if from >= MAX_SLOTS || target >= MAX_SLOTS {
            return Err("bad player".into());
        }
        let from_id = from as u8 + 1;
        let target_id = target as u8 + 1;

        {
            let Some(p) = self.slots[from].as_ref() else {
                return Err("not in game".into());
            };
            if !p.alive {
                return Err("you are out".into());
            }
            if !p.inventory.contains(&sp) {
                return Err("special not in inventory".into());
            }
        }
        {
            let Some(t) = self.slots[target].as_ref() else {
                return Err("target not in game".into());
            };
            if !t.alive {
                return Err("target is out".into());
            }
        }
        if sp == SP_SWITCH && from == target {
            return Err("pick another field to switch with".into());
        }

        if let Some(p) = self.slots[from].as_mut() {
            if let Some(pos) = p.inventory.iter().position(|&x| x == sp) {
                p.inventory.remove(pos);
            }
        }

        match sp {
            SP_ADD_LINE => self.add_lines(target, 1, Some(from_id)),
            SP_CLEAR_LINE => {
                if let Some(t) = self.slots[target].as_mut() {
                    for y in (1..H).rev() {
                        for x in 0..W {
                            t.board.cells[y * W + x] = t.board.cells[(y - 1) * W + x];
                        }
                    }
                    for x in 0..W {
                        t.board.cells[x] = 0;
                    }
                    t.board.version = t.board.version.wrapping_add(1);
                }
                self.revalidate_piece(target);
            }
            SP_NUKE => {
                if let Some(t) = self.slots[target].as_mut() {
                    t.board.cells = [0; CELLS];
                    t.board.version = t.board.version.wrapping_add(1);
                }
                self.revalidate_piece(target);
            }
            SP_CLEAR_RANDOM => {
                if let Some(t) = self.slots[target].as_mut() {
                    for _ in 0..10 {
                        let idx = self.rng.below(CELLS as u32) as usize;
                        t.board.cells[idx] = 0;
                    }
                    t.board.version = t.board.version.wrapping_add(1);
                }
                self.revalidate_piece(target);
            }
            SP_SWITCH => {
                let mut boards: Vec<Board> = Vec::new();
                for slot in [from, target] {
                    if let Some(p) = self.slots[slot].as_mut() {
                        boards.push(std::mem::replace(&mut p.board, Board::new()));
                    }
                }
                if boards.len() == 2 {
                    let second = boards.pop().unwrap();
                    let first = boards.pop().unwrap();
                    if let Some(p) = self.slots[from].as_mut() {
                        p.board = second;
                        p.board.version = p.board.version.wrapping_add(1);
                        p.kind = NONE;
                        p.spawn_wait = 220;
                        p.drop_acc = 0;
                    }
                    if let Some(p) = self.slots[target].as_mut() {
                        p.board = first;
                        p.board.version = p.board.version.wrapping_add(1);
                        p.kind = NONE;
                        p.spawn_wait = 220;
                        p.drop_acc = 0;
                    }
                }
            }
            SP_CLEAR_SPECIALS => {
                if let Some(t) = self.slots[target].as_mut() {
                    for cell in t.board.cells.iter_mut() {
                        if is_special(*cell) {
                            *cell = 1 + self.rng.below(6) as u8;
                        }
                    }
                    t.board.version = t.board.version.wrapping_add(1);
                }
                self.revalidate_piece(target);
            }
            SP_GRAVITY => {
                if let Some(t) = self.slots[target].as_mut() {
                    compact_gravity(&mut t.board);
                }
                self.revalidate_piece(target);
            }
            SP_BLOCKQUAKE => {
                if let Some(t) = self.slots[target].as_mut() {
                    for y in 0..H {
                        let mut s = 0i32;
                        let i = self.rng.below(22);
                        if i < 1 {
                            s += 1;
                        }
                        if i < 4 {
                            s += 1;
                        }
                        if i < 11 {
                            s += 1;
                        }
                        if self.rng.below(2) == 1 {
                            s = -s;
                        }
                        if s != 0 {
                            shift_row(&mut t.board, y, s);
                        }
                    }
                    t.board.version = t.board.version.wrapping_add(1);
                }
                self.revalidate_piece(target);
                self.clear_after_special(target);
            }
            SP_BLOCKBOMB => {
                if let Some(t) = self.slots[target].as_mut() {
                    block_bomb(&mut t.board, &mut self.rng);
                }
                self.revalidate_piece(target);
                self.clear_after_special(target);
            }
            _ => {}
        }

        self.events.push(format!(
            "{{\"t\":\"sp\",\"from\":{},\"to\":{},\"sp\":{}}}",
            from_id, target_id, sp
        ));
        self.check_over();
        Ok(())
    }

    fn clear_after_special(&mut self, idx: usize) {
        if let Some(p) = self.slots[idx].as_mut() {
            let (cleared, _) = clear_lines(&mut p.board, false);
            if cleared > 0 {
                p.board.version = p.board.version.wrapping_add(1);
                self.events.push(format!(
                    "{{\"t\":\"lines\",\"p\":{},\"n\":{},\"sp\":0}}",
                    idx as u8 + 1,
                    cleared
                ));
            }
        }
    }
}

fn shift_row(board: &mut Board, y: usize, d: i32) {
    if d > 0 {
        let d = d as usize;
        for x in (d..W).rev() {
            board.cells[y * W + x] = board.cells[y * W + x - d];
        }
        for x in 0..d.min(W) {
            board.cells[y * W + x] = 0;
        }
    } else if d < 0 {
        let d = (-d) as usize;
        for x in 0..W.saturating_sub(d) {
            board.cells[y * W + x] = board.cells[y * W + x + d];
        }
        for x in W.saturating_sub(d)..W {
            board.cells[y * W + x] = 0;
        }
    }
}

fn block_bomb(board: &mut Board, rng: &mut Rng) {
    let bomb = special_cell(SP_BLOCKBOMB);
    let mut bombs = Vec::new();
    for (i, cell) in board.cells.iter().enumerate() {
        if *cell == bomb {
            bombs.push(i);
        }
    }
    if bombs.is_empty() {
        return;
    }
    let mut scatter: Vec<u8> = Vec::new();
    for idx in bombs {
        if board.cells[idx] != bomb {
            continue;
        }
        board.cells[idx] = 0;
        let x = idx % W;
        let y = idx / W;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || nx >= W as i32 || ny < 0 || ny >= H as i32 {
                    continue;
                }
                let ni = ny as usize * W + nx as usize;
                let cell = board.cells[ni];
                if cell == bomb {
                    board.cells[ni] = 0;
                } else {
                    board.cells[ni] = 0;
                    scatter.push(cell);
                }
            }
        }
    }
    for cell in scatter {
        let idx = 6 * W + rng.below(((H - 6) * W) as u32) as usize;
        board.cells[idx] = cell;
    }
    board.version = board.version.wrapping_add(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_game_opts(ids: &[u8], opts: GameOptions) -> Game {
        let mut game = Game::new(ids, 12345, opts);
        for slot in game.slots.iter_mut().flatten() {
            slot.board.cells = [0; CELLS];
            slot.kind = NONE;
            slot.inventory.clear();
        }
        game
    }

    fn empty_game(ids: &[u8]) -> Game {
        empty_game_opts(ids, GameOptions::default())
    }

    #[test]
    fn seven_bag_deals_each_piece_once() {
        let mut rng = Rng::new(123);
        let mut bag = PieceBag::new(&mut rng);
        for _ in 0..50 {
            let mut seen = [0u8; 7];
            for _ in 0..7 {
                let piece = bag.next(&mut rng);
                assert!(piece < 7);
                seen[piece as usize] += 1;
            }
            assert_eq!(seen, [1u8; 7]);
        }
    }

    #[test]
    fn game_spawns_from_the_seven_bag() {
        let mut game = Game::new(&[1, 2], 12345, GameOptions::default());
        let mut seen = Vec::new();
        for _ in 0..7 {
            let kind = game.slots[0].as_ref().unwrap().kind;
            assert!(kind < 7);
            seen.push(kind);
            game.slots[0].as_mut().unwrap().board.cells = [0; CELLS];
            game.input(0, 5);
            for _ in 0..2 {
                let now = game.last_update + std::time::Duration::from_millis(100);
                game.tick(now);
            }
            assert!(game.slots[0].as_ref().unwrap().alive);
        }
        seen.sort_unstable();
        assert_eq!(seen, (0u8..7).collect::<Vec<_>>());
    }

    #[test]
    fn line_clear_collects_specials() {
        let mut board = Board::new();
        for x in 0..W {
            board.cells[(H - 2) * W + x] = 1;
        }
        board.cells[(H - 2) * W + 3] = special_cell(SP_SWITCH);
        let (n, gained) = clear_lines(&mut board, true);
        assert_eq!(n, 1);
        assert_eq!(gained, vec![SP_SWITCH]);
        assert!(board.cells[(H - 2) * W..(H - 1) * W]
            .iter()
            .all(|&c| c == 0));
    }

    #[test]
    fn add_lines_pushes_and_has_one_hole() {
        let mut game = empty_game(&[1, 2]);
        game.add_lines(0, 2, None);
        let board = &game.slots[0].as_ref().unwrap().board;
        for y in [H - 2, H - 1] {
            let holes = (0..W).filter(|&x| board.cells[y * W + x] == 0).count();
            assert_eq!(holes, 1, "row {y}");
        }
    }

    #[test]
    fn add_lines_at_top_kills() {
        let mut game = empty_game(&[1, 2]);
        game.slots[0].as_mut().unwrap().board.cells[0] = 1;
        game.add_lines(0, 1, None);
        assert!(!game.slots[0].as_ref().unwrap().alive);
    }

    #[test]
    fn hard_drop_locks_and_spawns_delay() {
        let mut game = empty_game(&[1, 2]);
        {
            let p = game.slots[0].as_mut().unwrap();
            p.kind = 0;
            p.rot = 0;
            p.x = 4;
            p.y = 0;
        }
        game.input(0, 5);
        let p = game.slots[0].as_ref().unwrap();
        assert_eq!(p.kind, NONE);
        assert!(p.spawn_wait > 0);
        assert_eq!(p.board.cells.iter().filter(|&&c| c != 0).count(), 4);
    }

    #[test]
    fn wall_kick_works() {
        let mut game = empty_game(&[1, 2]);
        {
            let p = game.slots[0].as_mut().unwrap();
            p.kind = 0;
            p.rot = 1;
            p.x = 9;
            p.y = 0;
        }
        game.input(0, 2);
        let p = game.slots[0].as_ref().unwrap();
        assert_eq!(p.rot, 0);
        assert_eq!(p.x, 8);
    }

    #[test]
    fn four_line_clear_attacks() {
        let mut game = empty_game(&[1, 2]);
        {
            let p = game.slots[0].as_mut().unwrap();
            for y in H - 4..H {
                for x in 0..W {
                    if x != 5 {
                        p.board.cells[y * W + x] = 1;
                    }
                }
            }
            p.kind = 0;
            p.rot = 1;
            p.x = 3; // vertical I in column 5
            p.y = H as i32 - 4;
        }
        game.input(0, 5);
        assert!(game.slots[0].as_ref().unwrap().lines >= 4);
        let opponent = game.slots[1].as_ref().unwrap();
        assert!(opponent.board.cells[(H - 1) * W..H * W]
            .iter()
            .any(|&c| c != 0));
        assert!(game.events.iter().any(|e| e.contains("\"t\":\"atk\"")));
    }

    #[test]
    fn switch_swaps_and_consumes() {
        let mut game = empty_game(&[1, 2]);
        game.slots[0].as_mut().unwrap().inventory.push(SP_SWITCH);
        game.slots[0].as_mut().unwrap().board.cells[0] = 3;
        game.slots[1].as_mut().unwrap().board.cells[1] = 4;
        game.use_special(0, SP_SWITCH, 1).unwrap();
        assert_eq!(game.slots[0].as_ref().unwrap().board.cells[1], 4);
        assert_eq!(game.slots[1].as_ref().unwrap().board.cells[0], 3);
        assert!(game.slots[0].as_ref().unwrap().inventory.is_empty());
    }

    #[test]
    fn gravity_special_compacts_columns() {
        let mut game = empty_game(&[1, 2]);
        game.slots[0].as_mut().unwrap().inventory.push(SP_GRAVITY);
        {
            let board = &mut game.slots[0].as_mut().unwrap().board;
            board.cells[0] = 1;
            board.cells[W] = 1;
        }
        game.use_special(0, SP_GRAVITY, 0).unwrap();
        let board = &game.slots[0].as_ref().unwrap().board;
        assert_eq!(board.cells[0], 0);
        assert_eq!(board.cells[(H - 1) * W], 1);
        assert_eq!(board.cells[(H - 2) * W], 1);
    }

    #[test]
    fn clear_specials_converts_cells() {
        let mut game = empty_game(&[1, 2]);
        game.slots[0]
            .as_mut()
            .unwrap()
            .inventory
            .push(SP_CLEAR_SPECIALS);
        game.slots[0].as_mut().unwrap().board.cells[0] = special_cell(SP_ADD_LINE);
        game.use_special(0, SP_CLEAR_SPECIALS, 0).unwrap();
        let cell = game.slots[0].as_ref().unwrap().board.cells[0];
        assert!((1..=6).contains(&cell));
    }

    #[test]
    fn options_clamp_to_ranges() {
        let opts = GameOptions {
            initial_level: 0,
            starting_lines: 99,
            lines_per_level: 0,
            level_increment: 9,
            special_every: 99,
            special_count: 9,
            special_capacity: 99,
            line_attacks: false,
        }
        .clamped();
        assert_eq!(opts.initial_level, 1);
        assert_eq!(opts.starting_lines, 12);
        assert_eq!(opts.lines_per_level, 1);
        assert_eq!(opts.level_increment, 5);
        assert_eq!(opts.special_every, 50);
        assert_eq!(opts.special_count, 5);
        assert_eq!(opts.special_capacity, 8);
        assert!(!opts.line_attacks);
    }

    #[test]
    fn options_apply_start_level_and_garbage() {
        let mut opts = GameOptions::default();
        opts.initial_level = 4;
        opts.starting_lines = 3;
        let game = Game::new(&[1, 2], 77, opts);
        for slot in game.slots.iter().flatten() {
            assert_eq!(slot.level, 4);
            for y in [H - 3, H - 2, H - 1] {
                let filled = (0..W)
                    .filter(|&x| slot.board.cells[y * W + x] != 0)
                    .count();
                assert_eq!(filled, 11, "row {y}");
            }
            for y in 0..H - 3 {
                let filled = (0..W)
                    .filter(|&x| slot.board.cells[y * W + x] != 0)
                    .count();
                assert_eq!(filled, 0, "row {y} should be empty");
            }
        }
    }

    #[test]
    fn options_disable_attacks_and_change_level_curve() {
        let mut opts = GameOptions::default();
        opts.initial_level = 3;
        opts.lines_per_level = 2;
        opts.level_increment = 2;
        opts.line_attacks = false;
        let mut game = empty_game_opts(&[1, 2], opts);
        {
            let p = game.slots[0].as_mut().unwrap();
            for y in H - 4..H {
                for x in 0..W {
                    if x != 5 {
                        p.board.cells[y * W + x] = 1;
                    }
                }
            }
            p.kind = 0;
            p.rot = 1;
            p.x = 3;
            p.y = H as i32 - 4;
        }
        game.input(0, 5);
        let p = game.slots[0].as_ref().unwrap();
        assert!(p.lines >= 4);
        assert_eq!(p.level, 3 + (p.lines / 2) * 2);
        let opponent = game.slots[1].as_ref().unwrap();
        assert_eq!(opponent.board.cells.iter().filter(|&&c| c != 0).count(), 0);
        assert!(!game.events.iter().any(|e| e.contains("\"t\":\"atk\"")));
    }

    #[test]
    fn options_control_capacity_and_field_specials() {
        let mut opts = GameOptions::default();
        opts.special_every = 1;
        opts.special_count = 1;
        opts.special_capacity = 1;
        opts.line_attacks = false;
        let mut game = empty_game_opts(&[1, 2], opts);
        {
            let p = game.slots[0].as_mut().unwrap();
            for y in [H - 2, H - 1] {
                for x in 0..W {
                    if x != 5 && x != 6 {
                        p.board.cells[y * W + x] = 1;
                    }
                }
            }
            p.board.cells[(H - 1) * W] = special_cell(SP_SWITCH);
            p.board.cells[(H - 1) * W + 1] = special_cell(SP_GRAVITY);
            p.board.cells[(H - 3) * W] = 1;
            p.kind = 1;
            p.rot = 0;
            p.x = 5;
            p.y = H as i32 - 2;
        }
        game.input(0, 5);
        let p = game.slots[0].as_ref().unwrap();
        assert_eq!(p.lines, 2);
        assert_eq!(p.inventory.len(), 1);
        assert_eq!(
            p.board.cells.iter().filter(|&&c| is_special(c)).count(),
            1,
            "two triggers should seed, but only one normal cell was available"
        );
    }

    #[test]
    fn rolling_until_game_over() {
        let mut game = empty_game(&[1, 2]);
        for _ in 0..200 {
            if game.over {
                break;
            }
            game.input(0, 5);
            let now = game.last_update + std::time::Duration::from_millis(300);
            game.tick(now);
        }
        assert!(game.over);
        assert_eq!(game.winner, 2);
    }
}
