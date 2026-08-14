use enet::Event;
use zerocopy::FromBytes;

pub mod data_utilities;

pub use data_utilities::*;

// if an event occured, we handle it:
// note: none of the helper functions return errors, because
// we need not handle them; in the context of a game server,
// we can ignore them and hope the next update will succeed.
// we do _not_ want to interrupt the program flow because of an error.
pub fn handle_event(event: &mut enet::Event<u32>, server_state: &mut ServerState) {
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

fn handle_connect_request(peer: &mut enet::Peer<u32>, server_state: &mut ServerState) {
    // tokens are single-use, meaning once a player disconnects,
    // they lose that token. one day, we will use a permanent uuid too.
    // therefore we must generate this token on connect no matter the player.
    let token = generate_token();

    // the token associated with a player is stored in
    // the enet::Peer type, and also as a key in the hashmap that stores
    // players' data.
    peer.set_data(Some(token));
    let new_id = server_state
        .players_data
        .values()
        .fold(0, |max_up_to_here, data| {
            if data.id > max_up_to_here {
                data.id
            } else {
                max_up_to_here
            }
        });
    // id is incremental: 0, 1, 2...
    // now if a player quits, and another joins, there will just be an unassigned id.
    // TODO: better implement this behaviour, because every single time a player connects,
    // we search through, instead we could just have an incremented variable
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
                        .to_packet_bytes(PacketType::ShareToken),
                ))
            }
            _ => server_state.things_to_send.push(SendToWhom::ToOne(
                token,
                client.to_packet_bytes(PacketType::NotifyStatusOnConnect),
            )),
        }
    }
}

fn handle_disconnect(server_state: &mut ServerState, token: &u32) {
    // send everyone a disconnect Packet
    server_state.things_to_send.push(SendToWhom::ToAll(
        server_state.players_data
            .get(token)
            .expect("if the player disconnects, we are certain they connected at some point and therefore we stored their data")
            .to_packet_bytes(PacketType::Leave)
            .clone(),
    ));
    server_state.players_data.remove(token);
}

fn handle_receive(
    server_state: &mut ServerState,
    peer: &mut enet::Peer<u32>,
    packet: &mut enet::Packet,
) {
    // TODO: peers could not have a token if they just connected, but we didn't handle that event yet.
    // should fix that and accept that some peers might not have a token, in which case we ignore them.
    // token is stored in the enet::Peer type itself
    //
    // // TODO: turn the if let pyramid into a let Ok(value) else {return (error string)}; //rest of code
    // then turn the error strings into error tuples, flatten with ? and .ok_or() method
    // and create an error tuple handling function to call from main.rs
    let actual_token = *peer.data().expect("all peers given token on connect");
    if let Ok((header, _training_data)) = PacketHeader::ref_from_prefix(packet.data()) {
        if let Some(packet_type) = PacketType::from_u8(header.packet_type) {
            if packet_type == PacketType::Update {
                if let Ok((packet, _trailing_data)) = UpdatePacket::ref_from_prefix(packet.data()) {
                    let claimed_token = packet.header.id;
                    if claimed_token == actual_token {
                        if let Some(e) = server_state.players_data.get(&actual_token) {
                            if let Some(new_data) = Packet::to_data(Packet::Update(*packet), e.id) {
                                server_state.players_data.insert(packet.header.id, new_data);
                                // send update to all players
                                server_state.things_to_send.push(SendToWhom::ToAll(
                                    server_state
                                        .players_data
                                        .get(&actual_token)
                                        .expect("all peers given token on connect")
                                        .to_packet_bytes(PacketType::Update),
                                ))
                            } else {
                                eprintln!(
                                    "Data sent by peer {:?} with token `{}` could not be read!",
                                    peer, actual_token
                                )
                            }
                        } else {
                            eprintln!(
                                "error: could not get data from Peer with token `{}`",
                                actual_token
                            )
                        };
                    } else {
                        eprintln!(
                            "Peer {:?} with expected connect token `{}` sent mismatching token `{}` instead.",
                            peer, actual_token, claimed_token
                        )
                    }
                } else {
                    // if the player sends invalid packets, we ignore it for now. Might want more complex behaviour later on.
                    eprintln!(
                        "Peer {:?} with connect token `{}` sent invalid packets.",
                        peer, actual_token
                    )
                };
            } else {
                eprintln!(
                    "error: received a packet of non-update type {:#?} when enet gave a Receive event.",
                    packet_type
                )
            }
        } else {
            eprintln!("error: could not read packet type from the packet header.")
        }
    } else {
        // if the player sends invalid packets, we ignore it for now. Might want more complex behaviour later on.
        eprintln!(
            "Peer {:?} with connect token `{}` sent invalid packets.",
            peer, actual_token
        )
    };
}

// because there is a send list with kinds of messages to send, this one sends all,
// from the main thread
pub fn handle_send_list(to_do: SendToWhom, enet: &mut enet::Host<u32>) {
    // two types of messages to send: only to one client, or to all (eg updates)
    match to_do.clone() {
        SendToWhom::ToAll(packet_to_send) => {
            enet.peers().for_each(move |mut peer| {
                match enet::Packet::new(
                    packet_to_send.as_slice(),
                    enet::PacketMode::ReliableSequenced,
                ) {
                    Ok(packet) => match peer.send_packet(packet, 0) {
                        Ok(_) => (),
                        Err(e) => eprintln!("error: could not send packet, got error: {e}"),
                    },
                    Err(e) => eprintln!("Could not convert data to packet, got error: {e}"),
                }
            });
        }
        // in this case, we do not check that this user exists, we assume
        // function that said to send this can be trusted.
        SendToWhom::ToAllButOne(token, packet_to_send) => {
            enet.peers().for_each(move |mut peer| {
                if *peer.data().expect("all peers given token on connect") == token {
                    match enet::Packet::new(
                        packet_to_send.as_slice(),
                        enet::PacketMode::ReliableSequenced,
                    ) {
                        Ok(packet) => match peer.send_packet(packet, 0) {
                            Ok(_) => (),
                            Err(e) => eprintln!("error: could not send packet, got error: {e}"),
                        },
                        Err(e) => eprintln!("Could not convert data to packet, got error: {e}"),
                    }
                }
            });
        }
        SendToWhom::ToOne(token, packet_to_send) => {
            if let Some(peer) = enet
                .peers()
                .find(|peer| *peer.data().expect("all peers given token on connect") == token)
            {
                match enet::Packet::new(
                    packet_to_send.as_slice(),
                    enet::PacketMode::ReliableSequenced,
                ) {
                    Ok(packet) => match peer.clone().send_packet(packet, 0) {
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
