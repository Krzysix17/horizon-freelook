// While the key is held, the mouse turns the camera's yaw and pitch, not the
// player's: the game's own mouse look runs on the camera's angles and the
// player's are put back after it, with the player model's head pitch and a held
// lead's old look that the turn moves too. Cancels the original; the nested call
// below runs it unchanged (m::begin is false then).
fn freelook_mouse(game: &mut crate::game::Game<'_>) -> std::ops::ControlFlow<()> {
    use crate::game::freelook as m;
    if !game.native_on(m::ID) || !game.native_window() || !m::begin() {
        return std::ops::ControlFlow::Continue(());
    }
    let (yaw, pitch) = (game.yaw, game.pitch);
    let rope = game.rope_rot_old;
    let head = game.player_model.as_ref().map(|p| (p.previous_head_pitch, p.head_pitch));
    let (look_yaw, look_pitch) = m::look(yaw, pitch);
    game.yaw = look_yaw;
    game.pitch = look_pitch;
    game.apply_mouse_look();
    m::set_look(game.yaw, game.pitch);
    game.yaw = yaw;
    game.pitch = pitch;
    game.rope_rot_old = rope;
    if let (Some(p), Some((previous, now))) = (game.player_model.as_mut(), head) {
        p.previous_head_pitch = previous;
        p.head_pitch = now;
    }
    m::end();
    std::ops::ControlFlow::Break(())
}
