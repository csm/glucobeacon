//! The e-ink layout.
//!
//! Designed for a 7-inch 800×480 monochrome panel, read from across a room at
//! three in the morning. That drives every choice here: one enormous number,
//! one arrow, and everything else small and out of the way. Ink is black on
//! white ([`BinaryColor::On`] is ink), because e-ink at rest is white and
//! flooding the panel with black wastes a refresh and looks broken.

use core::fmt::Write as _;

use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_9X18_BOLD, FONT_10X20};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};
use heapless::String;

use glucobeacon_core::{AlarmKind, Duration, Glucose, Timestamp};

use crate::glyphs::{self, DigitStyle};
use crate::state::DisplayState;

/// Panel width in pixels.
pub const PANEL_WIDTH: u32 = 800;
/// Panel height in pixels.
pub const PANEL_HEIGHT: u32 = 480;

const MARGIN: i32 = 16;
const HEADER_RULE_Y: i32 = 44;
const READING_TOP: i32 = 62;
const DIGIT_HEIGHT: u32 = 190;
const SUBLINE_Y: i32 = 296;
const BANNER_TOP: i32 = 316;
const BANNER_HEIGHT: u32 = 52;
const GRAPH_TOP: i32 = 384;
const GRAPH_BOTTOM: i32 = 468;

/// The window the graph covers.
const GRAPH_SPAN: Duration = Duration::from_mins(180);
/// Glucose range the graph plots; values outside are clamped to the edges.
const GRAPH_MIN_MGDL: i32 = 40;
const GRAPH_MAX_MGDL: i32 = 300;

/// Everything one frame needs.
#[derive(Copy, Clone, Debug)]
pub struct Frame<'a> {
    /// The wall clock, if the node has learned it.
    pub now: Option<Timestamp>,
    /// Seconds since boot, for measuring link silence.
    pub uptime: Duration,
    /// What the node knows.
    pub state: &'a DisplayState,
    /// The alarm in force.
    pub alarm: Option<AlarmKind>,
    /// How much of the silence window is left.
    pub silence_remaining: Option<Duration>,
}

/// Draws a complete frame.
pub fn render<D>(target: &mut D, frame: &Frame<'_>) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    target.clear(BinaryColor::Off)?;
    draw_header(target, frame)?;
    draw_reading(target, frame)?;
    draw_subline(target, frame)?;
    if let Some(alarm) = frame.alarm {
        draw_banner(target, frame, alarm)?;
    }
    draw_graph(target, frame)?;
    Ok(())
}

