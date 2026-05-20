use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Mutex,
        mpsc::{self, Receiver},
    },
    time::Duration,
};

use bevy::prelude::*;
use serde::Deserialize;

use crate::network::StartNetworkSession;

const CONTROL_PLANE_ADDR: &str = "127.0.0.1:8000";
const SERVICE_PREFIX: &str = "/game-service";

#[derive(Resource)]
pub struct ControlPlaneClient {
    rx: Mutex<Receiver<ControlPlaneResult>>,
}

#[derive(Resource, Default)]
pub struct ControlPlaneState {
    pub status: String,
    pub token: Option<String>,
    pub rider_name: Option<String>,
    pub races: Vec<Race>,
    pub queue: Option<Queue>,
    pub match_ticket: Option<String>,
    pub quic_endpoint: Option<String>,
    pub started_network: bool,
}

#[derive(Clone, Deserialize)]
pub struct Race {
    pub id: String,
    pub name: String,
    pub route: String,
    #[serde(rename = "startsInSeconds")]
    pub starts_in_seconds: u64,
    #[serde(rename = "distanceMeters")]
    pub distance_meters: u64,
    pub laps: u32,
    pub category: String,
    pub registered: u32,
}

#[derive(Clone, Deserialize)]
pub struct Queue {
    pub mode: String,
    pub status: String,
    #[serde(rename = "ridersWaiting")]
    pub riders_waiting: u32,
    #[serde(rename = "estimatedWaitSeconds")]
    pub estimated_wait_seconds: u64,
    #[serde(rename = "targetFieldSize")]
    pub target_field_size: u32,
    pub category: String,
}

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
    rider: Rider,
}

#[derive(Deserialize)]
struct Rider {
    name: String,
}

#[derive(Deserialize)]
struct RacesResponse {
    races: Vec<Race>,
}

#[derive(Deserialize)]
struct QueueResponse {
    queue: Queue,
}

#[derive(Deserialize)]
struct MatchmakingResponse {
    ticket: String,
}

#[derive(Deserialize)]
struct BootstrapResponse {
    #[serde(rename = "quicEndpoint")]
    quic_endpoint: String,
}

enum ControlPlaneResult {
    Ready {
        token: String,
        rider_name: String,
        races: Vec<Race>,
        queue: Queue,
        ticket: String,
        quic_endpoint: String,
    },
    Failed(String),
}

pub struct ControlPlanePlugin;

impl Plugin for ControlPlanePlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = fetch_control_plane_flow();
            let _ = tx.send(result);
        });

        app.insert_resource(ControlPlaneClient { rx: Mutex::new(rx) })
            .insert_resource(ControlPlaneState {
                status: "Logging in to RVM Tour...".to_owned(),
                ..default()
            })
            .add_systems(Update, poll_control_plane);
    }
}

fn poll_control_plane(
    client: Res<ControlPlaneClient>,
    mut state: ResMut<ControlPlaneState>,
    mut start_network: MessageWriter<StartNetworkSession>,
) {
    let Ok(rx) = client.rx.lock() else {
        state.status = "Control plane receiver lock failed".to_owned();
        return;
    };

    while let Ok(result) = rx.try_recv() {
        match result {
            ControlPlaneResult::Ready {
                token,
                rider_name,
                races,
                queue,
                ticket,
                quic_endpoint,
            } => {
                state.status = "Matchmaking ready. Connecting to game server...".to_owned();
                state.token = Some(token);
                state.rider_name = Some(rider_name);
                state.races = races;
                state.queue = Some(queue);
                state.match_ticket = Some(ticket);
                state.quic_endpoint = Some(quic_endpoint.clone());

                if !state.started_network {
                    state.started_network = true;
                    start_network.write(StartNetworkSession {
                        endpoint: quic_endpoint,
                    });
                }
            }
            ControlPlaneResult::Failed(err) => {
                state.status = format!("Control plane error: {err}");
            }
        }
    }
}

fn fetch_control_plane_flow() -> ControlPlaneResult {
    match fetch_control_plane_flow_inner() {
        Ok(flow) => flow,
        Err(err) => ControlPlaneResult::Failed(err),
    }
}

fn fetch_control_plane_flow_inner() -> Result<ControlPlaneResult, String> {
    let login: LoginResponse = request("POST", "/login?rider=Demo%20Rider", None)?;
    let auth = format!("Bearer {}", login.token);
    let races: RacesResponse = request("GET", "/races", Some(&auth))?;
    let queue: QueueResponse = request("GET", "/matchmaking/queue", Some(&auth))?;
    let race_id = races
        .races
        .first()
        .map(|race| race.id.as_str())
        .unwrap_or("crit-city-1830");
    let matchmaking: MatchmakingResponse = request(
        "POST",
        &format!("/matchmaking/join?race={race_id}"),
        Some(&auth),
    )?;
    let bootstrap: BootstrapResponse = request("GET", "/session/bootstrap", Some(&auth))?;

    Ok(ControlPlaneResult::Ready {
        token: login.token,
        rider_name: login.rider.name,
        races: races.races,
        queue: queue.queue,
        ticket: matchmaking.ticket,
        quic_endpoint: bootstrap.quic_endpoint,
    })
}

fn request<T: for<'de> Deserialize<'de>>(
    method: &str,
    path: &str,
    authorization: Option<&str>,
) -> Result<T, String> {
    let mut stream = TcpStream::connect(CONTROL_PLANE_ADDR)
        .map_err(|err| format!("connect {CONTROL_PLANE_ADDR}: {err}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|err| format!("set read timeout: {err}"))?;

    let path = format!("{SERVICE_PREFIX}{path}");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {CONTROL_PLANE_ADDR}\r\nConnection: close\r\nContent-Length: 0\r\n"
    );
    if let Some(authorization) = authorization {
        request.push_str(&format!("Authorization: {authorization}\r\n"));
    }
    request.push_str("\r\n");

    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("{method} {path}: write failed: {err}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|err| format!("{method} {path}: read failed: {err}"))?;

    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("{method} {path}: malformed HTTP response"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("{method} {path}: missing HTTP status"))?;

    let body = if has_chunked_transfer_encoding(head) {
        decode_chunked(body)
            .map_err(|err| format!("{method} {path}: decode chunked body: {err}"))?
    } else {
        body.to_owned()
    };

    if !(200..300).contains(&status) {
        return Err(format!("{method} {path}: HTTP {status}: {body}"));
    }

    serde_json::from_str(&body).map_err(|err| format!("{method} {path}: decode JSON: {err}"))
}

fn has_chunked_transfer_encoding(headers: &str) -> bool {
    headers.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        line.starts_with("transfer-encoding:") && line.contains("chunked")
    })
}

fn decode_chunked(body: &str) -> Result<String, String> {
    let mut rest = body;
    let mut decoded = String::new();

    loop {
        let Some((size_line, after_size)) = rest.split_once("\r\n") else {
            return Err("missing chunk size".to_owned());
        };
        let size_hex = size_line.split(';').next().unwrap_or(size_line);
        let size = usize::from_str_radix(size_hex.trim(), 16).map_err(|err| format!("{err}"))?;
        if size == 0 {
            return Ok(decoded);
        }
        if after_size.len() < size + 2 {
            return Err("chunk shorter than declared size".to_owned());
        }

        decoded.push_str(&after_size[..size]);
        rest = &after_size[size + 2..];
    }
}
