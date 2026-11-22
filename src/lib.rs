//! Freelook: hold a key (X by default; change it in Options > Controls > Key Binds) for a third person view you can
//! look around in. The mouse turns only the camera: the player keeps walking, aiming and facing
//! the way they did when the key went down. Scroll while holding it to change the camera's
//! distance. When the key is let go, the view goes back to what it was.
//!
//! How it works, with the mod API only (no game internals):
//! - The mouse still turns the player a little each frame. `render_frame_pre` and
//!   `client_tick_pre` read that turn, add it to the camera's own angles and put the player's
//!   angles back (`set_local_rotation`), so every tick and every packet sees the frozen ones.
//! - `camera_pose` draws the world from the camera's angles, a distance behind the eye that a
//!   block in the way shortens (found with `block_collision`, like vanilla's third person).
//! - `camera_distance` asks vanilla for distance 0, so the position vanilla gives `camera_pose`
//!   is the eye itself.
//! - `mouse_scrolling` takes the wheel while the key is held.
use mod_api::{
    self, CameraDistanceEvent, CameraPoseEvent, CameraType, ClientHost, ClientTickPreEvent, Flow,
    Hooks, KeyConflictContext, KeyRegistration, MouseScrollingEvent, Registrar, RegistrationContext,
    RenderFramePreEvent, SettingSpec,
};
#[allow(unused_imports)]
use mod_api::prelude::*;
use std::sync::Mutex;

/// The mod's id, as in `horizon-mod.toml`.
const ID: &str = "freelook";
/// The key's id in Controls.
const KEY: &str = "freelook";
const DEFAULT_KEY: &str = "key.keyboard.x";
/// The camera's distance in blocks: the game's own third person is 4.
const START_DISTANCE: f32 = 4.0;
/// The settings on the mod's Config page (Options > Mods).
const MODE_SETTING: &str = "mode";
const DISTANCE_SETTING: &str = "distance";
const MIN_DISTANCE: f32 = 1.0;
const MAX_DISTANCE: f32 = 24.0;

struct State {
    /// The key is held and freelook is on.
    held: bool,
    /// The player's yaw and pitch, frozen while it is on.
    frozen: [f32; 2],
    /// The camera's yaw and pitch, without the turn the mouse made since the last put-back.
    look: [f32; 2],
    /// The camera type to go back to.
    before: Option<CameraType>,
    /// Blocks behind the eye.
    distance: f32,
    /// The key was down at the last frame: a press is the frame it goes down.
    was_down: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            held: false,
            frozen: [0.0; 2],
            look: [0.0; 2],
            before: None,
            distance: START_DISTANCE,
            was_down: false,
        }
    }
}

impl State {
    /// Adds the player's turn since it was frozen (`current` is its angles now) to the camera's.
    fn absorb(&mut self, current: [f32; 2]) {
        self.look = self.view(current);
    }

    /// The camera's yaw and pitch for a player at `current`: the frozen look, plus the turn the
    /// mouse has made since.
    fn view(&self, current: [f32; 2]) -> [f32; 2] {
        [
            self.look[0] + (current[0] - self.frozen[0]),
            (self.look[1] + (current[1] - self.frozen[1])).clamp(-90.0, 90.0),
        ]
    }
}

/// The direction of a view with this yaw and pitch (degrees), as Minecraft turns them.
fn forward(yaw: f32, pitch: f32) -> [f64; 3] {
    let (yaw, pitch) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ]
}

/// Where the camera goes: `distance` blocks behind the eye, against the view's direction.
fn behind(eye: [f64; 3], yaw: f32, pitch: f32, distance: f32) -> [f64; 3] {
    let f = forward(yaw, pitch);
    let d = f64::from(distance);
    [eye[0] - f[0] * d, eye[1] - f[1] * d, eye[2] - f[2] * d]
}

