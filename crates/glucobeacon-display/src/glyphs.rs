//! Oversized glyphs drawn from primitives.
//!
//! The glucose value has to be readable across a room, which means digits on
//! the order of 150 pixels tall. Bitmap fonts that size are large, and a font
//! rasterizer is a dependency and a pile of flash this node does not have to
//! spare — but seven-segment digits are just seven polygons, and polygons scale
//! for free. Same reasoning for the trend arrow.
//!
//! The segments are elongated hexagons — a full-thickness body with both ends
//! mitred to a point — separated by a hairline gap, which is what a moulded LCD
//! digit looks like. Plain rectangles fuse into each other at the corners; the
//! mitres and the gap keep every segment's outline distinct, so a `0` cannot be
//! misread as an `8` from across the room.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Primitive, PrimitiveStyle, Rectangle, Triangle};

use glucobeacon_core::Trend;

/// Which of the seven segments each digit lights, as a bitmask over
/// `a b c d e f g` from bit 0 to bit 6.
const SEGMENTS: [u8; 10] = [
    0b0111111, // 0: a b c d e f
    0b0000110, // 1: b c
    0b1011011, // 2: a b d e g
    0b1001111, // 3: a b c d g
    0b1100110, // 4: b c f g
    0b1101101, // 5: a c d f g
    0b1111101, // 6: a c d e f g
    0b0000111, // 7: a b c
    0b1111111, // 8: all
    0b1101111, // 9: a b c d f g
];

/// The size of a seven-segment digit.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DigitStyle {
    /// Total width of one digit, in pixels.
    pub width: u32,
    /// Total height of one digit, in pixels.
    pub height: u32,
    /// Segment thickness, in pixels.
    pub thickness: u32,
    /// Gap between adjacent digits, in pixels.
    pub spacing: u32,
    /// Gap between neighbouring segments of one digit, in pixels.
    ///
    /// Trimmed off each end of every segment, so two segments whose points
    /// converge end up twice this far apart.
    pub gap: u32,
}

impl DigitStyle {
    /// A style scaled to `height`, with proportions that read well on e-ink.
    pub const fn with_height(height: u32) -> Self {
        let thickness = height / 10;
        Self {
            width: height / 2,
            height,
            thickness,
            spacing: height / 12,
            // A seam, not a space: converging points sit twice this far
            // apart, and a `1` stops reading as one stroke well before the
            // seam gets wide. Never zero, or the mitres fuse into a corner.
            gap: if thickness < 8 { 1 } else { thickness / 8 },
        }
    }

    /// Total width of a `count`-digit number in this style.
    pub const fn width_of(&self, count: u32) -> u32 {
        if count == 0 {
            0
        } else {
            count * self.width + (count - 1) * self.spacing
        }
    }
}

/// Draws a decimal number in seven-segment digits, left-aligned at `origin`.
///
/// Returns the width drawn, so callers can lay out what follows.
pub fn draw_number<D>(
    target: &mut D,
    value: u16,
    origin: Point,
    style: &DigitStyle,
    color: D::Color,
) -> Result<u32, D::Error>
where
    D: DrawTarget,
{
    let mut digits = [0u8; 5];
    let mut count = 0;
    let mut remaining = value;
    loop {
        digits[count] = (remaining % 10) as u8;
        count += 1;
        remaining /= 10;
        if remaining == 0 || count == digits.len() {
            break;
        }
    }

    let mut x = origin.x;
    for index in (0..count).rev() {
        draw_digit(target, digits[index], Point::new(x, origin.y), style, color)?;
        x += (style.width + style.spacing) as i32;
    }
    Ok(style.width_of(count as u32))
}

/// Draws placeholder dashes where a number would go, for "no reading".
pub fn draw_blank<D>(
    target: &mut D,
    count: u32,
    origin: Point,
    style: &DigitStyle,
    color: D::Color,
) -> Result<u32, D::Error>
where
    D: DrawTarget,
{
    // Just the middle segment: an unmistakable "nothing here".
    let (offset, size) = segments(style)[MIDDLE];
    for index in 0..count {
        let x = origin.x + (index * (style.width + style.spacing)) as i32;
        draw_segment(target, Point::new(x, origin.y) + offset, size, color)?;
    }
    Ok(style.width_of(count))
}

