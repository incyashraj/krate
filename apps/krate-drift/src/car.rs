//! Car physics, lap tracking and collision.
//!
//! Not a simulator. The model is the arcade one -- speed along the heading,
//! grip that gives way past a threshold, and a slip angle that the body is
//! drawn at -- because the target is a game that feels good to drive, not a
//! vehicle dynamics exercise. What it does have is the thing the benchmark
//! version lacked entirely: the car can hit something.

use crate::mathx::{abs, angle_delta, atan2_approx, cos_approx, sin_approx, sqrt_approx};
use crate::track::{Track, ROAD_HALF, VERGE_HALF};

/// Laps in a race.
/// The default race length, and the ceiling `Car` uses when nobody says
/// otherwise.
///
/// The race length is a SETTING now -- the menu changes it -- so the value
/// that matters is carried on the car rather than read from here. This stays
/// as the default a fresh car gets, and as the number the physics falls back
/// to if a car is built outside a race.
pub const MAX_LAPS: u32 = 3;

/// Half-length of the car's body. Collision is a circle around the centre
/// rather than a box, so the width is not needed: see `Car::collide`.
pub const CAR_HALF_L: f32 = 2.2;

#[derive(Clone, Copy, PartialEq)]
pub enum Surface {
    Road,
    Grass,
}

pub struct Car {
    pub x: f32,
    pub z: f32,
    pub y: f32,
    /// Where the nose points, radians.
    pub heading: f32,
    /// Speed along the heading.
    pub speed: f32,
    /// How far the body is rotated away from the direction of travel. Purely
    /// visual and audible, but it is most of what "drifting" means to a
    /// player, so it is state rather than something recomputed for drawing.
    pub slip: f32,
    /// Vertical velocity, so crests actually launch the car.
    pub vy: f32,
    pub airborne: bool,

    /// Nearest centreline node, carried between frames as the search hint.
    pub node: usize,
    pub lap: u32,
    /// Distance travelled along the lap, used for ordering the field.
    pub progress: f32,
    /// Guards the start/finish line so crossing it backwards, or twice in two
    /// frames, cannot bank a lap.
    pub lap_armed: bool,
    pub finished: bool,
    pub finish_time: f32,
    /// How many laps THIS race is. Carried per car rather than read from a
    /// constant, so the menu can change the race length without the physics
    /// needing to know a menu exists.
    pub race_laps: u32,

    pub surface: Surface,
    /// Set for one frame when the car hits something, so the caller can play a
    /// sound and shake the camera without the physics knowing about either.
    pub hit: f32,
    /// Whether the car was already against the barrier last frame, so a long
    /// scrape is one impact rather than one per frame.
    pub on_barrier: bool,
    /// Set while the brake is being held from a forward roll, so the car stops
    /// at zero instead of continuing into reverse. Cleared when the brake is
    /// released, which is what makes reverse a deliberate second press.
    pub braked_to_stop: bool,
}

impl Car {
    pub fn new(track: &Track, slot: usize, field: usize) -> Self {
        // Grid slots stagger back and alternate sides, as a real grid does.
        //
        // The grid sits BEHIND the start line, as a real one does, and the
        // counter starts armed. Both halves matter and getting either alone
        // wrong costs a lap in one direction or the other:
        //
        // - grid in front of the line, counter disarmed: the car has to go all
        //   the way round before it can arm, so the crossing that ends lap one
        //   is ignored and a three-lap race needs four crossings.
        // - grid in front of the line, counter armed: the grid is already
        //   inside the window that banks a lap, so lap one is credited on the
        //   first frame and the race is a lap short. (Measured: a three-lap
        //   race finished in 55 s when a lap takes 27.)
        //
        // Behind the line and armed, the first crossing is a real one.
        //
        // Pole is the slot FURTHEST along the lap, so later slots step
        // backwards down the track. Stepping forwards instead put the last row
        // ahead of pole, and the player -- always slot 0 -- started behind the
        // entire field and finished last however fast they drove.
        let n = track.nodes.len();
        let rows = field.div_ceil(2);
        // Nine nodes back per row from a point just before the line. Pole
        // (row 0) sits closest to the line and later rows step further back.
        let back = 4 + (slot / 2) * 9;
        let _ = rows;
        let idx = (n - back) % n;
        let node = track.nodes[idx];
        let side = if slot % 2 == 0 { 4.5 } else { -4.5 };
        let nx = -node.dir_z;
        let nz = node.dir_x;
        Self {
            x: node.x + nx * side,
            z: node.z + nz * side,
            y: node.y,
            // `heading` is measured so that the car moves by
            // (sin h, cos h): h = 0 points along +Z. Converting a direction
            // to that convention is atan2(dir_x, dir_z), with x FIRST.
            // Passing them in the usual (y, x) order instead measures the
            // angle from +X, which is ninety degrees out -- every car drove
            // straight off the circuit at the green light.
            heading: atan2_approx(node.dir_x, node.dir_z),
            speed: 0.0,
            slip: 0.0,
            vy: 0.0,
            airborne: false,
            node: idx,
            lap: 0,
            progress: 0.0,
            // NOT armed. With the grid behind the start line this is correct
            // on its own: the car crosses the line a few metres after the
            // lights, which is ignored because it has not armed yet, then arms
            // in the middle of its first lap and trips at the end of it. Lap
            // one counts as lap one.
            //
            // Pre-arming here instead credited a lap on the first crossing,
            // twenty metres in, and a three-lap race finished in 57 s when a
            // lap takes 27.
            lap_armed: false,
            finished: false,
            finish_time: 0.0,
            race_laps: MAX_LAPS,
            surface: Surface::Road,
            hit: 0.0,
            on_barrier: false,
            braked_to_stop: false,
        }
    }

