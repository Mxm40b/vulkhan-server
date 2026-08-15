use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use rand;

use std::collections::HashMap;

#[derive(PartialEq, Debug)]
pub enum PacketType {
    // sent to tell players that are already/still here who joined/left,
    // and also reused server-side to dump existing players' state to a
    // freshly-identified client (the wire bytes are identical either way,
    // and the client treats them identically too):
    Join,
    Leave,
    Update,
    // sent once by the client, right after connecting, carrying its
    // persistent UUID. The server assigns a session id in response to this
    // and never needs to send it back to the client (see HelloPacket).
    Hello,
}

#[derive(Clone)]
pub enum SendToWhom {
    ToAll(Vec<u8>, enet::PacketMode),
    ToOne(u32, Vec<u8>, enet::PacketMode),
    ToAllButOne(u32, Vec<u8>, enet::PacketMode),
}

impl PacketType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(PacketType::Join),
            1 => Some(PacketType::Leave),
            2 => Some(PacketType::Update),
            3 => Some(PacketType::Hello),
            _ => None,
        }
    }
    pub fn to_u8(&self) -> u8 {
        match self {
            Self::Join => 0,
            Self::Leave => 1,
            Self::Update => 2,
            Self::Hello => 3,
        }
    }
}

// pub trait Packet: Clone + DeriveEq {}
// TODO: we will eventually replace the packet enum with a trait,
// and all types of packets will have that trait.
// then, all functions that manipulate packets will instead deal with the packet trait.
// their PacketType will say wether they are of the InformationPacket
// or the PositionPacket variant,
// meaning PacketType enum will be shared between both kinds.

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
    pub position: [f32; 3],
    pub orientation: [f32; 4],
}

// Sent by the client, once, right after connecting. Not part of the
// Join/Leave/Update wire format above -- this is its own 17-byte shape.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, KnownLayout, Immutable)]
pub struct HelloPacket {
    pub packet_type: u8,
    pub uuid: [u8; 16],
}

impl Packet {
    // convert a packet to usable data
    // because they are not interchangeable in rust
    //
    // `id` is the session id the server has already assigned this peer
    // (from its Hello handshake) -- we no longer trust an id field the
    // client sends us on every packet, since the client doesn't send one
    // anymore.
    pub fn to_data(self, id: u32) -> Option<PlayerData> {
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

#[derive(Clone)]
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
    //
    // Join/Update go out as the full 33-byte UpdatePacket shape (position +
    // orientation are meaningful for both). Leave only needs the 5-byte
    // header -- there's no point spending 28 extra bytes telling everyone
    // where a player was standing when they left. The client parses the
    // header first and only reads the rest for types that need it (see
    // NetworkSession::poll on the client).
    pub fn to_packet_bytes(&self, packet_type: PacketType) -> Vec<u8> {
        match packet_type {
            PacketType::Join | PacketType::Update => UpdatePacket {
                header: PacketHeader {
                    packet_type: packet_type.to_u8(),
                    id: self.id,
                },
                position: self.position.to_array(),
                orientation: self.orientation.to_array(),
            }
            .as_bytes()
            .to_vec(),
            PacketType::Leave => PacketHeader {
                packet_type: packet_type.to_u8(),
                id: self.id,
            }
            .as_bytes()
            .to_vec(),
            PacketType::Hello => {
                unreachable!("Hello is only ever sent by the client, never by the server")
            }
        }
    }
    pub fn with_id(&self, id: u32) -> Self {
        PlayerData { id, ..*self }
    }
}

// Generates a fresh, unique session id for a newly-identified player. Called
// once a client's Hello (with its UUID) has been received -- not on raw
// connect, since an un-identified peer doesn't have a session yet.
pub fn generate_session_id(server_state: &ServerState) -> u32 {
    loop {
        let id = rand::random::<u32>();
        if id != 0 && !server_state.players_data.contains_key(&id) {
            return id;
        }
    }
}

#[derive(Default)]
pub struct ServerState {
    pub players_data: HashMap<u32, PlayerData>,

    pub things_to_send: Vec<SendToWhom>,
}

pub enum ReceiveError {
    // Peer sent something before we'd received its Hello / assigned it a
    // session id yet -- dropped, not an error worth surfacing loudly.
    PeerNotIdentified,
    InvalidHeader { id: u32 },
    UnreadableHeader { id: u32 },
    NonUpdateEvent { id: u32 },
    // Peer already has a session (sent Hello before); a second Hello is
    // ignored rather than re-identified.
    AlreadyIdentified { id: u32 },
    PlayerNotFound { id: u32 },
    UnreadableDataReceived { id: u32 },
    AssociatedDataNotFound { id: u32 },
    UnreadableHello,
}

pub enum DisconnectError {
    InvalidId,
    NoDataStored { id: u32 },
}

pub enum EventError {
    ReceiveError(ReceiveError),
    DisconnectError(DisconnectError),
}

pub enum SendError {
    PeerWithoutId,
}
