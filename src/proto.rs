//! Wire protocol.
//!
//! Client -> server (binary WebSocket frames or POST bodies):
//!   0x01 MOVE     u8 action: 0 left, 1 right, 2 rotate cw, 3 rotate ccw,
//!                          4 soft drop, 5 hard drop
//!   0x02 SPECIAL  u8 special, u8 target slot (1..=6)
//!   0x03 START
//!   0x04 CHAT     utf-8
//!   0x05 PING     8-byte nonce
//!   0x06 LEAVE
//!   0x07 RESYNC
//!   0x08 OPTIONS  u8 initial level, u8 starting lines, u8 lines per level,
//!                 u8 level increment, u8 special every, u8 special count,
//!                 u8 special capacity, u8 line attacks (0/1)
//!   0x09 READY    u8 (0 = not ready, 1 = ready)
//!
//! Server -> client:
//!   0x80 LOBBY     json
//!   0x81 STATE     u32 tick, 6 x slot state
//!   0x82 BOARD     u8 slot, u32 version, 132 packed nibbles
//!   0x83 EVENT     json
//!   0x84 SNAPSHOT  u32 tick, 6 x (u8 present, u32 version, 132 packed nibbles)
//!   0x85 PONG      echoed nonce
//!   0x86 HELLO     json {you, token, game}

use crate::game::{Board, Game, GameOptions, CELLS, MAX_SLOTS};

pub const C_MOVE: u8 = 0x01;
pub const C_SPECIAL: u8 = 0x02;
pub const C_START: u8 = 0x03;
pub const C_CHAT: u8 = 0x04;
pub const C_PING: u8 = 0x05;
pub const C_LEAVE: u8 = 0x06;
pub const C_RESYNC: u8 = 0x07;
pub const C_OPTIONS: u8 = 0x08;
pub const C_READY: u8 = 0x09;

pub const S_LOBBY: u8 = 0x80;
pub const S_STATE: u8 = 0x81;
pub const S_BOARD: u8 = 0x82;
pub const S_EVENT: u8 = 0x83;
pub const S_SNAPSHOT: u8 = 0x84;
pub const S_PONG: u8 = 0x85;
pub const S_HELLO: u8 = 0x86;

pub enum ClientMsg {
    Move(u8),
    Special { sp: u8, target: u8 },
    Start,
    Chat(String),
    Ping(Vec<u8>),
    Leave,
    Resync,
    Options(GameOptions),
    Ready(bool),
}

pub fn parse_client(buf: &[u8]) -> Option<ClientMsg> {
    let (op, rest) = buf.split_first()?;
    match *op {
        C_MOVE => Some(ClientMsg::Move(*rest.first()?)),
        C_SPECIAL => {
            if rest.len() < 2 {
                return None;
            }
            Some(ClientMsg::Special {
                sp: rest[0],
                target: rest[1],
            })
        }
        C_START => Some(ClientMsg::Start),
        C_CHAT => {
            let text = String::from_utf8_lossy(rest).trim().to_string();
            if text.is_empty() {
                None
            } else {
                let text: String = text.chars().take(240).collect();
                Some(ClientMsg::Chat(text))
            }
        }
        C_PING => Some(ClientMsg::Ping(rest.to_vec())),
        C_LEAVE => Some(ClientMsg::Leave),
        C_RESYNC => Some(ClientMsg::Resync),
        C_OPTIONS => {
            if rest.len() < 8 {
                return None;
            }
            Some(ClientMsg::Options(GameOptions {
                initial_level: rest[0] as u32,
                starting_lines: rest[1] as u32,
                lines_per_level: rest[2] as u32,
                level_increment: rest[3] as u32,
                special_every: rest[4] as u32,
                special_count: rest[5] as u32,
                special_capacity: rest[6] as u32,
                line_attacks: rest[7] != 0,
            }))
        }
        C_READY => Some(ClientMsg::Ready(*rest.first()? != 0)),
        _ => None,
    }
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn packed_board(board: &Board) -> Vec<u8> {
    let mut out = Vec::with_capacity(CELLS / 2);
    for pair in board.cells.chunks_exact(2) {
        out.push((pair[0] << 4) | (pair[1] & 0x0f));
    }
    out
}

pub fn state_frame(game: &Game) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + MAX_SLOTS * 18);
    out.push(S_STATE);
    push_u32(&mut out, game.tick);
    for slot in game.slots.iter() {
        match slot {
            Some(p) => {
                out.push(1);
                out.push(p.alive as u8);
                out.push(p.level.min(255) as u8);
                push_u16(&mut out, p.lines.min(u16::MAX as u32) as u16);
                out.push(p.kind);
                out.push(p.x as i8 as u8);
                out.push(p.y as i8 as u8);
                out.push(p.rot);
                out.push(p.next);
                out.push(p.inventory.len() as u8);
                out.extend_from_slice(&p.inventory);
            }
            None => out.push(0),
        }
    }
    out
}

