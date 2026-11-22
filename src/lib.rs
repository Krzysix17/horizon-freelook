//! Freelook: hold a key (X by default; change it in Options > Controls > Key Binds) for a third person view you can
//! look around in. The mouse turns only the camera: the player keeps walking, aiming and facing
//! the way they did when the key went down. Scroll while holding it to change the camera's
//! distance. When the key is let go, the view goes back to what it was.
//!
//! How it works, with the mod API only (no game internals):
//! - The mouse still turns the player a little each frame. `render_frame_pre` and
//!   `client_tick_pre` read that turn, add it to the camera's own angles and put the player's
//!   angles back (`set_local_rotation`), so every tick and every packet sees the frozen ones.
//! - `camera_pose` draws the world from the camera's angles, a distance behind the eye.
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

    /// The camera the world is drawn from: the camera's angles, a distance behind the eye.
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
        *event.position = behind(eye, yaw, pitch, state.distance);
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
