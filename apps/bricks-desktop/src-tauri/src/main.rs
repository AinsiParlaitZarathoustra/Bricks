// Prevents an additional console window on Windows, debug builds included
// (ignored on other platforms).
#![windows_subsystem = "windows"]

fn main() {
    bricks_desktop_lib::run()
}
