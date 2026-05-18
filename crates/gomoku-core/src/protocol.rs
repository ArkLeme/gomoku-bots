use crate::board::{Board, Move, Piece, Position};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Role {
    Player,
    Observer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum UserType {
    Registered,
    Guest,
    Bot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    pub color: Piece,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    pub role: Role,
    pub game_info: Option<GameInfo>,
    #[serde(rename = "type")]
    pub user_type: UserType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coordinate {
    pub row: usize,
    pub column: usize,
}

impl From<Position> for Coordinate {
    fn from(value: Position) -> Self {
        Self {
            row: value.row,
            column: value.column,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayersTime {
    pub black_time_ms: u64,
    pub white_time_ms: u64,
    pub turn_start_date: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LobbyData {
    pub users: Vec<User>,
    pub is_room_owner: bool,
    pub move_time_seconds: u32,
    pub initial_board_moves_history: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameData {
    pub time: PlayersTime,
    pub grid: Vec<Vec<Option<Piece>>>,
    pub player_turn: Piece,
}

impl From<&Board> for GameData {
    fn from(board: &Board) -> Self {
        let mut grid = Vec::with_capacity(crate::board::BOARD_SIZE);
        for row in 0..crate::board::BOARD_SIZE {
            let mut output_row = Vec::with_capacity(crate::board::BOARD_SIZE);
            for column in 0..crate::board::BOARD_SIZE {
                output_row.push(board.get(Position { row, column }));
            }
            grid.push(output_row);
        }

        Self {
            time: PlayersTime {
                black_time_ms: 0,
                white_time_ms: 0,
                turn_start_date: String::new(),
            },
            grid,
            player_turn: Piece::Black,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRoomResponseData {
    pub user: User,
    pub lobby_data: LobbyData,
    pub game_data: Option<GameData>,
    pub game_over_data: Option<GameOverData>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameOverData {
    pub winning_points: Option<Vec<Coordinate>>,
    pub moves_history: String,
    pub game_state: GameState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum JoinRoomError {
    RoomDoesNotExist,
    InvalidRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRoomResponse {
    pub success: bool,
    pub data: Option<JoinRoomResponseData>,
    pub error: Option<JoinRoomError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum GameState {
    NoWin,
    BlackWin,
    WhiteWin,
    Draw,
    BlackDisqualified,
    WhiteDisqualified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum RoomClosedReason {
    Inactive,
    ClosedByOwner,
    RoomOwnerLeft,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum DisqualificationReason {
    IllegalMove,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorMessage {
    #[serde(rename = "type")]
    pub error_type: String,
    pub title: String,
    pub details: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SetInitialBoardError {
    RoomNotFound,
    NotRoomOwner,
    GameAlreadyStarted,
    GameAlreadyEnded,
    EmptyMovesHistory,
    InvalidMoveFoundInMovesHistory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", rename_all_fields = "camelCase")]
#[serde(tag = "type", content = "body")]
pub enum ClientCommand {
    CreateTwoPlayerRoom { room_name: String, move_time_seconds: u32 },
    SetInitialBoard { room_id: String, moves_history: String },
    JoinRoom { room_name: String },
    StartGame { room_name: String },
    LeaveRoom { room_name: String },
    CloseRoom { room_name: String },
    PlayMove { room_name: String, row: usize, column: usize },
    SetMoveTime { room_id: String, seconds: u32 },
    ChangeRole { room_id: String, new_role: Role, preferred_color: Option<Piece> },
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "body")]
pub enum ServerEvent {
    RoomCreated(String),
    UserJoined(User, Vec<User>),
    GameStarted(Piece),
    MovePlayed(Move, String, PlayersTime),
    GameOver(GameState, String, Option<Vec<Coordinate>>),
    Timeout(String, Piece),
    RoomClosed(RoomClosedReason),
    PlayerDisqualified(User, DisqualificationReason),
    Error(ErrorMessage),
    RoleChanged(User),
    SettingsChanged(u32),
    InitialBoardChanged(String),
    PlayerLeft(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_room_response_round_trips() {
        let json = r#"{
            "success": true,
            "data": {
                "user": {
                    "id": "ID1",
                    "username": "Bot1",
                    "role": "Player",
                    "gameInfo": { "color": "Black" },
                    "type": "Guest"
                },
                "lobbyData": {
                    "users": [],
                    "isRoomOwner": true,
                    "moveTimeSeconds": 30,
                    "initialBoardMovesHistory": "0:Black:0:0;1:White:1:1;!"
                },
                "gameData": null,
                "gameOverData": null
            },
            "error": null
        }"#;

        let parsed: JoinRoomResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.data.unwrap().user.username, "Bot1");
    }

    #[test]
    fn client_command_serializes_like_signalr() {
        let command = ClientCommand::PlayMove {
            room_name: "room-1".to_string(),
            row: 1,
            column: 2,
        };

        let serialized = serde_json::to_value(command).unwrap();
        assert_eq!(serialized["type"], "PlayMove");
        assert_eq!(serialized["body"]["roomName"], "room-1");
    }

    #[test]
    fn leave_room_command_serializes_like_signalr() {
        let command = ClientCommand::LeaveRoom {
            room_name: "room-1".to_string(),
        };

        let serialized = serde_json::to_value(command).unwrap();
        assert_eq!(serialized["type"], "LeaveRoom");
        assert_eq!(serialized["body"]["roomName"], "room-1");
    }

    #[test]
    fn close_room_command_serializes_like_signalr() {
        let command = ClientCommand::CloseRoom {
            room_name: "room-1".to_string(),
        };

        let serialized = serde_json::to_value(command).unwrap();
        assert_eq!(serialized["type"], "CloseRoom");
        assert_eq!(serialized["body"]["roomName"], "room-1");
    }

    #[test]
    fn set_initial_board_command_serializes_like_signalr() {
        let command = ClientCommand::SetInitialBoard {
            room_id: "room-1".to_string(),
            moves_history: "0:Black:0:0;!".to_string(),
        };

        let serialized = serde_json::to_value(command).unwrap();
        assert_eq!(serialized["type"], "SetInitialBoard");
        assert_eq!(serialized["body"]["roomId"], "room-1");
        assert_eq!(serialized["body"]["movesHistory"], "0:Black:0:0;!");
    }
}
