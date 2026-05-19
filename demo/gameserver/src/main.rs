mod game;
mod quic;
mod quic_stream;

use bincode::config::Configuration;
use game::{GameWorld, NetworkMessage, ROUTE_LENGTH_M, TICK_HZ};
use std::io::ErrorKind;

use wit_bindgen::generate;
use wstd::io::{self, AsyncWrite};
use wstd::iter::AsyncIterator;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::rvm::lambda::host::{self, log};
use crate::rvm::lambda::quic::ErrorCode;

generate!({
    world: "rvm-quic",
    path: "../../wit",
    with: {
        "wasi:io/poll@0.2.8":  wasip2::io::poll,
        "wasi:io/error@0.2.8":  wasip2::io::error,
        "wasi:io/streams@0.2.8":  wasip2::io::streams,
        "wasi:clocks/monotonic-clock@0.2.8":  wasip2::clocks::monotonic_clock,
        "wasi:sockets/network@0.2.8":  wasip2::sockets::network,
    },
    async: true
});

fn to_io_err(err: ErrorCode) -> io::Error {
    match err {
        ErrorCode::Unknown => ErrorKind::Other.into(),
        ErrorCode::AccessDenied => ErrorKind::PermissionDenied.into(),
        ErrorCode::NotSupported => ErrorKind::Unsupported.into(),
        ErrorCode::InvalidArgument => ErrorKind::InvalidInput.into(),
        ErrorCode::OutOfMemory => ErrorKind::OutOfMemory.into(),
        ErrorCode::Timeout => ErrorKind::TimedOut.into(),
        ErrorCode::WouldBlock => ErrorKind::WouldBlock.into(),
        ErrorCode::InvalidState => ErrorKind::InvalidData.into(),
        ErrorCode::AddressInUse => ErrorKind::AddrInUse.into(),
        ErrorCode::ConnectionRefused => ErrorKind::ConnectionRefused.into(),
        ErrorCode::ConnectionReset => ErrorKind::ConnectionReset.into(),
        ErrorCode::ConnectionAborted => ErrorKind::ConnectionAborted.into(),
        ErrorCode::ConcurrencyConflict => ErrorKind::AlreadyExists.into(),
        _ => ErrorKind::Other.into(),
    }
}

type ClientId = u64;
type ClientOutboxes = Arc<Mutex<HashMap<ClientId, Arc<Mutex<Vec<Vec<u8>>>>>>>;

#[wstd::main]
async fn main() -> io::Result<()> {
    // Initialize the ECS world
    let world = Arc::new(Mutex::new(GameWorld::new()));
    let clients: ClientOutboxes = Arc::new(Mutex::new(HashMap::new()));

    // Spawn game loop task
    let world_clone = world.clone();
    let clients_clone = clients.clone();
    wstd::runtime::spawn(async move {
        game_loop(world_clone, clients_clone).await;
    })
    .detach();

    let listener = crate::quic::QuicListener::bind("127.0.0.1:8086").await?;
    let mut incoming = listener.incoming();
    let mut next_client_id: ClientId = 0;

    while let Some(incoming) = incoming.next().await {
        match incoming {
            Ok(connection) => {
                crate::rvm::lambda::host::log("got incoming connection".into()).await;
                let client_id = next_client_id;
                next_client_id += 1;

                let world_clone = world.clone();
                let clients_clone = clients.clone();

                // Spawn handler for this client
                crate::rvm::lambda::host::log("spawning task".into()).await;
                // wstd::runtime::spawn(async move {
                crate::rvm::lambda::host::log("running task".into()).await;
                if let Err(e) =
                    handle_client(client_id, connection, world_clone, clients_clone).await
                {
                    crate::rvm::lambda::host::log(format!("Client {} error: {}", client_id, e))
                        .await;
                    eprintln!("Client {} error: {}", client_id, e);
                }
                // })
                // .detach();
            }
            Err(err) => {
                host::log(format!("ERR {err}")).await;
            }
        }
    }

    Ok(())
}

async fn handle_client(
    client_id: ClientId,
    connection: crate::quic::QuicConnection,
    world: Arc<Mutex<GameWorld>>,
    clients: ClientOutboxes,
) -> io::Result<()> {
    log("[GUEST] starting client handler".to_owned()).await;
    if let Some(max_size) = connection.max_datagram_size().await {
        log(format!("[GUEST] peer datagram max size: {max_size}")).await;
    }

    let msg = connection.accept_bi().await?;
    let (_input, mut output) = msg.split();

    // Register client
    let outbox = Arc::new(Mutex::new(Vec::new()));
    clients.lock().unwrap().insert(client_id, outbox.clone());

    // Spawn player in world
    {
        let mut world = world.lock().unwrap();
        world.spawn_player(client_id);
    }

    // Send initial state
    let state_msg = NetworkMessage::Welcome {
        client_id,
        tick_hz: TICK_HZ,
        route_length_m: ROUTE_LENGTH_M,
        datagrams_enabled: connection.max_datagram_size().await.is_some(),
    };
    output
        .write(
            &bincode::encode_to_vec::<_, Configuration>(&state_msg, Configuration::default())
                .unwrap(),
        )
        .await?;
    output.flush().await?;

    loop {
        let data = match connection.receive_datagram().await {
            Ok(data) => data,
            Err(err) => {
                log(format!("[GUEST] client datagram loop closed: {err}")).await;
                break;
            }
        };
        log("[GUEST] got input datagram from client".to_owned()).await;

        if let Ok((msg, _)) = bincode::decode_from_slice::<NetworkMessage, Configuration>(
            &data,
            Configuration::default(),
        ) {
            match msg {
                NetworkMessage::Input { power } => {
                    log("[GUEST] got player input".to_owned()).await;
                    let mut world = world.lock().unwrap();
                    world.update_player_input(client_id, power);
                }
                _ => {}
            }
        }

        // Send queued messages
        let messages = {
            let mut outbox = outbox.lock().unwrap();
            std::mem::take(&mut *outbox)
        };

        for message in messages {
            log("[GUEST] sending world state datagram".to_owned()).await;
            connection.send_datagram(&message).await?;
        }
    }

    // Cleanup
    clients.lock().unwrap().remove(&client_id);
    world.lock().unwrap().despawn_player(client_id);

    Ok(())
}

async fn game_loop(world: Arc<Mutex<GameWorld>>, clients: ClientOutboxes) {
    let mut interval = wstd::time::interval(wstd::time::Duration::from_millis(50)); // 20 ticks/sec

    loop {
        interval.next().await;

        // Update game state
        let state_msg = {
            let mut world = world.lock().unwrap();
            world.update(0.05); // 50ms = 0.05s
            world.get_state()
        };

        // Broadcast to all clients
        let data = bincode::encode_to_vec::<_, Configuration>(&state_msg, Configuration::default())
            .unwrap();

        let clients_lock = clients.lock().unwrap();
        for outbox in clients_lock.values() {
            let mut outbox = outbox.lock().unwrap();
            outbox.push(data.clone());
        }
    }
}
