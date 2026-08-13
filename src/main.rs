use enet::Event;
use enet::{Address, Enet};
use std::net;
use std::{collections::HashMap, error::Error};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

const MAX_PLAYERS: usize = 32;

// #[repr(u8)]
// #[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, KnownLayout, Immutable)]
#[derive(PartialEq)]
enum PacketType {
    Join,
    Leave,
    Update,
}
//
// instead of this, packet will be stored as u32 then converted via a function.

#[derive(Clone)]
enum SendToWhom<'a> {
    ToAll(Vec<u8>),
    #[allow(dead_code)]
    // because one day the server will send packages to individual users not just on updates, i think
    ToOne((&'a enet::Peer<'a, u32>, Vec<u8>)),
}

impl PacketType {
    fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(PacketType::Join),
            1 => Some(PacketType::Leave),
            2 => Some(PacketType::Update),
            _ => None,
        }
    }
    fn to_u8(&self) -> u8 {
        match self {
            Self::Join => 0,
            Self::Leave => 1,
            Self::Update => 2,
        }
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, KnownLayout, Immutable)]
struct Packet {
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

struct PlayerData {
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
    fn to_packet_bytes(&self) -> Vec<u8> {
        Packet {
            packet_type: PacketType::Join.to_u8(),
            id: self.id,
            position: self._position.to_array(),
            orientation: self._orientation.to_array(),
        }
        .as_bytes()
        .to_vec()
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

fn main() -> Result<(), Box<dyn Error>> {
    let enet = Enet::new()?;

    let address = net::Ipv4Addr::new(0, 0, 0, 0);
    let port = 1234;
    // host is of type u32 because players also are.
    // this is their token, and they might send packages with
    // an invalid token, in which case the packet will be rejected.
    let mut enet = enet.create_host::<u32>(
        Some(&Address::new(address, port)),
        MAX_PLAYERS,
        enet::ChannelLimit::Limited(2),
        enet::BandwidthLimit::Unlimited,
        enet::BandwidthLimit::Unlimited,
    )?;
    println!("Server started at address: {address}:{port}");

    let mut player_data: HashMap<u32, PlayerData> = HashMap::new();

    let mut things_to_send: Vec<SendToWhom> = Vec::new();

    loop {
        // in loop, in each iteration create new context.
        // maybe this will fix the fact that enet was borrowed as a mutable reference last cycle.
        {
            let attempt = enet.service(1000);
            if let Ok(event) = attempt {
                match event {
                    None => continue,
                    Some(event) => {
                        let mut event = event;
                        match event {
                            Event::Connect(ref mut peer) => {
                                let token = generate_token();

                                peer.set_data(Some(token));
                                player_data.insert(
                                    token,
                                    PlayerData::new(player_data.keys().count() as u32),
                                ); // id is incremental: 0, 1, 2...
                                // TODO: fix bug: if player disconnects, the id's clash.
                                let new_player = &player_data
                                    .get(&token)
                                    .expect("this player exists; they were just created");
                                // TODO now: send everyone the data of the user.
                                things_to_send
                                    .push(SendToWhom::ToAll(new_player.to_packet_bytes().clone()));
                            } // currently the only way to disconnect is if the user has internet connection
                            // and chooses to disconnect.
                            // todo: if a user timeouts, disconnect them.
                            // or does enet do that already? idk
                            Event::Disconnect(ref _peer, token) => {
                                player_data.remove(&token);
                            }
                            Event::Receive {
                                sender: ref mut peer,
                                ref packet,
                                channel_id: _id,
                            } => {
                                let (packet, _trailing_data) = Packet::ref_from_prefix(
                                    packet.data(),
                                )
                                .expect(
                                    "for now the server panics when a player sends invalid data",
                                );
                                let claimed_token = *peer
                                    .data()
                                    .expect("shouldn't all peers have data once they connect?");
                                if packet.id == claimed_token {
                                    let new_data = Packet::to_data(
                                        *packet,
                                        player_data.get(&claimed_token).unwrap().id,
                                    )
                                    .expect(
                                        "for now i just really hope that clients send valid data",
                                    );
                                    player_data.insert(packet.id, new_data);
                                }
                            }
                        }
                    }
                }
            } else {
                panic!("{attempt:?}")
            };
        };
        for to_do in things_to_send.clone() {
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
                SendToWhom::ToOne((peer, packet_to_send)) => peer
                    .clone()
                    .send_packet(
                        enet::Packet::new(
                            packet_to_send.as_slice(),
                            enet::PacketMode::ReliableSequenced,
                        )
                        .unwrap(),
                        0,
                    )
                    .expect("not handling this error for now"),
            }
        }
    }
}

// planning for each player to have a connect token u64 created by serv, sent over for each update packet, and unencrypted;
// a permanent u64 id per server created on first connect, sent in encrypted form (one day)
//
// the players will be stored in a fixed-length list of size _max player count_, and positions assigned according to peerId given by enet
// then, each player sending a packet will send their session token and their id (enet handles this), and the server will check that token
// against the one stored at that address in the list.