    /// Advance one step.
    ///
    /// `throttle` is -1..1 and `steer` is -1..1, so a human and an opponent
    /// drive through exactly the same function. That is deliberate: an
    /// opponent that cheats -- that follows the line by teleporting along it,
    /// or ignores grass -- stops being a test of the physics, and the moment
    /// it touches the player it exports its cheating into the race.
    pub fn step(&mut self, track: &Track, throttle: f32, steer: f32, dt: f32, now: f32) {
        self.hit = (self.hit - dt * 3.0).max(0.0);

        let where_ = track.locate(self.x, self.z, self.node);
        self.node = where_.node;
        let off = abs(where_.offset);
        self.surface = if off <= ROAD_HALF {
            Surface::Road
        } else {
            Surface::Grass
        };

        let (grip, drag, power) = match self.surface {
            Surface::Road => (1.0, 0.45, 58.0),
            // Grass is slow and vague rather than an instant stop: a spin that
            // ends the race outright is a punishment, not a consequence.
            Surface::Grass => (0.55, 2.4, 26.0),
        };

        // Longitudinal.
        if throttle > 0.0 {
            self.speed += throttle * power * dt;
        } else if throttle < 0.0 {
            // Braking bites harder than reverse accelerates.
            let brake = if self.speed > 0.0 { 78.0 } else { 26.0 };
            if self.speed > 0.0 {
                self.braked_to_stop = true;
            }
            self.speed += throttle * brake * dt;
            // Braking STOPS the car; it does not drive it backwards in the
            // same press. Without this, holding the brake to avoid the car in
            // front carried straight through zero into reverse about a tenth
            // of a second later and drove into whatever was behind.
            //
            // The flag has to LATCH for as long as the brake is held: guarding
            // only the frame that crosses zero leaves the next frame starting
            // from a standstill, where nothing says the car was ever moving
            // forward, and reverse begins anyway. Releasing the brake clears
            // it, which is what makes reverse a deliberate second press.
            if self.braked_to_stop && self.speed < 0.0 {
                self.speed = 0.0;
            }
        }
        if throttle >= 0.0 {
            self.braked_to_stop = false;
        }
        self.speed -= self.speed * drag * dt;
        self.speed = self.speed.clamp(-22.0, 92.0);

        // Steering authority falls away at very low speed, so the car does not
        // pivot on the spot, and at very high speed, so a straight is stable.
        let v = abs(self.speed);
        let authority = (v / 26.0).min(1.0) * (1.0 - (v / 190.0).min(0.55));
        let turn = steer * 2.35 * authority * grip * dt;
        self.heading += turn;

        // Slip: the body lags the heading when turning hard at speed, and
        // recovers when not. This is the drift, and it is what the tyre sound
        // and the camera lean both read from.
        let target_slip = -turn * (v / 30.0).min(2.2);
        self.slip += (target_slip - self.slip) * (6.0 * dt).min(1.0);
        self.slip = self.slip.clamp(-0.6, 0.6);

        // Position.
        self.x += sin_approx(self.heading) * self.speed * dt;
        self.z += cos_approx(self.heading) * self.speed * dt;

        // Vertical: follow the road, but keep momentum over a crest so a fast
        // rise actually throws the car into the air.
        let ground = track.surface_height(&where_) + terrain_bias(where_.offset);
        if self.airborne {
            self.vy -= 34.0 * dt;
            self.y += self.vy * dt;
            if self.y <= ground {
                self.y = ground;
                self.vy = 0.0;
                self.airborne = false;
            }
        } else {
            let climb = (ground - self.y) / dt.max(0.0001);
            if climb < -30.0 && v > 30.0 {
                self.airborne = true;
                self.vy = 0.0;
            } else {
                self.y += (ground - self.y) * (12.0 * dt).min(1.0);
            }
        }

        // The barrier. Past the verge the car is pushed back and loses speed:
        // a wall you can lean on, not one that ends the race.
        if off > VERGE_HALF {
            let node = track.nodes[where_.node];
            let nx = -node.dir_z;
            let nz = node.dir_x;
            let excess = off - VERGE_HALF;
            let sign = if where_.offset > 0.0 { 1.0 } else { -1.0 };
            self.x -= nx * sign * excess;
            self.z -= nz * sign * excess;
            self.speed *= 0.965;
            // Only the moment of ARRIVAL counts as a hit. Setting it every
            // frame the car is against the barrier pins the flash on and
            // retriggers the impact sound at frame rate, so a car scraping
            // along a wall strobes white and buzzes.
            if v > 18.0 && !self.on_barrier {
                self.hit = 1.0;
            }
            self.on_barrier = true;
        } else {
            self.on_barrier = false;
        }

        self.update_lap(track, &where_, now);
    }