/// Where the middle segment (`g`) sits in [`segments`].
const MIDDLE: usize = 6;

/// The seven segment boxes in `a b c d e f g` order, as offsets from a digit's
/// top-left corner paired with the extent each one fills.
///
/// A mitre puts two segments' points nose to nose rather than butting one
/// segment's point against the other's flank, which is what sets the reach of
/// everything here. The verticals run all the way to the digit's waist, so at
/// the middle of each side three points converge on one spot: the upper
/// vertical's, the lower vertical's, and the middle bar's. A `1` is the case
/// that shows whether this is right — its two lit segments are verticals with
/// nothing between them, so their points have to nearly touch.
///
/// Paying for that costs the middle bar half a stroke at each end: if it ran
/// the full width like the top and bottom bars do, the verticals would have to
/// stop against its body corners, a whole stroke apart. Real moulded digits
/// make the same trade, which is why their middle bar is the short one.
///
/// Every box is then trimmed by [`DigitStyle::gap`] at both ends to open the
/// seam, so a converging pair ends up `2 * gap` apart.
fn segments(style: &DigitStyle) -> [(Point, Size); 7] {
    let t = style.thickness;
    let w = style.width;
    let h = style.height;
    let g = style.gap;

    // The waist: where the verticals stop and the middle bar is centred.
    let waist = h / 2;
    let upper = waist.saturating_sub(t);
    let lower = (h - t).saturating_sub(waist);

    // The top and bottom bars run the full width, points and all — the
    // verticals tuck inside their mitres rather than being displaced by them.
    let bar = Size::new(w.saturating_sub(2 * g), t);
    let arm = |length: u32| Size::new(t, length.saturating_sub(2 * g));
    let x_right = (w - t) as i32;
    let y_upper = (t + g) as i32;
    let y_lower = (waist + g) as i32;

    [
        (Point::new(g as i32, 0), bar),              // a: top
        (Point::new(x_right, y_upper), arm(upper)),  // b: upper right
        (Point::new(x_right, y_lower), arm(lower)),  // c: lower right
        (Point::new(g as i32, (h - t) as i32), bar), // d: bottom
        (Point::new(0, y_lower), arm(lower)),        // e: lower left
        (Point::new(0, y_upper), arm(upper)),        // f: upper left
        // g: middle, pulled in half a stroke so its points meet the verticals'.
        (
            Point::new((t / 2 + g) as i32, (waist - t / 2) as i32),
            Size::new(w.saturating_sub(t + 2 * g), t),
        ),
    ]
}

/// Draws one segment as an elongated hexagon filling `size` from `origin`.
///
/// The long axis is whichever of `size` is larger; both of its ends taper to a
/// point at 45°. `embedded-graphics` fills rectangles and triangles but not
/// arbitrary polygons, and butting a triangle against a rectangle leaves a seam
/// where the two rasterizers disagree — so the hexagon goes down as slices of
/// the long axis, one pixel each, shortened as they approach the outer face.
fn draw_segment<D>(
    target: &mut D,
    origin: Point,
    size: Size,
    color: D::Color,
) -> Result<(), D::Error>
where
    D: DrawTarget,
{
    let horizontal = size.width >= size.height;
    let (length, thickness) = if horizontal {
        (size.width, size.height)
    } else {
        (size.height, size.width)
    };

    let fill = PrimitiveStyle::with_fill(color);
    for slice in 0..thickness {
        // How far this slice is from the segment's spine, which is exactly how
        // far the 45° mitre pulls both of its ends in.
        let inset = ((2 * slice + 1) as i32 - thickness as i32).unsigned_abs() / 2;
        let run = length.saturating_sub(2 * inset);
        if run == 0 {
            continue;
        }
        let (offset, size) = if horizontal {
            (Point::new(inset as i32, slice as i32), Size::new(run, 1))
        } else {
            (Point::new(slice as i32, inset as i32), Size::new(1, run))
        };
        Rectangle::new(origin + offset, size)
            .into_styled(fill)
            .draw(target)?;
    }
    Ok(())
}