pub fn board_frame(slot: u8, board: &Board) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + CELLS / 2);
    out.push(S_BOARD);
    out.push(slot);
    push_u32(&mut out, board.version);
    out.extend_from_slice(&packed_board(board));
    out
}

pub fn snapshot_frame(game: &Game) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + MAX_SLOTS * (5 + CELLS / 2));
    out.push(S_SNAPSHOT);
    push_u32(&mut out, game.tick);
    for slot in game.slots.iter() {
        match slot {
            Some(p) => {
                out.push(1);
                push_u32(&mut out, p.board.version);
                out.extend_from_slice(&packed_board(&p.board));
            }
            None => out.push(0),
        }
    }
    out
}

pub fn lobby_frame(json: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(json.len() + 1);
    out.push(S_LOBBY);
    out.extend_from_slice(json.as_bytes());
    out
}

pub fn event_frame(json: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(json.len() + 1);
    out.push(S_EVENT);
    out.extend_from_slice(json.as_bytes());
    out
}

pub fn hello_frame(game: &str, you: u8, token: &str) -> Vec<u8> {
    let json = format!(
        "{{\"you\":{},\"token\":\"{}\",\"game\":\"{}\"}}",
        you,
        crate::util::json_escape(token),
        crate::util::json_escape(game)
    );
    let mut out = Vec::with_capacity(json.len() + 1);
    out.push(S_HELLO);
    out.extend_from_slice(json.as_bytes());
    out
}

pub fn pong_frame(nonce: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nonce.len() + 1);
    out.push(S_PONG);
    out.extend_from_slice(nonce);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{special_cell, SP_ADD_LINE};

    #[test]
    fn board_pack_roundtrip() {
        let mut board = Board::new();
        board.cells[0] = 1;
        board.cells[1] = special_cell(SP_ADD_LINE);
        board.cells[CELLS - 1] = 6;
        let packed = packed_board(&board);
        assert_eq!(packed.len(), CELLS / 2);
        assert_eq!(packed[0], 0x17);
        assert_eq!(packed[CELLS / 2 - 1] & 0x0f, 6);
    }

    #[test]
    fn client_parsing() {
        assert!(matches!(parse_client(&[C_MOVE, 3]), Some(ClientMsg::Move(3))));
        match parse_client(&[C_SPECIAL, 4, 2]) {
            Some(ClientMsg::Special { sp, target }) => {
                assert_eq!(sp, 4);
                assert_eq!(target, 2);
            }
            _ => panic!("bad parse"),
        }
        assert!(parse_client(&[C_CHAT]).is_none());
        assert!(matches!(
            parse_client(&[C_CHAT, b'h', b'i']),
            Some(ClientMsg::Chat(ref s)) if s == "hi"
        ));
        match parse_client(&[C_OPTIONS, 5, 3, 8, 2, 2, 2, 7, 0]) {
            Some(ClientMsg::Options(opts)) => {
                assert_eq!(opts.initial_level, 5);
                assert_eq!(opts.starting_lines, 3);
                assert_eq!(opts.lines_per_level, 8);
                assert_eq!(opts.level_increment, 2);
                assert_eq!(opts.special_every, 2);
                assert_eq!(opts.special_count, 2);
                assert_eq!(opts.special_capacity, 7);
                assert!(!opts.line_attacks);
            }
            _ => panic!("bad options parse"),
        }
        assert!(matches!(
            parse_client(&[C_READY, 1]),
            Some(ClientMsg::Ready(true))
        ));
        assert!(matches!(
            parse_client(&[C_READY, 0]),
            Some(ClientMsg::Ready(false))
        ));
    }
}