    /// Lap counting, guarded so it cannot be gamed or double-counted.
    fn update_lap(&mut self, track: &Track, where_: &crate::track::Where, now: f32) {
        let n = track.nodes.len() as f32;
        let quarter = track.length / 4.0;
        self.progress = self.lap as f32 * track.length + where_.along;

        // Arm the line only once the car is properly round the lap, so
        // reversing over the start line, or sitting on it, cannot count.
        if where_.along > quarter * 2.0 && where_.along < quarter * 3.5 {
            self.lap_armed = true;
        }
        let _ = n;
        if self.lap_armed && where_.along < quarter * 0.5 && self.speed > 0.0 {
            self.lap_armed = false;
            self.lap += 1;
            if self.lap >= self.race_laps && !self.finished {
                self.finished = true;
                self.finish_time = now;
            }
        }
    }

    /// Push two cars apart when their bodies overlap.
    ///
    /// Circle-based rather than box-based: a box needs a separating-axis test
    /// per pair, and at these speeds the visible difference is nil while the
    /// cost and the number of ways to get it wrong are both much higher.
    pub fn collide(a: &mut Car, b: &mut Car) {
        let dx = b.x - a.x;
        let dz = b.z - a.z;
        let d2 = dx * dx + dz * dz;
        let r = CAR_HALF_L * 1.25;
        let min = r * 2.0;
        if d2 >= min * min || d2 < 0.0001 {
            return;
        }
        let d = sqrt_approx(d2);
        let nx = dx / d;
        let nz = dz / d;
        let overlap = min - d;

        // Separate them, then exchange a little speed along the contact so a
        // rear-end shunt shoves the car in front rather than stopping dead.
        let push = overlap * 0.5;
        a.x -= nx * push;
        a.z -= nz * push;
        b.x += nx * push;
        b.z += nz * push;

        let closing = (b.speed - a.speed) * 0.5;
        a.speed += closing * 0.45;
        b.speed -= closing * 0.45;

        // Only a real knock flashes. Two cars running side by side stay
        // marginally overlapped for as long as they are alongside each other,
        // and setting `hit` every one of those frames pinned both bodies at
        // full flash -- the whole field drove around looking washed out, and
        // the impact sound retriggered at frame rate.
        if abs(closing) > 2.5 {
            a.hit = 1.0;
            b.hit = 1.0;
        }
    }

    /// Which way the body is drawn: heading plus slip.
    pub fn body_angle(&self) -> f32 {
        self.heading + self.slip
    }

    /// How sideways the car is, 0..1, for tyre noise and smoke.
    pub fn slide(&self) -> f32 {
        (abs(self.slip) / 0.6).min(1.0) * (abs(self.speed) / 40.0).min(1.0)
    }
}

/// A slight camber: the road crowns in the middle and drops at the edges, so
/// running wide has a felt consequence before the barrier arrives.
fn terrain_bias(offset: f32) -> f32 {
    let o = abs(offset);
    if o <= ROAD_HALF {
        0.0
    } else {
        -((o - ROAD_HALF) * 0.16).min(1.4)
    }
}

/// Steering and throttle for an opponent, aiming at a point up the road.
///
/// The look-ahead scales with speed, which is the whole trick: aiming at a
/// fixed distance makes a car that either saws at low speed or understeers
/// off every corner at high speed.
pub fn drive_ai(car: &Car, track: &Track, skill: f32) -> (f32, f32) {
    let ahead = 18.0 + abs(car.speed) * 0.85;
    let (tx, tz) = track.look_ahead(car.node, ahead);
    // x first: see the note in `Car::new` about the heading convention.
    let want = atan2_approx(tx - car.x, tz - car.z);
    let delta = angle_delta(car.heading, want);
    let steer = (delta * 2.1).clamp(-1.0, 1.0);

    // Slow down for a corner rather than only when already off the line: the
    // sharper the aim point, the less throttle.
    let sharp = (abs(delta) / 0.55).min(1.0);
    let target = 92.0 * skill * (1.0 - sharp * 0.62);
    let throttle = if car.speed < target { 1.0 } else { -0.35 };
    (throttle, steer)
}