/// Draws one seven-segment digit with its top-left corner at `origin`.
pub fn draw_digit<D>(
    target: &mut D,
    digit: u8,
    origin: Point,
    style: &DigitStyle,
    color: D::Color,
) -> Result<(), D::Error>
where
    D: DrawTarget,
{
    let Some(mask) = SEGMENTS.get(digit as usize).copied() else {
        return Ok(());
    };

    for (index, (offset, size)) in segments(style).into_iter().enumerate() {
        if mask & (1 << index) != 0 {
            draw_segment(target, origin + offset, size, color)?;
        }
    }
    Ok(())
}

/// Draws the trend arrow centred on `center`.
///
/// Trends with no direction ([`Trend::None`], [`Trend::NotComputable`],
/// [`Trend::RateOutOfRange`]) draw nothing — an arrow pointing nowhere is worse
/// than no arrow.
pub fn draw_trend<D>(
    target: &mut D,
    trend: Trend,
    center: Point,
    size: u32,
    color: D::Color,
) -> Result<(), D::Error>
where
    D: DrawTarget,
{
    let Some(degrees) = trend.arrow_degrees() else {
        return Ok(());
    };
    let direction = unit_vector(degrees);

    if trend.is_rapid() {
        // Two stacked arrows, offset along the perpendicular.
        let offset = (size / 3) as i32;
        let perpendicular = Point::new(-direction.y, direction.x);
        let shift = scale(perpendicular, offset, 1000);
        draw_arrow(target, center - shift, direction, size * 3 / 4, color)?;
        draw_arrow(target, center + shift, direction, size * 3 / 4, color)?;
        Ok(())
    } else {
        draw_arrow(target, center, direction, size, color)
    }
}

/// A direction vector scaled by 1000, for the five angles arrows use.
///
/// Fixed point rather than trigonometry: five directions do not justify pulling
/// `sin` onto a target that has no FPU.
fn unit_vector(degrees: i16) -> Point {
    match degrees {
        d if d <= -90 => Point::new(0, -1000),
        -45 => Point::new(707, -707),
        45 => Point::new(707, 707),
        d if d >= 90 => Point::new(0, 1000),
        _ => Point::new(1000, 0),
    }
}

fn scale(vector: Point, numerator: i32, denominator: i32) -> Point {
    Point::new(
        vector.x * numerator / denominator,
        vector.y * numerator / denominator,
    )
}

