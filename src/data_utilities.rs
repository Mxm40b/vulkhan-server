use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use rand;

use std::collections::HashMap;

#[derive(PartialEq, Debug)]
pub enum PacketType {
    // sent to tell players that are already/still here who joined/left:
    Join,
    Leave,
    Update,
    // serves to share token and spawn position on connect:
    ShareToken,
    // serves to tell the player who was already on the server:
    NotifyStatusOnConnect,
}

#[derive(Clone)]
pub enum SendToWhom {
    ToAll(Vec<u8>),
    ToOne(u32, Vec<u8>),
    ToAllButOne(u32, Vec<u8>),
}

impl PacketType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(PacketType::Join),
            1 => Some(PacketType::Leave),
            2 => Some(PacketType::Update),
            3 => Some(PacketType::ShareToken),
            4 => Some(PacketType::NotifyStatusOnConnect),
            _ => None,
        }
    }
    pub fn to_u8(&self) -> u8 {
        match self {
            Self::Join => 0,
            Self::Leave => 1,
            Self::Update => 2,
            Self::ShareToken => 3,
            Self::NotifyStatusOnConnect => 4,
        }
    }
}

pub enum Packet {
    Update(UpdatePacket),
    Header(PacketHeader),
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, KnownLayout, Immutable)]
pub struct PacketHeader {
    pub packet_type: u8,
    pub id: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, KnownLayout, Immutable)]
pub struct UpdatePacket {
    pub header: PacketHeader,
    // can be both token and user id depending on context
    pub position: [f32; 3],
    pub orientation: [f32; 4],
}

impl Packet {
    // convert a packet to usable data
    // because they are not interchangeable in rust
    pub fn to_data(self, id: u32) -> Option<PlayerData> {
        // TODO: make this return a Result<PlayerData, Error> so that calling code handles it
        match self {
            Packet::Update(UpdatePacket {
                header,
                position,
                orientation,
            }) => {
                if let Some(packet) = PacketType::from_u8(header.packet_type) {
                    if packet == PacketType::Update {
                        Some(PlayerData {
                            id,
                            orientation: glam::Quat::from_xyzw(
                                orientation[0],
                                orientation[1],
                                orientation[2],
                                orientation[3],
                            ),
                            position: glam::Vec3::new(position[0], position[1], position[2]),
                        })
                    } else {
                        eprintln!("error: PacketType was not of Update type, ignoring it");
                        None
                    }
                } else {
                    eprintln!("error: could not convert packet to data");
                    None
                }
            }
            Packet::Header(_) => None,
        }
    }
}

pub struct PlayerData {
    position: glam::Vec3,
    orientation: glam::Quat,
    pub id: u32,
}

impl PlayerData {
    pub fn new(id: u32) -> PlayerData {
        PlayerData {
            id,
            // starting position:
            position: glam::Vec3::new(0f32, 0f32, 0f32),
            orientation: glam::Quat::IDENTITY,
        }
    }
    // to make sure the packet sent back has the same structure
    // as the one received, we insert everything into the
    // correctly laid-out in memory
    // Packet type then take it as bytes
    pub fn to_packet_bytes(&self, packet_type: PacketType) -> Vec<u8> {
        match packet_type {
            PacketType::Update
            | PacketType::Join
            | PacketType::NotifyStatusOnConnect
            | PacketType::ShareToken => UpdatePacket {
                header: PacketHeader {
                    packet_type: packet_type.to_u8(),
                    id: self.id,
                },
                position: self.position.to_array(),
                orientation: self.orientation.to_array(),
            }
            .as_bytes()
            .to_vec(),
            // only sending header with player id when a player disconnects instead of everything
            PacketType::Leave => PacketHeader {
                packet_type: packet_type.to_u8(),
                id: self.id,
            }
            .as_bytes()
            .to_vec(),
        }
    }
    pub fn with_id(&self, id: u32) -> Self {
        PlayerData { id, ..*self }
    }
}

pub fn generate_token() -> u32 {
    rand::random::<u32>() // thank you, rand crate!
}

#[derive(Default)]
pub struct ServerState {
    pub players_data: HashMap<u32, PlayerData>,

    pub things_to_send: Vec<SendToWhom>,
}

// impl ServerState {
//     pub fn new() -> Self {
//         Self {
//             players_data: HashMap::new(),
//             things_to_send: Vec::new(),
//         }
//     }
// }

pub enum ReceiveError {
    // InvalidPacketData,
    InvalidHeader { token: u32 },
    UnreadableHeader { token: u32 },
    NonUpdateEvent { token: u32 },
    TokenMismatch { expected: u32, got: u32 },
    PlayerNotFound { token: u32 },
    UnreadableDataReceived { token: u32 },
    AssociatedDataNotFound { token: u32 },
}
pub enum ConnectError {}
// only adding for structure now, if error cases are discovered,

pub enum DisconnectError {
    InvalidToken,
    NoDataStored { token: u32 },
}

pub enum EventError {
    ReceiveError(ReceiveError),
    ConnectError(ConnectError),
    DisconnectError(DisconnectError),
}
