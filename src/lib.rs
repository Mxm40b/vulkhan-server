use enet::Event;
use std::collections::HashMap;
use zerocopy::FromBytes;

pub mod data_utilities;

use data_utilities::*;

// TODO: fix all .expect()'s for the server to not crash constantly

// if an event occured, we handle it:
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
            .expect("shut up")
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
    // token is stored in the enet::Peer type itself
    let actual_token = *peer
        .data()
        .expect("shouldn't all peers have data once they connect?");
    let claimed_token = packet.id;
    if claimed_token == actual_token {
        let new_data = Packet::to_data(*packet, players_data.get(&actual_token).unwrap().id)
            .expect("for now i just really hope that clients send valid data");
        players_data.insert(packet.id, new_data);
        // send update to all players
        things_to_send.push(SendToWhom::ToAll(
            players_data
                .get(&claimed_token)
                .expect("aaaaaaaa")
                .to_packet_bytes(PacketType::Update),
        ));
    }
}

// because there is a send list with kinds of messages to send, this one sends all,
// from the main thread
pub fn handle_send_list(to_do: SendToWhom, enet: &mut enet::Host<u32>) {
    // two types of messages to send: only to one client, or to all (eg updates)
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
