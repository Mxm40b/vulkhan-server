use enet::Event;
use zerocopy::FromBytes;

pub mod data_utilities;

pub use data_utilities::*;

// if an event occured, we handle it:
// note: none of the helper functions return errors, because
// we need not handle them; in the context of a game server,
// we can ignore them and hope the next update will succeed.
// we do _not_ want to interrupt the program flow because of an error.
pub fn handle_event(
    event: &mut enet::Event<u32>,
    server_state: &mut ServerState,
) -> Result<(), EventError> {
    match event {
        Event::Connect(peer) => handle_connect_request(peer, server_state),
        Event::Disconnect(peer, _reason) => handle_disconnect(server_state, peer),
        Event::Receive {
            sender: peer,
            packet,
            channel_id: _id,
        } => handle_receive(server_state, peer, packet),
    }
}

// A raw ENet connection doesn't get a session id or player data yet -- the
// peer's user data (`peer.data()`) stays `None` until they send a Hello
// packet with their UUID (see handle_receive). Any packet other than Hello
// from an un-identified peer is dropped there.
fn handle_connect_request(
    _peer: &mut enet::Peer<u32>,
    _server_state: &mut ServerState,
) -> Result<(), EventError> {
    // we do this when we receive a ConnectPacket instead:
    // let id = generate_session_id(server_state);
    // peer.set_data(Some(id));
    // server_state.players_data.insert(id, PlayerData::new(None));
    Ok(())
}

fn handle_disconnect(
    server_state: &mut ServerState,
    peer: &mut enet::Peer<u32>,
) -> Result<(), EventError> {
    // Note: this reads the peer's own persistent session id via
    // `peer.data()`. It is *not* the same as the `u32` the Event::Disconnect
    // variant also carries -- that second field is the disconnect *reason*
    // code passed to enet_peer_disconnect(peer, reason) by whichever side
    // requested the disconnect, unrelated to who the peer is. The client
    // always passes 0 for that reason, so reading it as if it were the
    // session id meant this function always saw `id == 0` and never
    // actually cleaned up a disconnecting player.
    let Some(&id) = peer.data() else {
        // Never sent a Hello (or was force-reset before/without ever being
        // identified) -- nothing to clean up.
        return Err(EventError::DisconnectError(DisconnectError::InvalidId));
    };

    let player = server_state
        .players_data
        .get(&id)
        .ok_or(EventError::DisconnectError(DisconnectError::NoDataStored {
            id,
        }))?;

    server_state.things_to_send.push(SendToWhom::ToAll(
        player.to_packet_bytes(PacketType::Leave, id),
        enet::PacketMode::ReliableSequenced,
    ));
    server_state.players_data.remove(&id);
    Ok(())
}

fn handle_receive(
    server_state: &mut ServerState,
    peer: &mut enet::Peer<u32>,
    packet: &mut enet::Packet,
) -> Result<(), EventError> {
    let peer_id: Option<u32> = peer.data().copied();

    let Ok((header, _trailing_data)) = PacketHeader::ref_from_prefix(packet.data()) else {
        return Err(EventError::ReceiveError(ReceiveError::InvalidHeader {
            id: peer_id,
        }));
    };

    let packet_type = PacketType::from_u8(header.packet_type).ok_or(EventError::ReceiveError(
        ReceiveError::UnreadableHeader { id: peer_id },
    ))?;

    // Every other packet type requires an already-identified peer; anyone
    // who hasn't sent their Hello yet is silently ignored, per design.

    match packet_type {
        // TODO: handle the Hello type
        PacketType::Hello => {
            let (hello_packet, _trailing_data) = HelloPacket::ref_from_prefix(packet.data())
                .map_err(|_| EventError::ReceiveError(ReceiveError::UnreadableHelloDataReceived))?;
            if let Some(&existing_id) = peer.data() {
                // a player sent two hello's. Pff. Looser.
                return Err(EventError::ReceiveError(ReceiveError::HelloDuplication {
                    id: existing_id,
                }));
            }
            let new_id = generate_session_id(server_state);
            peer.set_data(Some(new_id));

            let new_player_data = PlayerData::new(hello_packet.uuid);
            server_state
                .players_data
                .insert(new_id, new_player_data.clone());

            server_state.things_to_send.push(SendToWhom::ToOne(
                new_id,
                new_player_data.to_packet_bytes(PacketType::Spawn, new_id),
                enet::PacketMode::ReliableSequenced,
            ));

            // No need to notify everyone about the new player, they will receive UpdatePackets and
            // notice themselves that a new Alpha Wolf has joined
        }
        PacketType::Leave | PacketType::ServerUpdate | PacketType::Spawn => {
            return Err(EventError::ReceiveError(ReceiveError::NonUpdateEvent {
                id: peer_id,
            }));
        }
        PacketType::ClientUpdate => {
            // if the peer does not have an associated id, we exit early; they need to send a ConnectPacket first.
            let id = peer_id.ok_or(EventError::ReceiveError(ReceiveError::PeerNotIdentified))?;
            let (packet, _trailing_data) =
                UpdatePacket::ref_from_prefix(packet.data()).map_err(|_| {
                    EventError::ReceiveError(ReceiveError::UnreadableDataReceived { id })
                })?;

            // The client no longer sends a meaningful id on Update packets --
            // we already know who this is from the connection itself.
            //
            // we make sure that the player has associated data:
            server_state
                .players_data
                .get(&id)
                .ok_or(EventError::ReceiveError(
                    ReceiveError::AssociatedDataNotFound { id },
                ))?;

            let new_data = Packet::to_data(Packet::Update(*packet)).ok_or(
                EventError::ReceiveError(ReceiveError::UnreadableDataReceived { id }),
            )?;

            let Some(old_data) = server_state.players_data.get(&id) else {
                return Err(EventError::ReceiveError(
                    ReceiveError::AssociatedDataNotFound { id },
                ));
            };
            let new_data = integrate_position_data_with_player_data(new_data, old_data);

            // new_data.0 is the id, and we already have it
            server_state.players_data.insert(id, new_data);

            server_state.things_to_send.push(SendToWhom::ToAllButOne(
                id,
                server_state
                    .players_data
                    .get(&id)
                    .ok_or(EventError::ReceiveError(ReceiveError::PeerNotIdentified))?
                    .to_packet_bytes(PacketType::ServerUpdate, id),
                enet::PacketMode::UnreliableSequenced,
            ))
        }
    }
    Ok(())
}

