use enet::Event;
use enet::{Address, Enet};
use std::net;
use std::{collections::HashMap, error::Error};
use vulkhan_server;

const MAX_PLAYERS: usize = 32;

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

    let mut players_data: HashMap<u32, PlayerData> = HashMap::new();

    let mut things_to_send: Vec<SendToWhom> = Vec::new();

    loop {
        // in loop, in each iteration create new context for reading Events.
        // this fixes the fact that enet is borrowed in that scope when service()
        // is called because enet is weird.
        //
        // this loop does two things:
        // first, it reads a single Event, with a timeout of 1000ms, if no event occurs,
        // it returns an Err and we ignore it, otherwise we handle the event.
        // second, outside of the first scope, it looks at what it has to send,
        // and sends each thing from the main thread, either to all players, or to a single one.
        //
        // this let statement is the scope in question:
        let () = {
            let attempt = enet.service(1000); // this lineeeeee... UUUUUUGH
            await_event(attempt, &mut players_data, &mut things_to_send);
        };
        // end of that scope to get rid of enet borrowing error
        for to_do in things_to_send.clone() {
            vulkhan_server::handle_send_list(to_do, &mut enet);
        }
    }
}

pub use vulkhan_server::data_utilities::{PlayerData, SendToWhom};

fn await_event(
    attempt: Result<Option<Event<'_, u32>>, enet::Error>,
    players_data: &mut HashMap<u32, PlayerData>,
    things_to_send: &mut Vec<SendToWhom>,
) {
    if let Ok(event) = attempt {
        match event {
            // if nothing happened we do nothing:
            None => (),
            // otherwise we handle the event:
            Some(event) => {
                let mut event = event; // to borrow it as mutable, not sure how but it works
                vulkhan_server::handle_event(&mut event, players_data, things_to_send);
            }
        }
    } else {
        panic!("{attempt:?}")
    };
}