/// Where a ray from `from` to `to` first meets a box: `boxes(cell)` gives a block's collision boxes
/// (block-local `[min_x, min_y, min_z, max_x, max_y, max_z]`), and the blocks the ray passes through
/// are asked in order.
fn ray_hit(
    from: [f64; 3],
    to: [f64; 3],
    mut boxes: impl FnMut([i32; 3]) -> Vec<[f64; 6]>,
) -> Option<[f64; 3]> {
    let dir = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut cell = [from[0].floor() as i32, from[1].floor() as i32, from[2].floor() as i32];
    let end = [to[0].floor() as i32, to[1].floor() as i32, to[2].floor() as i32];
    let mut step = [0i32; 3];
    // The ray's t (0 at `from`, 1 at `to`) where it leaves the cell on each axis, and the t one
    // cell takes.
    let mut t_max = [f64::INFINITY; 3];
    let mut t_delta = [f64::INFINITY; 3];
    for i in 0..3 {
        if dir[i] > 0.0 {
            step[i] = 1;
        } else if dir[i] < 0.0 {
            step[i] = -1;
        }
        if step[i] != 0 {
            let next = f64::from(cell[i]) + if step[i] > 0 { 1.0 } else { 0.0 };
            t_max[i] = (next - from[i]) / dir[i];
            t_delta[i] = (1.0 / dir[i]).abs();
        }
    }
    for _ in 0..512 {
        let origin = [f64::from(cell[0]), f64::from(cell[1]), f64::from(cell[2])];
        let mut best: Option<f64> = None;
        for b in boxes(cell) {
            let min = [origin[0] + b[0], origin[1] + b[1], origin[2] + b[2]];
            let max = [origin[0] + b[3], origin[1] + b[4], origin[2] + b[5]];
            if let Some(t) = slab(from, dir, min, max) {
                best = Some(best.map_or(t, |old| old.min(t)));
            }
        }
        if let Some(t) = best {
            return Some([from[0] + dir[0] * t, from[1] + dir[1] * t, from[2] + dir[2] * t]);
        }
        if cell == end {
            return None;
        }
        let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };
        if t_max[axis] > 1.0 {
            return None;
        }
        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];
    }
    None
}