fn draw_arrow<D>(
    target: &mut D,
    center: Point,
    direction: Point,
    size: u32,
    color: D::Color,
) -> Result<(), D::Error>
where
    D: DrawTarget,
{
    let half = (size / 2) as i32;
    let head = (size / 2) as i32;
    let half_head = (size / 3) as i32;
    let shaft = (size / 6).max(2) as i32;

    let tip = center + scale(direction, half, 1000);
    let tail = center - scale(direction, half, 1000);
    let neck = tip - scale(direction, head, 1000);
    let perpendicular = Point::new(-direction.y, direction.x);
    let wing = scale(perpendicular, half_head, 1000);
    let shaft_offset = scale(perpendicular, shaft / 2, 1000);

    Triangle::new(tip, neck + wing, neck - wing)
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(target)?;

    // The shaft as a filled quad, drawn as two triangles: `Rectangle` cannot be
    // rotated, and a stroked `Line` does not thicken symmetrically at 45°.
    let a = tail + shaft_offset;
    let b = tail - shaft_offset;
    let c = neck + shaft_offset;
    let d = neck - shaft_offset;
    let fill = PrimitiveStyle::with_fill(color);
    Triangle::new(a, b, c).into_styled(fill).draw(target)?;
    Triangle::new(b, c, d).into_styled(fill).draw(target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::mock_display::MockDisplay;
    use embedded_graphics::pixelcolor::BinaryColor;

    /// Counts set pixels, which is enough to tell "drew something sensible"
    /// from "drew nothing" or "drew everything".
    fn ink(display: &MockDisplay<BinaryColor>, area: Rectangle) -> u32 {
        let mut count = 0;
        for point in area.points() {
            if display.get_pixel(point) == Some(BinaryColor::On) {
                count += 1;
            }
        }
        count
    }

    fn canvas() -> MockDisplay<BinaryColor> {
        let mut display = MockDisplay::new();
        // The layout intentionally draws overlapping primitives (arrow shaft
        // into arrow head), which is not a bug.
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);
        display
    }

    #[test]
    fn every_digit_draws_and_they_differ() {
        let style = DigitStyle::with_height(40);
        let area = Rectangle::new(Point::zero(), Size::new(style.width, style.height));

        let mut counts = Vec::new();
        for digit in 0..10u8 {
            let mut display = canvas();
            draw_digit(&mut display, digit, Point::zero(), &style, BinaryColor::On).expect("draw");
            counts.push(ink(&display, area));
        }

        // 1 lights the fewest segments, 8 lights all of them.
        assert_eq!(
            counts
                .iter()
                .enumerate()
                .min_by_key(|(_, c)| **c)
                .map(|(i, _)| i),
            Some(1)
        );
        assert_eq!(
            counts
                .iter()
                .enumerate()
                .max_by_key(|(_, c)| **c)
                .map(|(i, _)| i),
            Some(8)
        );
        assert!(counts.iter().all(|c| *c > 0));
    }

    #[test]
    fn an_out_of_range_digit_draws_nothing_rather_than_panicking() {
        let style = DigitStyle::with_height(40);
        let mut display = canvas();
        draw_digit(&mut display, 10, Point::zero(), &style, BinaryColor::On).expect("draw");
        let area = Rectangle::new(Point::zero(), Size::new(style.width, style.height));
        assert_eq!(ink(&display, area), 0);
    }

    #[test]
    fn numbers_report_the_width_they_occupy() {
        let style = DigitStyle::with_height(40);
        let mut display = canvas();

        let width =
            draw_number(&mut display, 137, Point::zero(), &style, BinaryColor::On).expect("draw");
        assert_eq!(width, style.width_of(3));

        let width = draw_number(&mut display, 7, Point::new(0, 100), &style, BinaryColor::On)
            .expect("draw");
        assert_eq!(width, style.width_of(1));
    }

    #[test]
    fn a_three_digit_number_covers_three_digit_positions() {
        let style = DigitStyle::with_height(40);
        let mut display = canvas();
        draw_number(&mut display, 137, Point::zero(), &style, BinaryColor::On).expect("draw");

        for index in 0..3u32 {
            let x = (index * (style.width + style.spacing)) as i32;
            let cell = Rectangle::new(Point::new(x, 0), Size::new(style.width, style.height));
            assert!(ink(&display, cell) > 0, "digit {index} is blank");
        }
    }

    #[test]
    fn zero_renders_as_a_digit_not_as_nothing() {
        let style = DigitStyle::with_height(40);
        let mut display = canvas();
        let width =
            draw_number(&mut display, 0, Point::zero(), &style, BinaryColor::On).expect("draw");
        assert_eq!(width, style.width_of(1));
        let area = Rectangle::new(Point::zero(), Size::new(style.width, style.height));
        assert!(ink(&display, area) > 0);
    }

    #[test]
    fn segments_taper_towards_their_outer_face() {
        let style = DigitStyle::with_height(40);
        let mut display = canvas();
        // 7's top segment has nothing above it, so rows near y = 0 are its own.
        draw_digit(&mut display, 7, Point::zero(), &style, BinaryColor::On).expect("draw");

        let row = |y: u32| {
            ink(
                &display,
                Rectangle::new(Point::new(0, y as i32), Size::new(style.width, 1)),
            )
        };
        // The mitres pull the outermost row in from both ends; the spine runs
        // the segment's full length.
        assert!(
            row(0) < row(style.thickness / 2),
            "outer row {} is not shorter than the spine {}",
            row(0),
            row(style.thickness / 2)
        );
    }

    #[test]
    fn a_seam_separates_segments_that_would_otherwise_meet() {
        let style = DigitStyle::with_height(40);
        let mut display = canvas();
        // 8 lights everything, so every joint in the digit is under test.
        draw_digit(&mut display, 8, Point::zero(), &style, BinaryColor::On).expect("draw");

        // The band between the top segment and the two verticals below it.
        let seam = Rectangle::new(
            Point::new(0, style.thickness as i32),
            Size::new(style.width, style.gap),
        );
        assert_eq!(ink(&display, seam), 0, "the top joint has no gap in it");
    }

    #[test]
    fn the_points_of_a_one_nearly_meet_at_the_waist() {
        let style = DigitStyle::with_height(40);
        let mut display = canvas();
        draw_digit(&mut display, 1, Point::zero(), &style, BinaryColor::On).expect("draw");

        // Down the spine of the two verticals, where their points converge. A
        // `1` has nothing lit between them, so this is the seam at its widest —
        // and it is what decides whether a `1` reads as a single stroke or as
        // two floating bars.
        let x = (style.width - style.thickness / 2) as i32;
        let lit: Vec<i32> = (0..style.height as i32)
            .filter(|y| display.get_pixel(Point::new(x, *y)) == Some(BinaryColor::On))
            .collect();
        let seam = lit
            .windows(2)
            .map(|pair| pair[1] - pair[0] - 1)
            .max()
            .expect("the digit drew nothing");

        assert!(seam > 0, "the two segments merged into one bar");
        assert!(
            seam <= 2 * style.gap as i32,
            "a {seam}px seam at the waist reads as a hole, not a join"
        );
    }

    #[test]
    fn blanks_are_lighter_than_any_digit() {
        let style = DigitStyle::with_height(40);
        let area = Rectangle::new(Point::zero(), Size::new(style.width, style.height));

        let mut blank = canvas();
        draw_blank(&mut blank, 1, Point::zero(), &style, BinaryColor::On).expect("draw");

        let mut one = canvas();
        draw_digit(&mut one, 1, Point::zero(), &style, BinaryColor::On).expect("draw");

        assert!(ink(&blank, area) > 0);
        assert!(ink(&blank, area) < ink(&one, area));
    }

    #[test]
    fn directional_trends_draw_and_undirected_ones_do_not() {
        let area = Rectangle::new(Point::zero(), Size::new(64, 64));
        let center = Point::new(32, 32);

        for trend in [
            Trend::DoubleUp,
            Trend::SingleUp,
            Trend::FortyFiveUp,
            Trend::Flat,
            Trend::FortyFiveDown,
            Trend::SingleDown,
            Trend::DoubleDown,
        ] {
            let mut display = canvas();
            draw_trend(&mut display, trend, center, 40, BinaryColor::On).expect("draw");
            assert!(ink(&display, area) > 0, "{trend:?} drew nothing");
        }

        for trend in [Trend::None, Trend::NotComputable, Trend::RateOutOfRange] {
            let mut display = canvas();
            draw_trend(&mut display, trend, center, 40, BinaryColor::On).expect("draw");
            assert_eq!(ink(&display, area), 0, "{trend:?} should draw nothing");
        }
    }

    #[test]
    fn up_and_down_arrows_carry_the_same_visual_weight() {
        let area = Rectangle::new(Point::zero(), Size::new(64, 64));
        let center = Point::new(32, 32);

        let mut up = canvas();
        draw_trend(&mut up, Trend::SingleUp, center, 40, BinaryColor::On).expect("draw");
        let mut down = canvas();
        draw_trend(&mut down, Trend::SingleDown, center, 40, BinaryColor::On).expect("draw");

        // Not pixel-identical: rasterizing a triangle rounds differently
        // depending on which way it points. A couple of pixels out of four
        // hundred is invisible; a systematically lighter arrow would not be.
        assert!(
            ink(&up, area).abs_diff(ink(&down, area)) <= 4,
            "up {} vs down {}",
            ink(&up, area),
            ink(&down, area)
        );
    }

    #[test]
    fn rapid_trends_draw_more_ink_than_single_ones() {
        let area = Rectangle::new(Point::zero(), Size::new(64, 64));
        let center = Point::new(32, 32);

        let mut single = canvas();
        draw_trend(&mut single, Trend::SingleUp, center, 40, BinaryColor::On).expect("draw");
        let mut double = canvas();
        draw_trend(&mut double, Trend::DoubleUp, center, 40, BinaryColor::On).expect("draw");

        assert!(ink(&double, area) > ink(&single, area));
    }

    #[test]
    fn styles_scale_proportionally() {
        let small = DigitStyle::with_height(50);
        let large = DigitStyle::with_height(200);
        assert_eq!(large.width, small.width * 4);
        assert_eq!(large.thickness, small.thickness * 4);
        assert_eq!(DigitStyle::with_height(100).width_of(0), 0);

        // A gap that rounds to nothing puts the corners back together, so even
        // the smallest style keeps a pixel of seam.
        for height in [10, 20, 40, 190] {
            assert!(DigitStyle::with_height(height).gap > 0, "height {height}");
        }
    }
}
