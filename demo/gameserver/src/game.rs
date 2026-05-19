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
    pub name: &'static str,
}

#[derive(Component, Clone, Copy)]
pub struct Bot {
    pub base_power: f32,
    pub phase: f32,
}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub struct PlayerState {
    pub client_id: u64,
    pub name: String,
    pub position: Position,
    pub velocity: Velocity,
    pub power: Power,
    pub lap: u32,
    pub rank: u32,
}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub enum NetworkMessage {
    Welcome {
        client_id: u64,
        tick_hz: u32,
        route_length_m: f32,
        datagrams_enabled: bool,
    },
    Input {
        power: f32,
    },
    WorldState {
        tick: u64,
        route_length_m: f32,
        players: Vec<PlayerState>,
    },
}

pub const TICK_HZ: u32 = 20;
pub const ROUTE_LENGTH_M: f32 = 1200.0;

pub struct GameWorld {
    world: World,
    schedule: Schedule,
    tick: u64,
}

impl GameWorld {
    pub fn new() -> Self {
        let world = World::new();
        let mut schedule = Schedule::default();

        // Add systems
        schedule.add_systems((physics_system, drag_system));

        let mut game = Self {
            world,
            schedule,
            tick: 0,
        };
        game.spawn_bot(10_001, "Tempo Bot", 195.0, 0.0, 35.0);
        game.spawn_bot(10_002, "Climber Bot", 230.0, 1.7, 5.0);
        game.spawn_bot(10_003, "Sprinter Bot", 160.0, 3.1, 80.0);
        game
    }

    pub fn spawn_player(&mut self, client_id: u64) {
        self.world.spawn((
            Player {
                client_id,
                name: "You",
            },
            Position { distance: 0.0 },
            Velocity { speed: 0.0 },
            Power { watts: 0.0 },
        ));
    }

    fn spawn_bot(
        &mut self,
        client_id: u64,
        name: &'static str,
        base_power: f32,
        phase: f32,
        start_distance: f32,
    ) {
        self.world.spawn((
            Player { client_id, name },
            Bot { base_power, phase },
            Position {
                distance: start_distance,
            },
            Velocity { speed: 6.0 },
            Power { watts: base_power },
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
                power.watts = watts.clamp(0.0, 1000.0);
                break;
            }
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.tick += 1;
        self.update_bots();
        self.world.insert_resource(DeltaTime(dt));
        self.schedule.run(&mut self.world);
    }

    fn update_bots(&mut self) {
        let elapsed = self.tick as f32 / TICK_HZ as f32;
        let mut query = self.world.query::<(&Bot, &mut Power)>();

        for (bot, mut power) in query.iter_mut(&mut self.world) {
            let surge = ((elapsed * 0.7 + bot.phase).sin() + 1.0) * 35.0;
            power.watts = bot.base_power + surge;
        }
    }

    pub fn get_state(&mut self) -> NetworkMessage {
        let mut query = self
            .world
            .query::<(&Player, &Position, &Velocity, &Power)>();

        let mut states = Vec::new();
        for (player, pos, vel, power) in query.iter(&self.world) {
            states.push(PlayerState {
                client_id: player.client_id,
                name: player.name.to_owned(),
                position: *pos,
                velocity: *vel,
                power: *power,
                lap: (pos.distance / ROUTE_LENGTH_M) as u32,
                rank: 0,
            });
        }

        states.sort_by(|a, b| b.position.distance.total_cmp(&a.position.distance));
        for (rank, state) in states.iter_mut().enumerate() {
            state.rank = rank as u32 + 1;
        }

        NetworkMessage::WorldState {
            tick: self.tick,
            route_length_m: ROUTE_LENGTH_M,
            players: states,
        }
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
