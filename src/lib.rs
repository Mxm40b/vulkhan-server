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
        // ...and immediately send everything to the associated helper function
        Event::Connect(peer) => handle_connect_request(peer, server_state), // currently the only way to disconnect is if the user has internet connection
        // and chooses to disconnect.
        // todo: if a user timeouts, disconnect them.
        // or does enet do that already? idk
        Event::Disconnect(_peer, token) => handle_disconnect(server_state, token),
        Event::Receive {
            sender: peer,
            packet,
            channel_id: _id,
        } => handle_receive(server_state, peer, packet),
    }
}

fn handle_connect_request(
    peer: &mut enet::Peer<u32>,
    server_state: &mut ServerState,
) -> Result<(), EventError> {
    // tokens are single-use, meaning once a player disconnects,
    // they lose that token. one day, we will use a permanent uuid too.
    // therefore we must generate this token on connect no matter the player.
    let token = generate_token(server_state);

    // the token associated with a player is stored in
    // the enet::Peer type, and also as a key in the hashmap that stores
    // players' data.
    peer.set_data(Some(token));
    let new_id = server_state.highest_player_id + 1;
    server_state.highest_player_id = new_id;
    // id is incremental: 1, 2, 3... note: starts at one because highest_player_id default value is 0.
    // now if a player quits, and another joins, there will just be an unassigned id.
    let temp_data = PlayerData::new(new_id);
    // Pierre, here you notice that the owner of that data is now the hashmap.
    // if you try to reuse temp_data later, rust will complain.
    // see chap 4 of the rust book.
    server_state.players_data.insert(token, temp_data);
    let new_player = &server_state
        .players_data
        .get(&token)
        .expect("this player exists; they were just created");
    // two messages: one will send to all but the client,
    // the position data, and one will send only to that client,
    // their token so that they can use it for later messages.
    // the client can differentiate between a ShareToken message
    // and a Join message. one communicates position and token,
    // the other communicates position and id.
    server_state.things_to_send.push(SendToWhom::ToAllButOne(
        token,
        new_player.to_packet_bytes(PacketType::Join).clone(),
        enet::PacketMode::ReliableSequenced,
    ));
    // this sends the new player all the old players' data
    for client in server_state.players_data.values() {
        match client.id {
            id if id == new_id => {
                server_state.things_to_send.push(SendToWhom::ToOne(
                    token,
                    server_state.players_data
                        .get(&token)
                        .expect("we are certain this player has a token since they were given one upon connect")
                        // we change the id to the token and reuse the same type of packet
                        // because we are lazy
                        // and this is only sent once per connection,
                        // and no useless data is shared anyways
                        .with_id(token)
                        .to_packet_bytes(PacketType::ShareToken),enet::PacketMode::ReliableSequenced
                ))
            }
            _ => server_state.things_to_send.push(SendToWhom::ToOne(
                token,
                client.to_packet_bytes(PacketType::NotifyStatusOnConnect),
                enet::PacketMode::ReliableSequenced,
            )),
        }
    }
    Ok(())
}

fn handle_disconnect(server_state: &mut ServerState, token: &u32) -> Result<(), EventError> {
    // send everyone a disconnect Packet
    if *token == 0 {
        // ENet clears a peer's user data to 0 on a forced/abrupt reset
        // (as opposed to a graceful disconnect), so a Disconnect event
        // can arrive with no real token attached. 0 was never handed out
        // by generate_token(), so treat it as "nothing to clean up".
        //
        // is there no data associated to that player, still stored in
        // players_data ? if so, should we ignore the memory leak?
        // idea: run a repair function for every (number of players) iterations of the loop,
        // that checks the number of players for whom we have data, the number of peers,
        // and if they mismatch, hunt for the extra data or the peer that didn't fully connect/disconnect
        // or other approach: match every peer for every piece of data stored in the hashmap,
        // and disconnect/delete all that don't have a match.
        // but need to think about possible attacks.
        return Err(EventError::DisconnectError(DisconnectError::InvalidToken));
    }

    let player = server_state
        .players_data
        .get(token)
        .ok_or(EventError::DisconnectError(DisconnectError::NoDataStored {
            token: *token,
        }))?;
    // {
    //     eprintln!(
    //         "warning: player with token `{token}` disconnected but no player data was stored for it, ignoring"
    //     );
    //     return Err(EventError::DisconnectError(DisconnectError::NoDataStored));
    // };
    // send everyone a disconnect Packet
    server_state.things_to_send.push(SendToWhom::ToAll(
        player.to_packet_bytes(PacketType::Leave).clone(),
        enet::PacketMode::ReliableSequenced,
    ));
    server_state.players_data.remove(token);
    Ok(())
}

