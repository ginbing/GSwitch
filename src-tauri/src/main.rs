// Tauri applications are GUI applications on Windows.  Without this attribute
// Windows allocates a visible console for every packaged launch.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    gswitch_lib::run();
}
