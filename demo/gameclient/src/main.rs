use bevy::{prelude::*, window::WindowResolution};
mod camera;
mod game;
mod network;

use game::GamePlugin;
use network::NetworkPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Cycling Game".to_string(),
                resolution: WindowResolution::new(1280, 720),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(NetworkPlugin)
        .add_plugins(GamePlugin)
        .add_plugins(camera::CameraPlugin)
        .run();
}