fn integrate_position_data_with_player_data(
    position_data: PlayerPositionData,
    old_data: &PlayerData,
) -> PlayerData {
    PlayerData {
        position: position_data.position,
        orientation: position_data.orientation,
        ..*old_data
    }
}

// Handles a client's one-time Hello: assigns them a session id, registers
// their player data, tells everyone else they joined, and dumps the
// existing players' state back to them (reusing the Join wire format --
// the client treats Join and this dump identically).
//
//
// P plz stop slop coding, I am moving code from this function into the match statement.
// fn handle_hello(
//     server_state: &mut ServerState,
//     peer: &mut enet::Peer<u32>,
//     packet: &mut enet::Packet,
// ) -> Result<(), EventError> {
//     if let Some(&existing_id) = peer.data() {
//         // Already identified; a repeated Hello is ignored rather than
//         // re-assigning a new session id out from under them.
//         return Err(EventError::ReceiveError(ReceiveError::HelloDuplication {
//             id: existing_id,
//         }));
//     }

//     let Ok((hello, _trailing_data)) = HelloPacket::ref_from_prefix(packet.data()) else {
//         return Err(EventError::ReceiveError(ReceiveError::UnreadableHello));
//     };
//     // `_hello.uuid` is the client's persistent identity. We don't yet key
//     // anything off it server-side (no reconnect/database support), but it's
//     // parsed and validated here so that's a drop-in addition later.

//     let new_id = generate_session_id(server_state);
//     peer.set_data(Some(new_id));

//     let new_player_data = PlayerData::new(hello.uuid);
//     server_state
//         .players_data
//         .insert(new_id, new_player_data.clone());

//     server_state.things_to_send.push(SendToWhom::ToAllButOne(
//         new_id,
//         new_player_data.to_packet_bytes(PacketType::Join, new_id),
//         enet::PacketMode::ReliableSequenced,
//     ));

//     server_state.things_to_send.push(SendToWhom::ToOne(
//         new_id,
//         new_player_data.to_packet_bytes(PacketType::Spawn, new_id),
//         enet::PacketMode::ReliableSequenced,
//     ));

//     server_state
//         .players_data
//         .iter()
//         // idk why need to deref twice but it works:
//         // this line is to not send to the user that just connected, their own data, we just did that:
//         .filter(|(key, _value)| (**key) != new_id)
//         .map(|(&key, existing_player)| {
//             server_state.things_to_send.push(SendToWhom::ToOne(
//                 new_id,
//                 existing_player.to_packet_bytes(PacketType::Join, key),
//                 enet::PacketMode::ReliableSequenced,
//             ));
//         })
//         // consumes the iterator:
//         .for_each(drop);

//     Ok(())
// }