fn handle_receive(
    server_state: &mut ServerState,
    peer: &mut enet::Peer<u32>,
    packet: &mut enet::Packet,
) -> Result<(), EventError> {
    // TODO: peers could not have a token if they just connected, but we didn't handle that event yet.
    // should fix that and accept that some peers might not have a token, in which case we ignore them.
    // token is stored in the enet::Peer type itself
    //
    // // TODO: turn the if let pyramid into a let Ok(value) else {return (error string)}; //rest of code
    // then turn the error strings into error tuples, flatten with ? and .ok_or() method
    // and create an error tuple handling function to call from main.rs
    let actual_token = *peer
        .data()
        .ok_or(EventError::ReceiveError(ReceiveError::PeerWithoutToken))?;
    let Ok((header, _training_data)) = PacketHeader::ref_from_prefix(packet.data()) else {
        // if the player sends invalid packets, we ignore it for now. Might want more complex behaviour later on.
        return Err(EventError::ReceiveError(ReceiveError::InvalidHeader {
            token: actual_token,
        }));
    };

    let packet_type = PacketType::from_u8(header.packet_type).ok_or(EventError::ReceiveError(
        ReceiveError::UnreadableHeader {
            token: actual_token,
        },
    ))?;

    match packet_type {
        PacketType::Join
        | PacketType::Leave
        | PacketType::NotifyStatusOnConnect
        | PacketType::ShareToken => {
            return Err(EventError::ReceiveError(ReceiveError::NonUpdateEvent {
                token: actual_token,
            }));
        }
        PacketType::Update => {
            let (packet, _trailing_data) = UpdatePacket::ref_from_prefix(packet.data()).map_err(
                // if the player sends invalid packets, we ignore it for now. Might want more complex behaviour later on.
                // eprintln!(
                //     "Peer {:?} with connect token `{}` sent invalid packets.",
                //     peer, actual_token
                // );
                |_| {
                    EventError::ReceiveError(ReceiveError::UnreadableDataReceived {
                        token: actual_token,
                    })
                },
            )?;

            let claimed_token = packet.header.id;
            if claimed_token != actual_token {
                return Err(EventError::ReceiveError(ReceiveError::TokenMismatch {
                    expected: actual_token,
                    got: claimed_token,
                }));
            };

            let old_data =
                server_state
                    .players_data
                    .get(&actual_token)
                    .ok_or(EventError::ReceiveError(
                        ReceiveError::AssociatedDataNotFound {
                            token: actual_token,
                        },
                    ))?;

            let new_data = Packet::to_data(Packet::Update(*packet), old_data.id).ok_or(
                EventError::ReceiveError(ReceiveError::UnreadableDataReceived {
                    token: actual_token,
                }),
            )?;

            server_state.players_data.insert(packet.header.id, new_data);
            // send update to all players
            // TODO: turn the following expect into an error to return, to not crash the server
            server_state.things_to_send.push(SendToWhom::ToAllButOne(
                actual_token,
                server_state
                    .players_data
                    .get(&actual_token)
                    .expect("all peers given token and data on connect")
                    .to_packet_bytes(PacketType::Update),
                enet::PacketMode::UnreliableSequenced,
            ))
        }
    }
    Ok(())
}

// because there is a send list with kinds of messages to send, this one sends all,
// from the main thread
pub fn handle_send_list(to_do: SendToWhom, enet: &mut enet::Host<u32>) {
    // two types of messages to send: only to one client, or to all (eg updates)
    match to_do {
        SendToWhom::ToAll(packet_to_send, packet_mode) => {
            enet.peers().for_each(move |mut peer| {
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
        SendToWhom::ToAllButOne(token, packet_to_send, packet_mode) => {
            enet.peers().for_each(move |mut peer| {
                if *peer.data().expect("all peers given token on connect") != token {
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
        SendToWhom::ToOne(token, packet_to_send, packet_mode) => {
            if let Some(peer) = enet
                .peers()
                .find(|peer| *peer.data().expect("all peers given token on connect") == token)
            {
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
                    "error: tried to send data to peer with token {} but no such peer was found.",
                    token
                )
            }
        }
    }
}

pub fn handle_event_error(e: EventError) {
    match e {
        EventError::ConnectError(e) => match e {
            // no connect errors for now
        },
        EventError::DisconnectError(e) => match e {
            DisconnectError::InvalidToken => {
                eprintln!(
                    "error: a disconnecting ting peer has an invalid token stored of value 0 its an error P talked about, i don't understand it."
                )
            }
            DisconnectError::NoDataStored { token } => {
                eprintln!(
                    "error: peer with token `{}` had no associated data stored.",
                    token
                )
            }
        },
        EventError::ReceiveError(e) => match e {
            ReceiveError::InvalidHeader { token } => {
                eprintln!(
                    "error: peer with token `{}` sent a header of invalid length.",
                    token
                );
            }
            ReceiveError::UnreadableHeader { token } => {
                eprintln!(
                    "error: could not read packet type from the packet header sent by player with token `{}`",
                    token
                );
            }
            ReceiveError::NonUpdateEvent { token } => {
                eprintln!(
                    "error: received a packet of non-update type from player with token `{} when enet gave a Receive event; packet types other than update not handled for now.",
                    token
                );
            }
            ReceiveError::TokenMismatch { expected, got } => {
                eprintln!(
                    "warning: Peer with expected connect token `{}` sent mismatching token `{}` instead.",
                    expected, got
                );
            }
            ReceiveError::PlayerNotFound { token } => {
                eprintln!("error: could not get data from Peer with token `{}`", token);
            }
            ReceiveError::UnreadableDataReceived { token } => {
                eprintln!(
                    "error: data sent by peer with token `{}` could not be read!",
                    token
                );
            }
            ReceiveError::AssociatedDataNotFound { token } => {
                eprintln!(
                    "error: could not get data from player with token `{}`; need to investigate this.",
                    token
                )
            }
            ReceiveError::PeerWithoutToken => {
                eprintln!(
                    "error: all enet::Peer should have a token, they were given one on connect. And yet..."
                )
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
