use crate::network::{NetworkClient, NetworkMessage, SessionStarted, WorldStateUpdate};
use bevy::prelude::*;

#[derive(Component)]
pub struct LocalPlayer;

#[derive(Component)]
pub struct RemotePlayer {
    pub client_id: u64,
}

#[derive(Component)]
pub struct HudText;

#[derive(Component)]
pub struct DisplayPosition(pub f32);

#[derive(Component)]
pub struct DisplaySpeed(pub f32);

#[derive(Component)]
pub struct DisplayPower(pub f32);

#[derive(Component)]
pub struct DisplayRank(pub u32);

#[derive(Component)]
pub struct DisplayLap(pub u32);

#[derive(Resource)]
pub struct CurrentPower(pub f32);

#[derive(Resource)]
pub struct RaceSession {
    pub client_id: Option<u64>,
    pub route_length_m: f32,
    pub tick_hz: u32,
    pub last_tick: u64,
    pub datagrams_enabled: bool,
}

#[derive(Resource)]
pub struct InputSendTimer(pub Timer);

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CurrentPower(0.0))
            .insert_resource(RaceSession {
                client_id: None,
                route_length_m: 1200.0,
                tick_hz: 20,
                last_tick: 0,
                datagrams_enabled: false,
            })
            .insert_resource(InputSendTimer(Timer::from_seconds(
                0.05,
                TimerMode::Repeating,
            )))
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (
                    handle_session_events,
                    handle_input,
                    send_input_to_server,
                    update_world_from_network,
                    // update_camera,
                    update_ui,
                ),
            );
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Spawn camera
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 2.0, 1.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.2, 0.2))),
        Transform::from_xyz(0.0, 1.0, 0.0),
        DisplayPosition(0.0),
        DisplayPower(0.0),
        DisplaySpeed(0.0),
        DisplayRank(0),
        DisplayLap(0),
        LocalPlayer,
    ));

    // Add light
    commands.spawn((
        DirectionalLight {
            illuminance: 10000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.7, 0.5, 0.0)),
    ));

    // Spawn ground/road
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(1000.0, 100.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.3, 0.3))),
        // Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
    ));

    for marker in 0..=12 {
        let distance = marker as f32 * 100.0;
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(12.0, 0.05, 0.35))),
            MeshMaterial3d(materials.add(if marker == 0 {
                Color::srgb(1.0, 0.95, 0.3)
            } else {
                Color::srgb(0.8, 0.8, 0.8)
            })),
            Transform::from_xyz(0.0, 0.03, -distance),
        ));
    }

    commands.spawn((
        Text::new("Connecting to QUIC gameserver..."),
        TextFont { ..default() },
        Node {
            position_type: PositionType::Absolute,
            bottom: px(5),
            left: px(15),
            ..default()
        },
        HudText,
    ));
}

fn handle_input(keyboard: Res<ButtonInput<KeyCode>>, mut power: ResMut<CurrentPower>) {
    let mut delta = 0.0;

    if keyboard.pressed(KeyCode::KeyW) || keyboard.pressed(KeyCode::ArrowUp) {
        delta += 5.0;
    }
    if keyboard.pressed(KeyCode::KeyS) || keyboard.pressed(KeyCode::ArrowDown) {
        delta -= 5.0;
    }
    // if keyboard.pressed(KeyCode::L)

    power.0 = (power.0 + delta).clamp(0.0, 500.0);
}

fn handle_session_events(
    mut events: MessageReader<SessionStarted>,
    mut session: ResMut<RaceSession>,
) {
    for event in events.read() {
        session.client_id = Some(event.client_id);
        session.tick_hz = event.tick_hz;
        session.route_length_m = event.route_length_m;
        session.datagrams_enabled = event.datagrams_enabled;
    }
}

fn send_input_to_server(
    time: Res<Time>,
    power: Res<CurrentPower>,
    client: Res<NetworkClient>,
    mut timer: ResMut<InputSendTimer>,
) {
    if !timer.0.tick(time.delta()).just_finished() {
        return;
    }

    if let Err(err) = client.tx.send(NetworkMessage::Input { power: power.0 }) {
        error!(%err, "failed to send input to server");
    }
}

fn update_world_from_network(
    mut commands: Commands,
    mut events: MessageReader<WorldStateUpdate>,
    mut session: ResMut<RaceSession>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut local_query: Query<
        (
            &mut Transform,
            &mut DisplayPosition,
            &mut DisplaySpeed,
            &mut DisplayPower,
            &mut DisplayRank,
            &mut DisplayLap,
        ),
        With<LocalPlayer>,
    >,
    mut remote_query: Query<
        (Entity, &RemotePlayer, &mut Transform, &mut DisplayPosition),
        Without<LocalPlayer>,
    >,
) {
    for event in events.read() {
        session.last_tick = event.tick;
        session.route_length_m = event.route_length_m;

        // Update or spawn players
        for state in &event.players {
            if Some(state.client_id) == session.client_id {
                if let Ok((mut transform, mut pos, mut speed, mut power, mut rank, mut lap)) =
                    local_query.single_mut()
                {
                    transform.translation.x = lane_for(state.client_id);
                    transform.translation.z =
                        -(state.position.distance % session.route_length_m.max(1.0));
                    transform.translation.y = 1.0;

                    rank.0 = state.rank;
                    lap.0 = state.lap;
                    power.0 = state.power.watts;
                    speed.0 = state.velocity.speed;
                    pos.0 = state.position.distance;
                }
            } else {
                // Find or spawn remote player
                let mut found = false;
                for (_entity, remote, mut transform, mut pos) in remote_query.iter_mut() {
                    if remote.client_id == state.client_id {
                        pos.0 = state.position.distance;
                        transform.translation.x = lane_for(state.client_id);
                        transform.translation.z =
                            -(state.position.distance % session.route_length_m.max(1.0));
                        transform.translation.y = 1.0;
                        found = true;
                        break;
                    }
                }

                if !found {
                    commands.spawn((
                        Mesh3d(meshes.add(Cuboid::new(1.0, 2.0, 1.0))),
                        MeshMaterial3d(materials.add(Color::srgb(0.2, 0.8, 0.2))),
                        Transform::from_xyz(
                            lane_for(state.client_id),
                            1.0,
                            -(state.position.distance % session.route_length_m.max(1.0)),
                        ),
                        RemotePlayer {
                            client_id: state.client_id,
                        },
                        DisplayPosition(state.position.distance),
                    ));
                }
            }
        }
    }
}

fn update_ui(
    mut text_query: Query<&mut Text, With<HudText>>,
    session: Res<RaceSession>,
    player_query: Query<
        (
            &DisplayPosition,
            &DisplaySpeed,
            &DisplayPower,
            &DisplayRank,
            &DisplayLap,
        ),
        With<LocalPlayer>,
    >,
    power: Res<CurrentPower>,
) {
    if let Ok((pos, speed, server_power, rank, lap)) = player_query.single() {
        for mut text in text_query.iter_mut() {
            let transport = if session.datagrams_enabled {
                "stream + datagrams"
            } else {
                "stream only"
            };
            text.0 = format!(
                "RVM QUIC demo | client {:?} | {} | tick {}\nInput {:.0} W -> server {:.0} W | {:.1} km/h | {:.0} m | lap {} | rank #{}",
                session.client_id,
                transport,
                session.last_tick,
                power.0,
                server_power.0,
                speed.0 * 3.6,
                pos.0,
                lap.0 + 1,
                rank.0,
            );
        }
    }
}

fn lane_for(client_id: u64) -> f32 {
    (client_id % 5) as f32 * 2.5 - 5.0
}