// because there is a send list with kinds of messages to send, this one sends all,
// from the main thread
pub fn handle_send_list(to_do: SendToWhom, enet: &mut enet::Host<u32>) -> Result<(), SendError> {
    match to_do {
        SendToWhom::ToAll(packet_to_send, packet_mode) => {
            enet.peers().for_each(move |mut peer| {
                if peer.state() != enet::PeerState::Connected {
                    return;
                }
                match enet::Packet::new(packet_to_send.as_slice(), packet_mode) {
                    Ok(packet) => match send_helper(&mut peer, packet, packet_mode) {
                        Ok(_) => (),
                        Err(e) => eprintln!("error: could not send packet, got error: {e}"),
                    },
                    Err(e) => eprintln!("Could not convert data to packet, got error: {e}"),
                }
            });
        }
        // in this case, we do not check that this user exists, we assume
        // function that said to send this can be trusted.
        SendToWhom::ToAllButOne(id, packet_to_send, packet_mode) => {
            enet.peers().for_each(move |mut peer| {
                if peer.state() != enet::PeerState::Connected {
                    return;
                }

                let Some(&receiver_id) = peer.data() else {
                    // un-identified peers haven't sent Hello yet -- they get
                    // nothing until they do.
                    return;
                };
                if receiver_id != id {
                    match enet::Packet::new(packet_to_send.as_slice(), packet_mode) {
                        Ok(packet) => match send_helper(&mut peer, packet, packet_mode) {
                            Ok(_) => (),
                            Err(e) => eprintln!("error: could not send packet, got error: {e}"),
                        },
                        Err(e) => eprintln!("Could not convert data to packet, got error: {e}"),
                    }
                }
            });
        }
        SendToWhom::ToOne(id, packet_to_send, packet_mode) => {
            if let Some(peer) = enet.peers().find(|peer| {
                let Some(&receiver_id) = peer.data() else {
                    return false;
                };
                receiver_id == id
            }) {
                match enet::Packet::new(packet_to_send.as_slice(), packet_mode) {
                    Ok(packet) => match send_helper(&mut peer.clone(), packet, packet_mode) {
                        Ok(_) => (),
                        Err(e) => eprintln!(
                            "error: could not send packet to peer {:?}, instead got: {}",
                            peer, e
                        ),
                    },
                    Err(e) => eprintln!(
                        "error: could not turn data {:?} into enet packet, instead got: {}",
                        packet_to_send, e
                    ),
                }
            } else {
                eprintln!(
                    "error: tried to send data to peer with session id {} but no such peer was found.",
                    id
                )
            }
        }
    };
    Ok(())
}

pub fn handle_event_error(e: EventError) {
    match e {
        EventError::DisconnectError(e) => match e {
            DisconnectError::InvalidId => {
                eprintln!(
                    "note: a disconnecting peer had no session id (never sent Hello, or was force-reset)."
                )
            }
            DisconnectError::NoDataStored { id } => {
                eprintln!(
                    "error: peer with session id `{}` had no associated data stored.",
                    id
                )
            }
        },
        EventError::ReceiveError(e) => match e {
            ReceiveError::PeerNotIdentified => {
                eprintln!("note: dropped a packet from a peer that hasn't sent its Hello yet.")
            }
            ReceiveError::InvalidHeader { id } => match id {
                Some(id) => eprintln!("error: peer `{}` sent a header of invalid length.", id),
                None => eprintln!("error: peer of unknown id sent a header of invalid length."),
            },
            ReceiveError::UnreadableHeader { id } => match id {
                Some(id) => eprintln!(
                    "error: could not read packet type from the packet header sent by peer `{}`",
                    id
                ),
                None => eprintln!(
                    "error: could not read packet type from the packet header sent by peer with unknown id"
                ),
            },
            ReceiveError::NonUpdateEvent { id } => match id {
                Some(id) => eprintln!(
                    "error: received a packet of non-update type from peer `{}`; only Update and Hello are valid from a client.",
                    id
                ),
                None => eprintln!(
                    "error: received a packet of non-update type from peer with unknown id; only Update and Hello are valid from a client."
                ),
            },
            ReceiveError::HelloDuplication { id } => {
                eprintln!(
                    "ERROR: LISTEN EVERYONE! Check this out: peer with id `{}` sent multiple Hello's, such a looser client. \n`Oooo, but i had too much bandwidth` \n\t- the peer. \nlmao.",
                    id
                );
            }
            ReceiveError::PlayerNotFound { id } => {
                eprintln!(
                    "error: could not get data from peer with session id `{}`",
                    id
                );
            }
            ReceiveError::UnreadableDataReceived { id } => {
                eprintln!("error: data sent by peer `{}` could not be read!", id);
            }
            ReceiveError::UnreadableHelloDataReceived => {
                eprintln!(
                    "error: data sent in hello packet (so by an unknown player) could not be read!"
                );
            }
            ReceiveError::AssociatedDataNotFound { id } => {
                eprintln!(
                    "error: could not get data from player with session id `{}`; need to investigate this.",
                    id
                )
            }
            ReceiveError::UnreadableHello => {
                eprintln!("error: received a malformed Hello packet, ignoring it.")
            }
        },
    }
}

pub fn send_helper(
    peer: &mut enet::Peer<u32>,
    packet: enet::Packet,
    mode: enet::PacketMode,
) -> Result<(), enet::Error> {
    match mode {
        // separating reliable sequenced and unreliable sequenced so that connect and disconnect happen quickly,
        // and so that movement packets don't block them
        enet::PacketMode::ReliableSequenced => peer.send_packet(packet, 0),
        enet::PacketMode::UnreliableSequenced => peer.send_packet(packet, 1),
        enet::PacketMode::UnreliableUnsequenced => {
            eprintln!(
                "warning: sending packet as unreliable unsequenced, this should not be possible"
            );
            peer.send_packet(packet, 1)
        }
    }
}
