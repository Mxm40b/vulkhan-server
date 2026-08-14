use enet::Event;
use std::collections::HashMap;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

// #[repr(u8)]
// #[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, KnownLayout, Immutable)]
#[derive(PartialEq)]
pub enum PacketType {
    Join,
    Leave,
    Update,
    ShareToken,
}
//
// instead of this, packet will be stored as u32 then converted via a function.

#[derive(Clone)]
pub enum SendToWhom {
    ToAll(Vec<u8>),
    #[allow(dead_code)]
    // because one day the server will send packages to individual users not just on updates, i think
    ToOne(u32, Vec<u8>),
}

impl PacketType {
    fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(PacketType::Join),
            1 => Some(PacketType::Leave),
            2 => Some(PacketType::Update),
            3 => Some(PacketType::ShareToken),
            _ => None,
        }
    }
    fn to_u8(&self) -> u8 {
        match self {
            Self::Join => 0,
            Self::Leave => 1,
            Self::Update => 2,
            Self::ShareToken => 3,
        }
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, KnownLayout, Immutable)]
pub struct Packet {
    packet_type: u8,
    id: u32, // one way, is a token, the other, is a user id
    // position: glam::Vec3,
    // orientation: glam::Quat,
    position: [f32; 3],
    orientation: [f32; 4],
}

impl Packet {
    fn to_data(self, id: u32) -> Option<PlayerData> {
        if PacketType::from_u8(self.packet_type)
            .expect("server does not handle unvalid packet type for now")
            == PacketType::Update
        {
            return Some(PlayerData {
                id,
                _orientation: glam::Quat::from_xyzw(
                    self.orientation[0],
                    self.orientation[1],
                    self.orientation[2],
                    self.orientation[3],
                ),
                _position: glam::Vec3::new(self.position[0], self.position[1], self.position[2]),
            });
        };
        None
    }
}

pub struct PlayerData {
    // token: u32,
    _position: glam::Vec3,
    _orientation: glam::Quat,
    id: u32,
}

// change this so that the peer contains a hash,
// and the hashmap contains Player data without any lifetime guarantees.. This will eliminate a few issues i think.
impl PlayerData {
    // starting position
    fn new(id: u32) -> PlayerData {
        PlayerData {
            // token,
            id,
            _position: glam::Vec3::new(0f32, 0f32, 0f32),
            _orientation: glam::quat(0f32, 0f32, 0f32, 0f32),
        }
    }
    fn to_packet_bytes(&self, packet_type: PacketType) -> Vec<u8> {
        Packet {
            packet_type: packet_type.to_u8(),
            id: self.id,
            position: self._position.to_array(),
            orientation: self._orientation.to_array(),
        }
        .as_bytes()
        .to_vec()
    }
    fn with_id(&self, id: u32) -> Self {
        PlayerData { id, ..*self }
    }
}

fn generate_token() -> u32 {
    // todo: use better random algorithm for generating tokens than this
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("why wouldn't it calculate the duration since epoch??")
        .as_secs() as u32
    // using as u32 removes the upper bits
}

pub fn handle_send_list(to_do: SendToWhom, enet: &mut enet::Host<u32>) {
    match to_do.clone() {
        SendToWhom::ToAll(packet_to_send) => {
            enet.peers().for_each(move |mut peer| {
                peer.send_packet(
                    enet::Packet::new(
                        packet_to_send.as_slice(),
                        enet::PacketMode::ReliableSequenced,
                    )
                    .expect("oh shut up"),
                    0,
                )
                .expect("let's assume the packet is sent properly for now");
            });
        }
        SendToWhom::ToOne(token, packet_to_send) => {
            let peer = enet.peers().find(|peer| {
                *peer
                    .data()
                    .expect("no reason why any peer shouldn't have a token")
                    == token
            });
            peer.clone()
                .expect("oh come on")
                .send_packet(
                    enet::Packet::new(
                        packet_to_send.as_slice(),
                        enet::PacketMode::ReliableSequenced,
                    )
                    .unwrap(),
                    0,
                )
                .expect("not handling this error for now")
        }
    }
}

pub fn handle_event(
    event: &mut enet::Event<u32>,
    players_data: &mut HashMap<u32, PlayerData>,
    things_to_send: &mut Vec<SendToWhom>,
) {
    match event {
        Event::Connect(peer) => handle_connect_request(peer, players_data, things_to_send), // currently the only way to disconnect is if the user has internet connection
        // and chooses to disconnect.
        // todo: if a user timeouts, disconnect them.
        // or does enet do that already? idk
        Event::Disconnect(_peer, token) => handle_disconnect(things_to_send, players_data, token),
        Event::Receive {
            sender: peer,
            packet,
            channel_id: _id,
        } => handle_receive(players_data, things_to_send, peer, packet),
    }
}

fn handle_connect_request(
    peer: &mut enet::Peer<u32>,
    players_data: &mut HashMap<u32, PlayerData>,
    things_to_send: &mut Vec<SendToWhom>,
) {
    let token = generate_token();

    peer.set_data(Some(token));
    let new_id = players_data.values().fold(0, |max_up_to_here, data| {
        if data.id > max_up_to_here {
            data.id
        } else {
            max_up_to_here
        }
    });
    // id is incremental: 0, 1, 2...
    // now if a player quits, and another joins, there will just be an unassigned id.
    let temp_data = PlayerData::new(new_id);
    players_data.insert(token, temp_data);
    let new_player = &players_data
        .get(&token)
        .expect("this player exists; they were just created");
    // TODO now: send everyone the data of the user.
    things_to_send.push(SendToWhom::ToAll(
        new_player.to_packet_bytes(PacketType::Join).clone(),
    ));
    things_to_send.push(SendToWhom::ToOne(
        token,
        players_data
            .get(&token)
            .expect("shut up")
            .with_id(token)
            .to_packet_bytes(PacketType::ShareToken),
    ));
}

fn handle_disconnect(
    things_to_send: &mut Vec<SendToWhom>,
    players_data: &mut HashMap<u32, PlayerData>,
    token: &u32,
) {
    // send everyone a disconnect Packet
    things_to_send.push(SendToWhom::ToAll(
        players_data
            .get(token)
            .expect("uuuugh")
            .to_packet_bytes(PacketType::Leave)
            .clone(),
    ));
    players_data.remove(token);
}

fn handle_receive(
    players_data: &mut HashMap<u32, PlayerData>,
    things_to_send: &mut Vec<SendToWhom>,
    peer: &mut enet::Peer<u32>,
    packet: &mut enet::Packet,
) {
    let (packet, _trailing_data) = Packet::ref_from_prefix(packet.data())
        .expect("for now the server panics when a player sends invalid data");
    let claimed_token = *peer
        .data()
        .expect("shouldn't all peers have data once they connect?");
    if packet.id == claimed_token {
        let new_data = Packet::to_data(*packet, players_data.get(&claimed_token).unwrap().id)
            .expect("for now i just really hope that clients send valid data");
        players_data.insert(packet.id, new_data);
        things_to_send.push(SendToWhom::ToAll(
            players_data
                .get(&claimed_token)
                .expect("aaaaaaaa")
                .to_packet_bytes(PacketType::Update),
        ));
    }
}