/// The t (0 to 1) where the ray `from + dir * t` enters the box, 0 when it starts inside it.
fn slab(from: [f64; 3], dir: [f64; 3], min: [f64; 3], max: [f64; 3]) -> Option<f64> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for i in 0..3 {
        if dir[i].abs() < 1e-12 {
            if from[i] < min[i] || from[i] > max[i] {
                return None;
            }
        } else {
            let a = (min[i] - from[i]) / dir[i];
            let b = (max[i] - from[i]) / dir[i];
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(t0)
}

/// How far behind the eye the camera can go, at most `wanted` blocks: like vanilla's third person
/// camera, eight rays from the corners of a small box around the eye, each as long as `wanted`
/// and going the way the camera does, and the camera stops at the nearest block they meet.
/// `hit(from, to)` answers where a ray from one point to another meets a block, if it does.
fn clipped_distance(
    eye: [f64; 3],
    yaw: f32,
    pitch: f32,
    wanted: f32,
    mut hit: impl FnMut([f64; 3], [f64; 3]) -> Option<[f64; 3]>,
) -> f32 {
    let mut distance = wanted;
    for corner in 0..8 {
        let offset = |bit: u32| f64::from(((corner >> bit) & 1) as i32 * 2 - 1) * 0.1;
        let from = [eye[0] + offset(0), eye[1] + offset(1), eye[2] + offset(2)];
        let to = behind(from, yaw, pitch, wanted);
        if let Some(at) = hit(from, to) {
            let d = ((at[0] - eye[0]).powi(2) + (at[1] - eye[1]).powi(2) + (at[2] - eye[2]).powi(2))
                .sqrt() as f32;
            distance = distance.min(d);
        }
    }
    distance
}

/// The yaw and pitch of the entity the view follows (the player, normally).
fn rotation(host: &dyn ClientHost) -> Option<[f32; 2]> {
    let entity = host.camera_entity()?;
    entity.rotation(host)
}

/// Takes the turn the mouse made into the camera and gives the player their frozen angles back.
fn take_turn(host: &mut dyn ClientHost, state: &mut State) {
    let Some(current) = rotation(&*host) else {
        return;
    };
    state.absorb(current);
    if current != state.frozen {
        let _ = host.set_local_rotation(state.frozen[0], state.frozen[1]);
    }
}

/// Freelook goes off: the player's angles and the camera type come back.
fn finish(host: &mut dyn ClientHost, state: &mut State) {
    let _ = host.set_local_rotation(state.frozen[0], state.frozen[1]);
    if let Some(before) = state.before.take() {
        host.set_camera_type(before);
    }
    state.held = false;
}

/// The settings on the mod's Config page (Options > Mods).
fn settings() -> [SettingSpec; 2] {
    [
        SettingSpec::choice(
            MODE_SETTING,
            "freelook.setting.mode",
            ["freelook.mode.hold", "freelook.mode.toggle"],
            0,
        )
        .tooltip("freelook.setting.mode.tooltip"),
        SettingSpec::range(
            DISTANCE_SETTING,
            "freelook.setting.distance",
            f64::from(MIN_DISTANCE),
            f64::from(MAX_DISTANCE),
            1.0,
            f64::from(START_DISTANCE),
        )
        .tooltip("freelook.setting.distance.tooltip"),
    ]
}

#[derive(Default)]
struct Mod {
    state: Mutex<State>,
}

#[mod_api::horizon_mod]
impl Hooks for Mod {
    /// The key, in Controls under the mod's name.
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        r.register_key_mapping(
            KeyRegistration::new(KEY, DEFAULT_KEY)
                .label("key.freelook.freelook")
                .context(KeyConflictContext::InGame),
        );
        for setting in settings() {
            // Only a malformed spec is refused, and a test checks every spec.
            let _ = r.register_setting(setting);
        }
    }

    /// Once a frame, before the mouse turns the view: the key went down or up, or the turn the
    /// last frame's mouse made is taken into the camera.
    fn render_frame_pre(&self, host: &mut dyn ClientHost, _: &mut RenderFramePreEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let playing = host.playing();
        let down = playing && host.is_down(ID, KEY);
        let pressed = down && !state.was_down;
        state.was_down = down;
        let toggle = host.setting_choice(MODE_SETTING) == Some(1);
        // Hold: on while the key is down. Toggle: each press flips it, and a screen switches it off.
        let want = if toggle {
            playing && if pressed { !state.held } else { state.held }
        } else {
            down
        };
        if want && !state.held {
            let Some(current) = rotation(&*host) else {
                return;
            };
            state.held = true;
            state.frozen = current;
            state.look = current;
            state.distance = host
                .setting_f64(DISTANCE_SETTING)
                .map_or(START_DISTANCE, |d| (d as f32).clamp(MIN_DISTANCE, MAX_DISTANCE));
            state.before = Some(host.camera_type());
            host.set_camera_type(CameraType::ThirdPersonBack);
        } else if !want && state.held {
            finish(host, &mut state);
        } else if state.held {
            take_turn(host, &mut state);
        }
    }

    /// Before the tick runs: the player has their frozen angles, so the tick's movement, aim
    /// and move packet are the ones from before the key went down.
    fn client_tick_pre(&self, host: &mut dyn ClientHost, _: &mut ClientTickPreEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.held {
            take_turn(host, &mut state);
        }
    }

    /// The third person distance: none, so that vanilla's camera sits at the eye.
    fn camera_distance(&self, _: &dyn ClientHost, event: &mut CameraDistanceEvent) {
        if self.state.lock().is_ok_and(|s| s.held) {
            event.distance = 0.0;
        }
    }

    /// The camera the world is drawn from: the camera's angles, a distance behind the eye (less when
    /// a block is in the way).
    fn camera_pose(&self, host: &dyn ClientHost, event: &mut CameraPoseEvent) {
        let Ok(state) = self.state.lock() else {
            return;
        };
        if !state.held {
            return;
        }
        let Some(current) = rotation(host) else {
            return;
        };
        let [yaw, pitch] = state.view(current);
        let eye = *event.position;
        // A wall behind the player stops the camera, as it does in vanilla's third person.
        let distance = clipped_distance(eye, yaw, pitch, state.distance, |from, to| {
            ray_hit(from, to, |cell| host.block_collision(cell))
        });
        *event.position = behind(eye, yaw, pitch, distance);
        event.yaw = yaw;
        event.pitch = pitch;
    }

    /// The wheel moves the camera, and not the hotbar, while the key is held.
    fn mouse_scrolling(&self, _: &mut dyn ClientHost, event: &mut MouseScrollingEvent) -> Flow {
        let Ok(mut state) = self.state.lock() else {
            return Flow::Continue;
        };
        if !state.held {
            return Flow::Continue;
        }
        let steps = event.accumulated[1] as f32;
        state.distance = (state.distance - steps).clamp(MIN_DISTANCE, MAX_DISTANCE);
        Flow::Cancel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mouse_turn_goes_to_the_camera() {
        let mut s = State {
            frozen: [30.0, 10.0],
            look: [30.0, 10.0],
            ..State::default()
        };
        assert_eq!(s.view([30.0, 10.0]), [30.0, 10.0]);
        // The mouse turned the player 5 right and 3 down since it was frozen.
        assert_eq!(s.view([35.0, 13.0]), [35.0, 13.0]);
        s.absorb([35.0, 13.0]);
        assert_eq!(s.look, [35.0, 13.0]);
        // Put back at the frozen angles, the next turn adds to it.
        assert_eq!(s.view([32.0, 10.0]), [37.0, 13.0]);
        // The camera's pitch stops at straight up and down.
        assert_eq!(s.view([30.0, 500.0])[1], 90.0);
    }

    /// Every text the mod asks for is in the lang file.
    #[test]
    fn every_text_is_in_the_lang_file() {
        let lang = include_str!("../assets/freelook/lang/en_us.json");
        for setting in settings() {
            assert_eq!(setting.check(), Ok(()));
        }
        let mut keys = vec!["key.freelook.freelook".to_owned()];
        for name in ["mode", "distance"] {
            keys.push(format!("freelook.setting.{name}"));
            keys.push(format!("freelook.setting.{name}.tooltip"));
        }
        keys.push("freelook.mode.hold".to_owned());
        keys.push("freelook.mode.toggle".to_owned());
        for key in keys {
            assert!(lang.contains(&format!("\"{key}\"")), "{key} is not in the lang file");
        }
    }

    /// A full block at the cell `at`, as `block_collision` answers for one.
    fn block_at(at: [i32; 3]) -> impl FnMut([i32; 3]) -> Vec<[f64; 6]> {
        move |cell| {
            if cell == at {
                vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn a_ray_meets_the_first_block() {
        let from = [0.5, 64.5, 0.5];
        // A block at z -3 spans -3 to -2: a ray going north meets its south face.
        let hit = ray_hit(from, [0.5, 64.5, -3.5], block_at([0, 64, -3])).unwrap();
        assert!((hit[2] + 2.0).abs() < 1e-9 && (hit[0] - 0.5).abs() < 1e-9, "{hit:?}");
        // Too short to reach it, or going the other way: no hit.
        assert_eq!(ray_hit(from, [0.5, 64.5, -1.5], block_at([0, 64, -3])), None);
        assert_eq!(ray_hit(from, [0.5, 64.5, 4.5], block_at([0, 64, -3])), None);
        // Diagonal rays walk the cells in order.
        let hit = ray_hit([0.5, 64.5, 0.5], [3.5, 64.5, 3.5], block_at([2, 64, 2])).unwrap();
        assert!((hit[0] - 2.0).abs() < 1e-9 && (hit[2] - 2.0).abs() < 1e-9, "{hit:?}");
        // Starting inside a block is a hit at the start.
        assert_eq!(ray_hit(from, [0.5, 64.5, -3.5], block_at([0, 64, 0])), Some(from));
    }

    #[test]
    fn a_wall_stops_the_camera() {
        let eye = [0.5, 64.5, 0.5];
        let ray = |at: [i32; 3]| move |from, to| ray_hit(from, to, block_at(at));
        // Nothing in the way: the wanted distance.
        assert_eq!(clipped_distance(eye, 0.0, 0.0, 4.0, |_, _| None), 4.0);
        // The camera looks south (yaw 0), so it goes north: a wall at z -3 spans -3 to -2, 2.5
        // from the eye.
        let d = clipped_distance(eye, 0.0, 0.0, 4.0, ray([0, 64, -3]));
        assert!((d - 2.5).abs() < 0.01, "{d}");
        // A wall further than the camera does nothing.
        assert_eq!(clipped_distance(eye, 0.0, 0.0, 4.0, ray([0, 64, -9])), 4.0);
    }

    #[test]
    fn the_camera_sits_behind_the_eye() {
        // Yaw 0 looks south (+z): behind is north.
        let p = behind([0.0, 64.0, 0.0], 0.0, 0.0, 4.0);
        assert!(p[0].abs() < 1e-9 && (p[1] - 64.0).abs() < 1e-9 && (p[2] + 4.0).abs() < 1e-9);
        // Looking straight down puts it straight up.
        let p = behind([0.0, 64.0, 0.0], 0.0, 90.0, 4.0);
        assert!((p[1] - 68.0).abs() < 1e-9);
    }
}
