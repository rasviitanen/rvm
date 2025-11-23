use std::f32::consts::{FRAC_PI_2, PI};

use bevy::{
    camera::{Exposure, visibility::RenderLayers},
    core_pipeline::{Skybox, tonemapping::Tonemapping},
    input::mouse::MouseMotion,
    light::{
        AtmosphereEnvironmentMapLight, CascadeShadowConfigBuilder, NotShadowCaster,
        NotShadowReceiver, light_consts::lux,
    },
    math::VectorSpace,
    pbr::{Atmosphere, AtmosphereSettings},
    post_process::bloom::Bloom,
    prelude::*,
    render::render_resource::{TextureViewDescriptor, TextureViewDimension},
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FollowCamera::default())
            .add_systems(Startup, init)
            .add_systems(Update, (dynamic_scene, grab_cursor, rotate_and_move));
    }
}

#[derive(Resource, Default, PartialEq, Eq)]
pub enum CursorState {
    #[default]
    Free,
    Locked,
    Follow,
}

#[derive(Resource)]
pub struct FollowCamera {
    /// Distance behind the character
    pub distance: f32,
    /// Height above the character
    pub height: f32,
    /// How quickly the camera moves (0.0-1.0, higher = faster)
    pub position_smoothing: f32,
    /// How quickly the camera rotates (0.0-1.0, higher = faster)
    pub rotation_smoothing: f32,
    /// Additional vertical offset for the look-at target
    pub look_offset: f32,
    /// Prediction strength - how far ahead to look (0.0 = no prediction)
    pub prediction_time: f32,
    /// Velocity tracking for prediction
    pub last_target_position: Vec3,
    /// Smoothed velocity for stable prediction
    pub smoothed_velocity: Vec3,
    /// Velocity smoothing factor
    pub velocity_smoothing: f32,
}

impl Default for FollowCamera {
    fn default() -> Self {
        Self {
            distance: 6.0,
            height: 2.75,
            position_smoothing: 0.2,
            rotation_smoothing: 0.1,
            look_offset: 2.5,
            prediction_time: 0.3,
            last_target_position: Vec3::ZERO,
            smoothed_velocity: Vec3::ZERO,
            velocity_smoothing: 0.2,
        }
    }
}

#[derive(Component)]
struct Sun;

pub fn init(mut commands: Commands) {
    let cascade_shadow_config = CascadeShadowConfigBuilder {
        first_cascade_far_bound: 0.3,
        maximum_distance: 3.0,
        ..default()
    }
    .build();

    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 1.0, 1.0),
            shadows_enabled: true,
            // lux::RAW_SUNLIGHT is recommended for use with this feature, since
            // other values approximate sunlight *post-scattering* in various
            // conditions. RAW_SUNLIGHT in comparison is the illuminance of the
            // sun unfiltered by the atmosphere, so it is the proper input for
            // sunlight to be filtered by the atmosphere.
            illuminance: lux::RAW_SUNLIGHT,
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::new(0.3, -1.0, 0.5), Vec3::Y),
        cascade_shadow_config,
        Sun,
    ));

    commands.spawn((
        Camera3d::default(),
        Camera {
            // renders after / on top of other cameras
            order: 0,
            ..Default::default()
        },
        // RenderLayers::layer(1),
        Transform::from_translation(Vec3::new(0.0, 1.0, -10.0)).looking_at(Vec3::Z, Vec3::Y),
        // This is the component that enables atmospheric scattering for a camera
        Atmosphere::EARTH,
        // The scene is in units of 10km, so we need to scale up the
        // aerial view lut distance and set the scene scale accordingly.
        // Most usages of this feature will not need to adjust this.
        AtmosphereSettings::default(),
        // Tonemapper chosen just because it looked good with the scene, any
        // tonemapper would be fine :)
        Tonemapping::AcesFitted,
        // Bloom gives the sun a much more natural look.
        Bloom::NATURAL,
        Exposure::SUNLIGHT,
        // Enables the atmosphere to drive reflections and ambient lighting (IBL) for this view
        AtmosphereEnvironmentMapLight::default(),
    ));

    commands.insert_resource(CursorState::Free);
}

pub fn grab_cursor(
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    mut cursor_state: ResMut<CursorState>,
) {
    let grabbed = if keyboard_input.just_pressed(KeyCode::Escape) {
        false
    } else if mouse_input.just_pressed(MouseButton::Left) {
        true
    } else {
        return;
    };

    cursor.grab_mode = if grabbed {
        CursorGrabMode::Confined
    } else {
        CursorGrabMode::None
    };

    *cursor_state = match *cursor_state {
        CursorState::Free => CursorState::Locked,
        CursorState::Locked => CursorState::Follow,
        CursorState::Follow => CursorState::Free,
    };

    cursor.visible = !grabbed;
}

pub fn rotate_and_move(
    time: Res<Time>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    cursor_state: Res<CursorState>,
) {
    if *cursor_state == CursorState::Follow {
        return;
    }
    let rotation_input = -mouse_motion
        .read()
        .fold(Vec2::ZERO, |acc, mot| acc + mot.delta);
    let movement_input = Vec3::new(
        keyboard_input.pressed(KeyCode::KeyD) as u32 as f32
            - keyboard_input.pressed(KeyCode::KeyA) as u32 as f32,
        keyboard_input.pressed(KeyCode::Space) as u32 as f32
            - keyboard_input.pressed(KeyCode::ControlLeft) as u32 as f32,
        keyboard_input.pressed(KeyCode::KeyS) as u32 as f32
            - keyboard_input.pressed(KeyCode::KeyW) as u32 as f32,
    );

    if rotation_input.length_squared() < 0.001 && movement_input.length_squared() < 0.001 {
        return;
    }

    for mut transform in cameras.iter_mut() {
        let translation = movement_input * time.delta_secs() * 10.;
        let translation = transform.rotation * translation;
        transform.translation += translation;
        transform.translation.y = transform.translation.y.max(-10.);

        if *cursor_state == CursorState::Locked {
            let mut euler = transform.rotation.to_euler(EulerRot::YXZ);
            euler.0 += rotation_input.x * 0.003;
            euler.1 += rotation_input.y * 0.003;
            transform.rotation = Quat::from_euler(EulerRot::YXZ, euler.0, euler.1, 0.);
        }
    }
}

fn dynamic_scene(mut suns: Query<&mut Transform, With<DirectionalLight>>, time: Res<Time>) {
    // suns.iter_mut()
    //     .for_each(|mut tf| tf.rotate_x(-time.delta_secs() * PI / 10.0));
}
