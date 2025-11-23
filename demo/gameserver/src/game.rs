use bevy_ecs::prelude::*;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Component, Clone, Copy, Serialize, Deserialize, Encode, Decode)]
pub struct Position {
    pub distance: f32, // meters along route
}

#[derive(Component, Clone, Copy, Serialize, Deserialize, Encode, Decode)]
pub struct Velocity {
    pub speed: f32, // m/s
}

#[derive(Component, Clone, Copy, Serialize, Deserialize, Encode, Decode)]
pub struct Power {
    pub watts: f32,
}

#[derive(Component, Clone, Copy)]
pub struct Player {
    pub client_id: u64,
}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub struct PlayerState {
    pub client_id: u64,
    pub position: Position,
    pub velocity: Velocity,
    pub power: Power,
}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub enum NetworkMessage {
    Input { power: f32 },
    WorldState(Vec<PlayerState>),
}

pub struct GameWorld {
    world: World,
    schedule: Schedule,
}

impl GameWorld {
    pub fn new() -> Self {
        let world = World::new();
        let mut schedule = Schedule::default();

        // Add systems
        schedule.add_systems((physics_system, drag_system));

        Self { world, schedule }
    }

    pub fn spawn_player(&mut self, client_id: u64) {
        self.world.spawn((
            Player { client_id },
            Position { distance: 0.0 },
            Velocity { speed: 0.0 },
            Power { watts: 0.0 },
        ));
    }

    pub fn despawn_player(&mut self, client_id: u64) {
        let mut to_despawn = None;
        let mut query = self.world.query::<(Entity, &Player)>();

        for (entity, player) in query.iter(&self.world) {
            if player.client_id == client_id {
                to_despawn = Some(entity);
                break;
            }
        }

        if let Some(entity) = to_despawn {
            self.world.despawn(entity);
        }
    }

    pub fn update_player_input(&mut self, client_id: u64, watts: f32) {
        let mut query = self.world.query::<(&Player, &mut Power)>();

        for (player, mut power) in query.iter_mut(&mut self.world) {
            if player.client_id == client_id {
                power.watts = watts.max(0.0).min(1000.0); // Clamp to reasonable range
                break;
            }
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.world.insert_resource(DeltaTime(dt));
        self.schedule.run(&mut self.world);
    }

    pub fn get_state(&mut self) -> Vec<PlayerState> {
        let mut states = Vec::new();
        let mut query = self
            .world
            .query::<(&Player, &Position, &Velocity, &Power)>();

        for (player, pos, vel, power) in query.iter(&self.world) {
            states.push(PlayerState {
                client_id: player.client_id,
                position: *pos,
                velocity: *vel,
                power: *power,
            });
        }

        states
    }
}

#[derive(Resource)]
struct DeltaTime(f32);

// Physics system: convert power to velocity (simplified physics)
fn physics_system(time: Res<DeltaTime>, mut query: Query<(&Power, &mut Velocity)>) {
    const RIDER_MASS: f32 = 75.0; // kg (rider + bike)
    const EFFICIENCY: f32 = 0.95;

    for (power, mut velocity) in query.iter_mut() {
        // Simplified: Power = Force * Velocity
        // Force = Power / Velocity
        // Acceleration = Force / Mass
        let current_speed = velocity.speed.max(1.0); // Avoid division by zero
        let force = (power.watts * EFFICIENCY) / current_speed;
        let acceleration = force / RIDER_MASS;

        velocity.speed += acceleration * time.0;
        velocity.speed = velocity.speed.max(0.0);
    }
}

// Drag system: air resistance and rolling resistance
fn drag_system(time: Res<DeltaTime>, mut query: Query<(&mut Velocity, &mut Position)>) {
    const AIR_DENSITY: f32 = 1.225; // kg/m³
    const DRAG_COEFFICIENT: f32 = 0.88;
    const FRONTAL_AREA: f32 = 0.4; // m²
    const ROLLING_RESISTANCE: f32 = 0.004;
    const RIDER_MASS: f32 = 75.0;
    const GRAVITY: f32 = 9.81;

    for (mut velocity, mut position) in query.iter_mut() {
        let speed = velocity.speed;

        // Air drag: F = 0.5 * ρ * Cd * A * v²
        let air_drag = 0.5 * AIR_DENSITY * DRAG_COEFFICIENT * FRONTAL_AREA * speed * speed;

        // Rolling resistance: F = Crr * m * g
        let rolling_drag = ROLLING_RESISTANCE * RIDER_MASS * GRAVITY;

        let total_drag = air_drag + rolling_drag;
        let deceleration = total_drag / RIDER_MASS;

        velocity.speed = (speed - deceleration * time.0).max(0.0);

        // Update position
        position.distance += velocity.speed * time.0;
    }
}