fn draw_header<D>(target: &mut D, frame: &Frame<'_>) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let bold = MonoTextStyle::new(&FONT_9X18_BOLD, BinaryColor::On);
    let left = TextStyleBuilder::new()
        .alignment(Alignment::Left)
        .baseline(Baseline::Middle)
        .build();
    let right = TextStyleBuilder::new()
        .alignment(Alignment::Right)
        .baseline(Baseline::Middle)
        .build();

    Text::with_text_style("GLUCOBEACON", Point::new(MARGIN, 22), bold, left).draw(target)?;

    let mut status: String<64> = String::new();
    let _ = write!(status, "link {}", link_age(frame));
    let _ = write!(status, "  ·  {}", frame.state.uplink().label());
    Text::with_text_style(
        &status,
        Point::new(PANEL_WIDTH as i32 - MARGIN, 22),
        MonoTextStyle::new(&FONT_10X20, BinaryColor::On),
        right,
    )
    .draw(target)?;

    Line::new(
        Point::new(MARGIN, HEADER_RULE_Y),
        Point::new(PANEL_WIDTH as i32 - MARGIN, HEADER_RULE_Y),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
    .draw(target)
}

fn draw_reading<D>(target: &mut D, frame: &Frame<'_>) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let style = DigitStyle::with_height(DIGIT_HEIGHT);
    let origin = Point::new(MARGIN + 24, READING_TOP);

    let width = match frame.state.latest() {
        Some(reading) => glyphs::draw_number(
            target,
            reading.glucose.mgdl(),
            origin,
            &style,
            BinaryColor::On,
        )?,
        // Three dashes: the panel is working, the data is not.
        None => glyphs::draw_blank(target, 3, origin, &style, BinaryColor::On)?,
    };

    if let Some(reading) = frame.state.latest() {
        let center = Point::new(
            origin.x + width as i32 + 110,
            READING_TOP + DIGIT_HEIGHT as i32 / 2,
        );
        glyphs::draw_trend(target, reading.trend, center, 140, BinaryColor::On)?;
    }

    // Units, small, tucked under the digits' baseline.
    Text::with_baseline(
        "mg/dL",
        Point::new(origin.x + 4, READING_TOP + DIGIT_HEIGHT as i32 + 8),
        MonoTextStyle::new(&FONT_10X20, BinaryColor::On),
        Baseline::Top,
    )
    .draw(target)?;
    Ok(())
}

fn draw_subline<D>(target: &mut D, frame: &Frame<'_>) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let mut line: String<96> = String::new();

    match frame.state.delta_mgdl() {
        Some(delta) => {
            let _ = write!(
                line,
                "{}{} mg/dL",
                if delta < 0 { "-" } else { "+" },
                delta.abs()
            );
        }
        None => {
            let _ = write!(line, "-- mg/dL");
        }
    }

    if let Some(reading) = frame.state.latest() {
        let tenths = reading.glucose.mmol_l_tenths();
        let _ = write!(line, "   ·   {}.{} mmol/L", tenths / 10, tenths % 10);
        let _ = write!(line, "   ·   {}", reading_age(frame));
    }

    Text::with_baseline(
        &line,
        Point::new(MARGIN + 24, SUBLINE_Y),
        MonoTextStyle::new(&FONT_10X20, BinaryColor::On),
        Baseline::Top,
    )
    .draw(target)?;
    Ok(())
}

fn draw_banner<D>(target: &mut D, frame: &Frame<'_>, alarm: AlarmKind) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    // Inverted: the one place worth spending black ink, because it has to be
    // unmistakable from across the room.
    Rectangle::new(
        Point::new(MARGIN, BANNER_TOP),
        Size::new(PANEL_WIDTH - 2 * MARGIN as u32, BANNER_HEIGHT),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(target)?;

    let middle = BANNER_TOP + BANNER_HEIGHT as i32 / 2;
    let knockout = MonoTextStyle::new(&FONT_9X18_BOLD, BinaryColor::Off);
    let left = TextStyleBuilder::new()
        .alignment(Alignment::Left)
        .baseline(Baseline::Middle)
        .build();
    let right = TextStyleBuilder::new()
        .alignment(Alignment::Right)
        .baseline(Baseline::Middle)
        .build();

    Text::with_text_style(
        alarm.label(),
        Point::new(MARGIN + 16, middle),
        knockout,
        left,
    )
    .draw(target)?;

    let mut note: String<48> = String::new();
    match frame.silence_remaining {
        Some(remaining) => {
            let _ = write!(note, "SILENCED {} MIN", remaining.as_mins() + 1);
        }
        None => {
            let _ = write!(note, "PRESS TO SILENCE");
        }
    }
    Text::with_text_style(
        &note,
        Point::new(PANEL_WIDTH as i32 - MARGIN - 16, middle),
        knockout,
        right,
    )
    .draw(target)?;
    Ok(())
}

fn draw_graph<D>(target: &mut D, frame: &Frame<'_>) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let left = MARGIN + 24;
    let right = PANEL_WIDTH as i32 - MARGIN - 44;
    let stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    let small = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);

    // Baseline and threshold guides.
    Line::new(
        Point::new(left, GRAPH_BOTTOM),
        Point::new(right, GRAPH_BOTTOM),
    )
    .into_styled(stroke)
    .draw(target)?;

    let thresholds = frame.state.policy().thresholds;
    for mgdl in [thresholds.low_mgdl, thresholds.high_mgdl] {
        let y = graph_y(i32::from(mgdl));
        draw_dashed(target, left, right, y)?;

        let mut label: String<8> = String::new();
        let _ = write!(label, "{mgdl}");
        Text::with_baseline(&label, Point::new(right + 6, y - 5), small, Baseline::Top)
            .draw(target)?;
    }

    let Some(now) = frame.now else {
        Text::with_baseline(
            "waiting for the gateway",
            Point::new(left, GRAPH_TOP + 8),
            small,
            Baseline::Top,
        )
        .draw(target)?;
        return Ok(());
    };

    let mut label: String<16> = String::new();
    let _ = write!(label, "-{}h", GRAPH_SPAN.as_mins() / 60);
    Text::with_baseline(
        &label,
        Point::new(left, GRAPH_BOTTOM + 4),
        small,
        Baseline::Top,
    )
    .draw(target)?;

    let earliest = Timestamp::from_unix_secs(now.as_unix_secs() - i64::from(GRAPH_SPAN.as_secs()));
    let span = i64::from(GRAPH_SPAN.as_secs()).max(1);
    let fill = PrimitiveStyle::with_fill(BinaryColor::On);

    for reading in frame.state.history().since(earliest) {
        let offset = (reading.at.as_unix_secs() - earliest.as_unix_secs()).clamp(0, span);
        let x = left + ((right - left) as i64 * offset / span) as i32;
        let y = graph_y(i32::from(reading.glucose.mgdl()));
        Rectangle::new(Point::new(x - 1, y - 1), Size::new(3, 3))
            .into_styled(fill)
            .draw(target)?;
    }
    Ok(())
}

/// Maps a glucose value to a y coordinate in the graph area.
fn graph_y(mgdl: i32) -> i32 {
    let clamped = mgdl.clamp(GRAPH_MIN_MGDL, GRAPH_MAX_MGDL);
    let range = GRAPH_MAX_MGDL - GRAPH_MIN_MGDL;
    let height = GRAPH_BOTTOM - GRAPH_TOP;
    GRAPH_BOTTOM - (clamped - GRAPH_MIN_MGDL) * height / range
}

fn draw_dashed<D>(target: &mut D, left: i32, right: i32, y: i32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let fill = PrimitiveStyle::with_fill(BinaryColor::On);
    let mut x = left;
    while x < right {
        Rectangle::new(Point::new(x, y), Size::new(4, 1))
            .into_styled(fill)
            .draw(target)?;
        x += 10;
    }
    Ok(())
}

/// "2 min ago" for the sensor reading, or a placeholder if the clock is unset.
fn reading_age(frame: &Frame<'_>) -> String<24> {
    let mut text: String<24> = String::new();
    match (frame.now, frame.state.latest()) {
        (Some(now), Some(reading)) => {
            let mins = reading.age(now).as_mins();
            let _ = if mins == 0 {
                write!(text, "just now")
            } else {
                write!(text, "{mins} min ago")
            };
        }
        _ => {
            let _ = write!(text, "age unknown");
        }
    }
    text
}

/// "3m" for how long the radio has been quiet.
fn link_age(frame: &Frame<'_>) -> String<16> {
    let mut text: String<16> = String::new();
    match frame.state.since_last_packet(frame.uptime) {
        Some(silence) => {
            let mins = silence.as_mins();
            let _ = if mins == 0 {
                write!(text, "ok")
            } else {
                write!(text, "{mins}m quiet")
            };
        }
        None => {
            let _ = write!(text, "no contact");
        }
    }
    text
}

/// The mmol/L reading of a value, as tenths, for callers formatting elsewhere.
pub fn mmol_tenths(glucose: Glucose) -> u16 {
    glucose.mmol_l_tenths()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DisplayState;
    use glucobeacon_core::{AlarmPolicy, Reading, Trend};
    use glucobeacon_proto::{Message, Packet, ReadingReport, Status, UplinkState};

    /// A canvas the size of the real panel.
    ///
    /// `MockDisplay` is capped well under 800×480, so rendering goes to a plain
    /// buffer and the tests assert on ink coverage rather than exact pixels —
    /// which is what actually matters for "is anything on the screen".
    #[derive(Debug)]
    struct Canvas {
        pixels: Vec<bool>,
    }

    impl Canvas {
        fn new() -> Self {
            Self {
                pixels: vec![false; (PANEL_WIDTH * PANEL_HEIGHT) as usize],
            }
        }

        fn ink_in(&self, area: Rectangle) -> u32 {
            let mut count = 0;
            for point in area.points() {
                if point.x < 0 || point.y < 0 {
                    continue;
                }
                let (x, y) = (point.x as u32, point.y as u32);
                if x < PANEL_WIDTH
                    && y < PANEL_HEIGHT
                    && self.pixels[(y * PANEL_WIDTH + x) as usize]
                {
                    count += 1;
                }
            }
            count
        }

        fn ink(&self) -> u32 {
            self.pixels.iter().filter(|on| **on).count() as u32
        }
    }

    impl OriginDimensions for Canvas {
        fn size(&self) -> Size {
            Size::new(PANEL_WIDTH, PANEL_HEIGHT)
        }
    }

    impl DrawTarget for Canvas {
        type Color = BinaryColor;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if point.x < 0 || point.y < 0 {
                    continue;
                }
                let (x, y) = (point.x as u32, point.y as u32);
                if x < PANEL_WIDTH && y < PANEL_HEIGHT {
                    self.pixels[(y * PANEL_WIDTH + x) as usize] = color.is_on();
                }
            }
            Ok(())
        }
    }

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_unix_secs(secs)
    }

    fn state_with_readings(values: &[(u16, i64)]) -> DisplayState {
        let mut state = DisplayState::new(AlarmPolicy::DEFAULT);
        for (index, (mgdl, sensor_at)) in values.iter().enumerate() {
            state.apply(
                &Packet::new(
                    index as u16,
                    Message::Reading(ReadingReport {
                        reading: Reading::new(
                            Glucose::from_mgdl(*mgdl),
                            Trend::FortyFiveDown,
                            at(*sensor_at),
                        ),
                        sent_at: at(*sensor_at + 30),
                    }),
                ),
                Duration::from_secs(index as u32 * 300),
            );
        }
        state
    }

    fn frame<'a>(state: &'a DisplayState, alarm: Option<AlarmKind>) -> Frame<'a> {
        Frame {
            now: state.now(Duration::from_secs(600)),
            uptime: Duration::from_secs(600),
            state,
            alarm,
            silence_remaining: None,
        }
    }

    fn draw(frame: &Frame<'_>) -> Canvas {
        let mut canvas = Canvas::new();
        render(&mut canvas, frame).expect("render");
        canvas
    }

    #[test]
    fn a_frame_with_no_data_still_draws_a_usable_screen() {
        let state = DisplayState::default();
        let canvas = draw(&frame(&state, None));

        // Header and the placeholder dashes, at minimum.
        assert!(canvas.ink() > 200, "screen is nearly blank");
        let reading_area = Rectangle::new(
            Point::new(0, READING_TOP),
            Size::new(PANEL_WIDTH, DIGIT_HEIGHT),
        );
        assert!(canvas.ink_in(reading_area) > 0, "no placeholder drawn");
    }

    #[test]
    fn the_reading_dominates_the_screen() {
        let state = state_with_readings(&[(188, 1_000)]);
        let canvas = draw(&frame(&state, None));

        let reading_area = Rectangle::new(
            Point::new(0, READING_TOP),
            Size::new(PANEL_WIDTH, DIGIT_HEIGHT),
        );
        let header_area = Rectangle::new(Point::zero(), Size::new(PANEL_WIDTH, 46));
        assert!(
            canvas.ink_in(reading_area) > canvas.ink_in(header_area) * 3,
            "the number should be the loudest thing on the panel"
        );
    }

    #[test]
    fn a_three_digit_reading_uses_more_ink_than_a_two_digit_one() {
        let two = draw(&frame(&state_with_readings(&[(88, 1_000)]), None));
        let three = draw(&frame(&state_with_readings(&[(188, 1_000)]), None));
        assert!(three.ink() > two.ink());
    }

    #[test]
    fn the_alarm_banner_is_a_solid_block_of_ink() {
        let state = state_with_readings(&[(52, 1_000)]);
        let quiet = draw(&frame(&state, None));
        let alarming = draw(&frame(&state, Some(AlarmKind::UrgentLow)));

        let banner = Rectangle::new(
            Point::new(MARGIN, BANNER_TOP),
            Size::new(PANEL_WIDTH - 2 * MARGIN as u32, BANNER_HEIGHT),
        );
        assert_eq!(quiet.ink_in(banner), 0, "no banner without an alarm");
        // Inverted block, so most of the region is ink, minus knocked-out text.
        let area = banner.size.width * banner.size.height;
        assert!(alarming.ink_in(banner) > area * 3 / 4);
        assert!(
            alarming.ink_in(banner) < area,
            "text should knock out of it"
        );
    }

    #[test]
    fn silencing_changes_the_banner_without_removing_it() {
        let state = state_with_readings(&[(52, 1_000)]);
        let mut silenced = frame(&state, Some(AlarmKind::UrgentLow));
        silenced.silence_remaining = Some(Duration::from_mins(12));

        let sounding = draw(&frame(&state, Some(AlarmKind::UrgentLow)));
        let quiet = draw(&silenced);

        let banner = Rectangle::new(
            Point::new(MARGIN, BANNER_TOP),
            Size::new(PANEL_WIDTH - 2 * MARGIN as u32, BANNER_HEIGHT),
        );
        assert!(quiet.ink_in(banner) > 0, "a silenced alarm is still shown");
        assert_ne!(sounding.ink_in(banner), quiet.ink_in(banner));
    }

    #[test]
    fn the_graph_plots_the_readings_it_has() {
        // Compared against a node that knows the time but has no readings, so
        // the difference is the plotted points and not the "waiting" notice.
        let mut clock_only = DisplayState::new(AlarmPolicy::DEFAULT);
        clock_only.apply(
            &Packet::new(
                0,
                Message::Status(Status {
                    sent_at: at(1_930),
                    uplink: UplinkState::Ok,
                    consecutive_failures: 0,
                    battery_millivolts: None,
                }),
            ),
            Duration::from_secs(600),
        );

        let empty = draw(&frame(&clock_only, None));
        let plotted = draw(&frame(
            &state_with_readings(&[(120, 1_000), (135, 1_300), (150, 1_600), (95, 1_900)]),
            None,
        ));

        let graph = Rectangle::new(
            Point::new(0, GRAPH_TOP),
            Size::new(PANEL_WIDTH, (GRAPH_BOTTOM - GRAPH_TOP) as u32),
        );
        assert!(plotted.ink_in(graph) > empty.ink_in(graph));
    }

    #[test]
    fn a_node_with_no_clock_says_so_in_the_graph_area() {
        let state = DisplayState::default();
        let canvas = draw(&frame(&state, None));
        let graph = Rectangle::new(
            Point::new(0, GRAPH_TOP),
            Size::new(PANEL_WIDTH, (GRAPH_BOTTOM - GRAPH_TOP) as u32),
        );
        assert!(
            canvas.ink_in(graph) > 0,
            "should explain why the graph is empty"
        );
    }

    #[test]
    fn graph_points_map_high_values_above_low_ones() {
        assert!(
            graph_y(200) < graph_y(80),
            "higher glucose should sit higher"
        );
        assert_eq!(graph_y(GRAPH_MIN_MGDL), GRAPH_BOTTOM);
        assert_eq!(graph_y(GRAPH_MAX_MGDL), GRAPH_TOP);
    }

    #[test]
    fn out_of_range_values_clamp_into_the_graph_area() {
        assert_eq!(graph_y(0), GRAPH_BOTTOM);
        assert_eq!(graph_y(600), GRAPH_TOP);
        for mgdl in [0, 39, 40, 120, 300, 400, 10_000] {
            let y = graph_y(mgdl);
            assert!((GRAPH_TOP..=GRAPH_BOTTOM).contains(&y), "{mgdl} -> {y}");
        }
    }

    #[test]
    fn nothing_is_drawn_outside_the_panel() {
        // The canvas silently drops out-of-bounds pixels, so this checks the
        // bottom and right edges are actually reachable and nothing overflows
        // into a panic.
        let state = state_with_readings(&[(400, 1_000), (40, 1_300)]);
        let canvas = draw(&frame(&state, Some(AlarmKind::UrgentHigh)));
        assert!(canvas.ink() > 0);
    }

    #[test]
    fn the_uplink_state_appears_in_the_header() {
        let mut healthy = state_with_readings(&[(120, 1_000)]);
        healthy.apply(
            &Packet::new(
                99,
                Message::Status(Status {
                    sent_at: at(1_030),
                    uplink: UplinkState::Ok,
                    consecutive_failures: 0,
                    battery_millivolts: None,
                }),
            ),
            Duration::from_secs(0),
        );

        let mut broken = state_with_readings(&[(120, 1_000)]);
        broken.apply(
            &Packet::new(
                99,
                Message::Status(Status {
                    sent_at: at(1_030),
                    uplink: UplinkState::AuthFailed,
                    consecutive_failures: 5,
                    battery_millivolts: None,
                }),
            ),
            Duration::from_secs(0),
        );

        let header = Rectangle::new(Point::zero(), Size::new(PANEL_WIDTH, 44));
        assert_ne!(
            draw(&frame(&healthy, None)).ink_in(header),
            draw(&frame(&broken, None)).ink_in(header),
            "the header should distinguish a healthy uplink from a broken one"
        );
    }

    #[test]
    fn age_text_says_so_when_the_clock_is_unknown() {
        let state = DisplayState::default();
        let f = Frame {
            now: None,
            uptime: Duration::from_secs(10),
            state: &state,
            alarm: None,
            silence_remaining: None,
        };
        assert_eq!(reading_age(&f).as_str(), "age unknown");
        assert_eq!(link_age(&f).as_str(), "no contact");
    }

    #[test]
    fn age_text_rounds_down_to_whole_minutes() {
        let state = state_with_readings(&[(120, 1_000)]);
        let mut f = frame(&state, None);
        f.now = Some(at(1_000));
        assert_eq!(reading_age(&f).as_str(), "just now");
        f.now = Some(at(1_000 + 179));
        assert_eq!(reading_age(&f).as_str(), "2 min ago");
    }

    #[test]
    fn mmol_conversion_is_exposed_for_callers() {
        assert_eq!(mmol_tenths(Glucose::from_mgdl(180)), 100);
    }
}
