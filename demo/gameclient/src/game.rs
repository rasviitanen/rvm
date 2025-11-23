use crate::network::{NetworkClient, NetworkMessage, PlayerState, WorldStateUpdate};
use bevy::prelude::*;

#[derive(Component)]
pub struct LocalPlayer;

#[derive(Component)]
pub struct RemotePlayer {
    pub client_id: u64,
}

#[derive(Component)]
pub struct DisplayPosition(pub f32);

#[derive(Component)]
pub struct DisplaySpeed(pub f32);

#[derive(Component)]
pub struct DisplayPower(pub f32);

#[derive(Resource)]
pub struct CurrentPower(pub f32);

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CurrentPower(0.0))
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (
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
        Transform::from_xyz(0.0, 0.0, 0.0),
        DisplayPosition(0.0),
        DisplayPower(0.0),
        DisplaySpeed(0.0),
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

    commands.spawn((
        Text::new("Power: "),
        TextFont { ..default() },
        Node {
            position_type: PositionType::Absolute,
            bottom: px(5),
            left: px(15),
            ..default()
        },
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

fn send_input_to_server(power: Res<CurrentPower>, client: Res<NetworkClient>) {
    if let Err(err) = client.tx.send(NetworkMessage::Input { power: power.0 }) {
        error!(%err, "failed to send input to server")
    }
}

fn update_world_from_network(
    mut commands: Commands,
    mut events: MessageReader<WorldStateUpdate>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut local_query: Query<
        (
            Entity,
            &mut DisplayPosition,
            &mut DisplaySpeed,
            &mut DisplayPower,
        ),
        With<LocalPlayer>,
    >,
    mut remote_query: Query<
        (Entity, &RemotePlayer, &mut Transform, &mut DisplayPosition),
        Without<LocalPlayer>,
    >,
) {
    for WorldStateUpdate(states) in events.read() {
        // Update or spawn players
        for state in states {
            // Check if this is the local player (first one we see is ours for now)
            if let Ok((entity, mut pos, mut speed, mut power)) = local_query.single_mut() {
                pos.0 = state.position.distance;
                speed.0 = state.velocity.speed;
                power.0 = state.power.watts;
            } else {
                // Find or spawn remote player
                let mut found = false;
                for (entity, remote, mut transform, mut pos) in remote_query.iter_mut() {
                    if remote.client_id == state.client_id {
                        pos.0 = state.position.distance;
                        transform.translation.z = -state.position.distance;
                        found = true;
                        break;
                    }
                }

                if !found {
                    // Spawn new remote player
                    commands.spawn((
                        Mesh3d(meshes.add(Cuboid::new(1.0, 2.0, 1.0))),
                        MeshMaterial3d(materials.add(Color::srgb(0.2, 0.8, 0.2))),
                        Transform::from_xyz(0.0, 1.0, -state.position.distance),
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
    mut text_query: Query<&mut Text>,
    player_query: Query<(&DisplayPosition, &DisplaySpeed, &DisplayPower), With<LocalPlayer>>,
) {
    if let Ok((pos, speed, power)) = player_query.single() {
        for mut text in text_query.iter_mut() {
            text.0 = format!(
                "{:.1} km/h | {:.0} W | {:.0} m",
                speed.0 * 3.6,
                power.0,
                pos.0
            );
            // text.sections[1].value = format!("{:.1} km/h\n", speed.0 * 3.6);
            // text.sections[3].value = format!("{:.0} W\n", power.0);
            // text.sections[5].value = format!("{:.0} m\n", pos.0);
        }
    }
}
