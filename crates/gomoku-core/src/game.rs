use crate::board::{Board, Move, Piece};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSnapshot {
    pub board: Board,
    pub player_turn: Piece,
    pub my_color: Piece,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext {
    pub room_name: Option<String>,
    pub is_room_owner: bool,
    pub move_time_seconds: Option<u32>,
}

impl SessionContext {
    pub fn new(is_room_owner: bool) -> Self {
        Self {
            room_name: None,
            is_room_owner,
            move_time_seconds: None,
        }
    }
}

impl GameSnapshot {
    pub fn new(board: Board, player_turn: Piece, my_color: Piece) -> Self {
        Self {
            board,
            player_turn,
            my_color,
        }
    }

    pub fn with_default_board(player_turn: Piece, my_color: Piece) -> Self {
        Self::new(Board::new(), player_turn, my_color)
    }

    pub fn apply_move(&mut self, mv: Move) -> Result<(), crate::board::BoardError> {
        self.board.apply_move(mv)
    }

    pub fn is_my_turn(&self) -> bool {
        self.player_turn == self.my_color
    }
}
