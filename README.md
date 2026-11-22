# Freelook for Horizon

A native mod for [Horizon](https://playhorizon.dev), not a Fabric or Java mod.

Hold a key (X by default; rebind it in Options > Controls > Key Binds, under Freelook) to switch to a third person view, like F5. While you hold it, the mouse turns only the camera: your character keeps walking and aiming the way it was facing. Scroll to change the camera distance, from 1 to 24 blocks. When you let go, the camera goes back to the view you had before.

The key is changed in Options > Controls > Key Binds, under Freelook (X by default). Options > Mods > Freelook > Config has two settings: Mode (Hold, the default, or Toggle: press once to switch freelook on and again to switch it off) and Distance (the camera's starting distance in blocks).

## What it does and does not do

It uses only the mod API (`render_frame_pre`, `client_tick_pre`, `camera_distance`, `camera_pose`, `mouse_scrolling` and one registered key). It touches no game internals, reads and writes no files, uses no network and starts no processes. While the key is held it puts the player's own view angles back every frame and tick, so the server sees the angles from before the key went down.

The camera does not stop at walls: with a block behind you it can go inside it.

MIT licensed.
