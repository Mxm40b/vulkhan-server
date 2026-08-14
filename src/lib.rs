use enet::Event;
use std::collections::HashMap;
use zerocopy::FromBytes;

pub mod data_utilities;

use data_utilities::*;

// if an event occured, we handle it:
// note: none of the helper functions return errors, because
// we need not handle them; in the context of a game server,
// we can ignore them and hope the next update will succeed.
// we do _not_ want to interrupt the program flow because of an error.
pub fn handle_event(
    event: &mut enet::Event<u32>,
    players_data: &mut HashMap<u32, PlayerData>,
    things_to_send: &mut Vec<SendToWhom>,
) {
    match event {
        // ...and immediately send everything to the associated helper function
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
    // tokens are single-use, meaning once a player disconnects,
    // they lose that token. one day, we will use a permanent uuid too.
    // therefore we must generate this token on connect no matter the player.
    let token = generate_token();

    // the token associated with a player is stored in
    // the enet::Peer type, and also as a key in the hashmap that stores
    // players' data.
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
    // TODO: better implement this behaviour, because every single time a player connects,
    // we search through, instead we could just have an incremented variable
    let temp_data = PlayerData::new(new_id);
    // Pierre, here you notice that the owner of that data is now the hashmap.
    // if you try to reuse temp_data later, rust will complain.
    // see chap 4 of the rust book.
    players_data.insert(token, temp_data);
    let new_player = &players_data
        .get(&token)
        .expect("this player exists; they were just created");
    // two messages: one will send to all, the connected client too,
    // their position data, and one will send only to that client,
    // their token so that they can use it for later messages.
    // the client can differentiate between a ShareToken message
    // and a Join message. one communicates position and token,
    // the other communicates position and id.
    things_to_send.push(SendToWhom::ToAll(
        new_player.to_packet_bytes(PacketType::Join).clone(),
    ));
    things_to_send.push(SendToWhom::ToOne(
        token,
        players_data
            .get(&token)
            .expect("we are certain this player has a token since they were given one upon connect")
            // we change the id to the token and reuse the same type of packet
            // because we are lazy (duplication of position data)
            // and this is only sent once per connection
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
            .expect("if the player disconnects, we are certain they connected at some point and therefore we stored their data")
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
    // token is stored in the enet::Peer type itself
    let actual_token = *peer
        .data()
        .expect("All peers that have connected (this function is called in case of an enet receive event so they have) have had tokens assigned.");
    if let Ok((packet, _trailing_data)) = Packet::ref_from_prefix(packet.data()) {
        let claimed_token = packet.id;
        if claimed_token == actual_token {
            if let Some(new_data) =
                Packet::to_data(*packet, players_data.get(&actual_token).unwrap().id)
            {
                players_data.insert(packet.id, new_data);
                // send update to all players
                things_to_send.push(SendToWhom::ToAll(
                    players_data
                        .get(&actual_token)
                        .expect("this peer has data assigned to their token, because they have connected at some point, and were assigned data on connect.")
                        .to_packet_bytes(PacketType::Update),
                ))
            } else {
                eprintln!(
                    "Data sent by peer {:?} with token `{}` could not be read!",
                    peer, actual_token
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
        SendToWhom::ToOne(token, packet_to_send) => {
            if let Some(peer) = enet.peers().find(|peer| {
                *peer
                    .data()
                    .expect("no reason why any peer shouldn't have a token, since on connect they are given one.")
                    == token
            }) {
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
