use std::sync::Arc;

use bevy::prelude::*;
use bincode::{Decode, Encode, config::Configuration};
use quinn::{
    Endpoint,
    crypto::rustls::QuicClientConfig,
    rustls::{self, crypto::CryptoProvider, pki_types::CertificateDer},
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Clone, Copy, Serialize, Deserialize, Encode, Decode)]
pub struct Position {
    pub distance: f32,
}

#[derive(Clone, Copy, Serialize, Deserialize, Encode, Decode)]
pub struct Velocity {
    pub speed: f32,
}

#[derive(Clone, Copy, Serialize, Deserialize, Encode, Decode)]
pub struct Power {
    pub watts: f32,
}

#[derive(Serialize, Deserialize, Clone, Encode, Decode)]
pub struct PlayerState {
    pub client_id: u64,
    pub position: Position,
    pub velocity: Velocity,
    pub power: Power,
}

#[derive(Serialize, Deserialize, Clone, Encode, Decode)]
pub enum NetworkMessage {
    Input { power: f32 },
    WorldState(Vec<PlayerState>),
}

#[derive(Resource)]
pub struct NetworkClient {
    pub tx: UnboundedSender<NetworkMessage>,
    pub rx: UnboundedReceiver<NetworkMessage>,
}

#[derive(Message)]
pub struct WorldStateUpdate(pub Vec<PlayerState>);

pub struct NetworkPlugin;

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        let (tx_to_server, rx_to_server) = unbounded_channel();
        let (tx_from_server, rx_from_server) = unbounded_channel();

        // Spawn tokio runtime for network communication
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = network_task(rx_to_server, tx_from_server).await {
                    eprintln!("Network error: {}", e);
                }
            });
        });

        // std::thread::spawn(move || {
        // let rt = tokio::runtime::Runtime::new().unwrap();
        // rt.block_on(async {
        //     info!("starting network task");
        //     if let Err(e) = mocked_network_task(rx_to_server, tx_from_server).await {
        //             eprintln!("Network error: {}", e);
        //         }
        //     });
        //     info!("network task stopped");
        // });

        app.insert_resource(NetworkClient {
            tx: tx_to_server,
            rx: rx_from_server,
        })
        .add_message::<WorldStateUpdate>()
        .add_systems(Update, receive_network_messages);
    }
}

fn receive_network_messages(
    mut client: ResMut<NetworkClient>,
    mut events: MessageWriter<WorldStateUpdate>,
) {
    while let Ok(msg) = client.rx.try_recv() {
        match msg {
            NetworkMessage::WorldState(states) => {
                events.write(WorldStateUpdate(states));
            }
            _ => {}
        }
    }
}

async fn mocked_network_task(
    mut rx: UnboundedReceiver<NetworkMessage>,
    tx: UnboundedSender<NetworkMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    // if let Err(err) = tx.send(NetworkMessage::WorldState(vec![PlayerState { client_id: 0, position: Position { distance: 0.0 }, velocity: Velocity { speed: 0.0 }, power: Power { watts: 0.0 } }])) {
    //     error!(%err, "failed to send world state update");
    // }

    loop {
        match rx.recv().await {
            Some(NetworkMessage::Input { power }) => {
                if let Err(err) = tx.send(NetworkMessage::WorldState(vec![PlayerState {
                    client_id: 0,
                    position: Position { distance: 0.0 },
                    velocity: Velocity { speed: 0.0 },
                    power: Power { watts: power },
                }])) {
                    error!(%err, "failed to send world state update");
                }
            }
            Some(state @ NetworkMessage::WorldState { .. }) => {
                let _ = tx.send(state);
            }
            _ => (),
        }
    }
}

async fn network_task(
    mut rx: UnboundedReceiver<NetworkMessage>,
    tx: UnboundedSender<NetworkMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(SkipServerVerification::new())
                .with_no_client_auth(),
        )?,
    )));
    let connection = endpoint
        .connect("127.0.0.1:8086".parse()?, "localhost")?
        .await?;
    let datagram_connection = connection.clone();
    let (mut send, mut recv) = connection.open_bi().await?;

    let stream_tx = tx.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match recv.read(&mut buf).await {
                Ok(Some(n)) => {
                    info!("Got stream message!");
                    if let Ok((msg, _)) = bincode::decode_from_slice::<NetworkMessage, Configuration>(
                        &buf[..n],
                        Configuration::default(),
                    ) {
                        let _ = stream_tx.send(msg);
                    }
                }
                _ => break,
            }
        }
    });

    tokio::spawn(async move {
        loop {
            match datagram_connection.read_datagram().await {
                Ok(bytes) => {
                    info!("Got datagram!");
                    if let Ok((msg, _)) = bincode::decode_from_slice::<NetworkMessage, Configuration>(
                        &bytes,
                        Configuration::default(),
                    ) {
                        let _ = tx.send(msg);
                    }
                }
                Err(err) => {
                    warn!("Datagram reader stopped: {err}");
                    break;
                }
            }
        }
    });

    // Writer loop
    while let Some(msg) = rx.recv().await {
        info!("Sending message");
        let data = bincode::encode_to_vec::<_, Configuration>(&msg, Configuration::default())?;
        send.write_all(&data).await?;
    }

    Ok(())
}

/// Dummy certificate verifier that treats any certificate as valid.
/// NOTE, such verification is vulnerable to MITM attacks, but convenient for testing.
#[derive(Debug)]
struct SkipServerVerification(Arc<CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
