//! Runs the display firmware's board constants through their own tests.
//!
//! `firmware/glucobeacon-fw-display` builds only for Xtensa, so `cargo test`
//! can never reach it. Its `board` module is pure constants and arithmetic
//! though, and the things it asserts — that no two peripherals share a pin,
//! that nothing lands on the soldered-down radio, that every frame the node
//! sends fits the region's dwell limit — are exactly the mistakes worth
//! catching before a board is on the bench rather than after.
//!
//! So the module is pulled in here by path and its tests run on the host, as
//! part of the ordinary workspace test run.

// The firmware uses every constant in here; this harness only reads some of
// them, so dead-code analysis has the wrong end of the stick.
#[allow(dead_code)]
#[path = "../../../firmware/glucobeacon-fw-display/src/board.rs"]
mod board;

/// The firmware's own assertions live in `board`'s test module and run with
/// this file. This one checks the panel the board is wired for is the panel
/// the layout is drawn for.
#[test]
fn the_board_targets_the_panel_the_ui_is_drawn_for() {
    // Waveshare 7.5" HAT, 800x480 black/white, per wiring diagram v1.0.
    assert_eq!(glucobeacon_display::PANEL_WIDTH, 800);
    assert_eq!(glucobeacon_display::PANEL_HEIGHT, 480);
    assert_eq!(glucobeacon_display::PANEL_BYTES, 48_000);
}
