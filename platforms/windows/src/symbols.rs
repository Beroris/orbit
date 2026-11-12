//! Orbit's own symbols, drawn as shapes on a 24-unit grid instead of taken from the Windows icon
//! font, whose thin strokes look like Windows rather than like Orbit. Each is a few strokes, drawn
//! smooth by measuring how far every pixel is from them. They keep the icon font's code points, so
//! a symbol Orbit hasn't drawn still comes from the font.

use crate::gfx::{Backbuffer, Rgb, blend_coverage};
use windows::Win32::Foundation::RECT;

type P = (f32, f32);

enum Shape {
    /// A line through these points, closed back to the first or not.
    Path(Vec<P>, bool),
    /// Part of a circle, from one angle to another, clockwise in degrees from three o'clock.
    Arc(P, f32, f32, f32),
    Ring(P, f32),
    /// A filled circle.
    Dot(P, f32),
    /// A rounded rectangle's outline: left, top, width, height, corner radius.
    Frame(f32, f32, f32, f32, f32),
    /// A filled shape.
    Fill(Vec<P>),
    /// The outline of a circle with another bitten out of it.
    Moon(P, f32, P, f32),
}

use Shape::*;

fn line(a: P, b: P) -> Shape {
    Path(vec![a, b], false)
}

fn open(points: &[P]) -> Shape {
    Path(points.to_vec(), false)
}

/// A curve from `a` to `b` pulled towards `pull`, as points.
fn curve(a: P, pull: P, b: P, into: &mut Vec<P>) {
    for step in 1..=10 {
        let t = step as f32 / 10.0;
        let u = 1.0 - t;
        into.push((u * u * a.0 + 2.0 * u * t * pull.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * pull.1 + t * t * b.1));
    }
}

/// `points` with each corner cut back by up to `radius` units and bent round, so lines meet in a
/// curve rather than a point, as the rest of Orbit's shapes do. A short side takes less, so two
/// corners never overlap. An open line keeps its two ends.
fn soften(points: &[P], radius: f32, closed: bool) -> Vec<P> {
    let count = points.len();
    if count < 3 {
        return points.to_vec();
    }
    let distance = |a: P, b: P| (b.0 - a.0).hypot(b.1 - a.1);
    let mut out = Vec::with_capacity(count * 7);
    for at in 0..count {
        let corner = points[at];
        if !closed && (at == 0 || at == count - 1) {
            out.push(corner);
            continue;
        }
        let (before, after) = (points[(at + count - 1) % count], points[(at + 1) % count]);
        let (into, out_of) = (distance(before, corner), distance(corner, after));
        let cut = radius.min(into / 2.0).min(out_of / 2.0);
        if cut < 0.05 {
            out.push(corner);
            continue;
        }
        let a = (corner.0 + (before.0 - corner.0) / into * cut, corner.1 + (before.1 - corner.1) / into * cut);
        let b = (corner.0 + (after.0 - corner.0) / out_of * cut, corner.1 + (after.1 - corner.1) / out_of * cut);
        out.push(a);
        // Six steps is round enough for a corner this small.
        for step in 1..=6 {
            let t = step as f32 / 6.0;
            let u = 1.0 - t;
            out.push((u * u * a.0 + 2.0 * u * t * corner.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * corner.1 + t * t * b.1));
        }
    }
    out
}

fn round_open(points: &[P], radius: f32) -> Shape {
    Path(soften(points, radius, false), false)
}

fn round_closed(points: &[P], radius: f32) -> Shape {
    Path(soften(points, radius, true), true)
}

fn at(center: P, radius: f32, degrees: f32) -> P {
    let angle = degrees.to_radians();
    (center.0 + radius * angle.cos(), center.1 + radius * angle.sin())
}

/// Two short strokes making an arrowhead at `tip`, pointing at `degrees`.
fn head(tip: P, degrees: f32, length: f32) -> Shape {
    open(&[at(tip, length, degrees + 180.0 - 42.0), tip, at(tip, length, degrees + 180.0 + 42.0)])
}

fn star() -> Vec<P> {
    (0..10).map(|k| at((12.0, 12.6), if k % 2 == 0 { 9.5 } else { 4.6 }, -90.0 + k as f32 * 36.0)).collect()
}

fn brace(mirror: bool) -> Shape {
    let x = |value: f32| if mirror { 24.0 - value } else { value };
    let mut points = vec![(x(9.5), 3.5)];
    curve((x(9.5), 3.5), (x(7.0), 3.5), (x(7.0), 6.5), &mut points);
    points.push((x(7.0), 9.8));
    curve((x(7.0), 9.8), (x(7.0), 12.0), (x(4.8), 12.0), &mut points);
    curve((x(4.8), 12.0), (x(7.0), 12.0), (x(7.0), 14.2), &mut points);
    points.push((x(7.0), 17.5));
    curve((x(7.0), 17.5), (x(7.0), 20.5), (x(9.5), 20.5), &mut points);
    Path(points, false)
}

fn shapes(glyph: char) -> Option<Vec<Shape>> {
    Some(match glyph {
        // Search
        '\u{E721}' => vec![Ring((10.5, 10.5), 6.5), line((15.4, 15.4), (20.0, 20.0))],
        // Filter: a funnel
        '\u{E71C}' => vec![round_closed(&[(3.5, 4.5), (20.5, 4.5), (14.0, 12.5), (14.0, 19.0), (10.0, 20.5), (10.0, 12.5)], 1.8)],
        // Note
        '\u{E70B}' => vec![
            Frame(4.5, 3.0, 15.0, 18.0, 4.0),
            line((8.5, 8.5), (15.5, 8.5)),
            line((8.5, 12.5), (15.5, 12.5)),
            line((8.5, 16.5), (12.0, 16.5)),
        ],
        // Snippet
        '\u{E943}' => vec![brace(false), brace(true)],
        // Clipboard, with lines of what's on it
        '\u{E77F}' => {
            let mut out = clipboard();
            out.extend([line((8.5, 11.5), (15.5, 11.5)), line((8.5, 15.5), (13.0, 15.5))]);
            out
        }
        // Calculator
        '\u{E8EF}' => {
            let mut out = vec![Frame(5.0, 2.5, 14.0, 19.0, 3.8), Frame(8.25, 5.75, 7.5, 3.5, 1.75)];
            // Four keys, not six, which run together when small.
            for y in [13.25, 17.25] {
                for x in [9.5, 14.5] {
                    out.push(Dot((x, y), 1.3));
                }
            }
            out
        }
        // Money
        '\u{E825}' => vec![Frame(2.5, 6.0, 19.0, 12.0, 3.5), Ring((12.0, 12.0), 2.6), Dot((6.4, 12.0), 1.15), Dot((17.6, 12.0), 1.15)],
        // Sun
        '\u{E706}' => {
            let mut out = vec![Ring((12.0, 12.0), 4.0)];
            for k in 0..8 {
                let degrees = k as f32 * 45.0;
                out.push(line(at((12.0, 12.0), 7.2, degrees), at((12.0, 12.0), 9.4, degrees)));
            }
            out
        }
        // Speed
        '\u{EC4A}' => vec![
            Arc((12.0, 15.0), 9.0, 180.0, 360.0),
            line((3.0, 15.0), (3.0, 16.5)),
            line((21.0, 15.0), (21.0, 16.5)),
            line((12.0, 15.0), (16.6, 9.6)),
            Dot((12.0, 15.0), 2.0),
        ],
        // Link
        '\u{E71B}' => vec![
            Arc((7.0, 12.0), 4.5, 90.0, 270.0),
            line((7.0, 7.5), (9.5, 7.5)),
            line((7.0, 16.5), (9.5, 16.5)),
            Arc((17.0, 12.0), 4.5, 270.0, 450.0),
            line((14.5, 7.5), (17.0, 7.5)),
            line((14.5, 16.5), (17.0, 16.5)),
            line((8.5, 12.0), (15.5, 12.0)),
        ],
        // Globe
        '\u{E774}' => {
            let meridian: Vec<P> = (0..48).map(|k| at((0.0, 0.0), 1.0, k as f32 * 7.5)).map(|(x, y)| (12.0 + x * 4.2, 12.0 + y * 9.0)).collect();
            vec![Ring((12.0, 12.0), 9.0), line((3.0, 12.0), (21.0, 12.0)), Path(meridian, true)]
        }
        // Copy: the copy in front, and the corners of the one behind rounded like it
        '\u{E8C8}' => {
            let behind = round_open(&[(15.5, 8.5), (15.5, 3.0), (3.0, 3.0), (3.0, 15.5), (8.5, 15.5)], 3.2);
            vec![Frame(8.5, 8.5, 12.5, 12.5, 3.2), behind]
        }
        // Pin: a push pin, with a rounded cap, a waist that flares out to its base, and the needle
        '\u{E718}' => {
            let mut left = vec![(9.0, 6.0), (9.0, 10.0)];
            curve((9.0, 10.0), (9.0, 11.8), (7.4, 12.6), &mut left);
            curve((7.4, 12.6), (5.0, 13.8), (5.0, 16.0), &mut left);
            left.push((5.0, 16.5));
            let mut body = left.clone();
            body.extend(left.iter().rev().map(|&(x, y)| (24.0 - x, y)));
            vec![Path(body, false), Frame(7.5, 2.5, 9.0, 3.5, 1.75), line((12.0, 16.5), (12.0, 21.5))]
        }
        // Delete
        '\u{E74D}' => vec![
            line((4.5, 6.0), (19.5, 6.0)),
            round_open(&[(9.0, 6.0), (9.0, 3.5), (15.0, 3.5), (15.0, 6.0)], 1.5),
            round_open(&[(5.75, 6.0), (6.75, 20.75), (17.25, 20.75), (18.25, 6.0)], 3.2),
            line((10.0, 10.5), (10.0, 16.5)),
            line((14.0, 10.5), (14.0, 16.5)),
        ],
        // Edit
        '\u{E70F}' => vec![round_closed(&[(16.5, 3.5), (20.5, 7.5), (8.5, 19.5), (3.5, 20.5), (4.5, 15.5)], 1.6), line((14.0, 6.0), (18.0, 10.0))],
        // Folder
        '\u{E8B7}' => vec![round_closed(&[(3.5, 5.0), (9.0, 5.0), (11.5, 7.5), (20.5, 7.5), (20.5, 19.0), (3.5, 19.0)], 3.5)],
        // File
        '\u{E8A5}' => vec![round_closed(&[(5.5, 3.0), (12.0, 3.0), (18.5, 9.5), (18.5, 21.0), (5.5, 21.0)], 3.0), round_open(&[(12.0, 3.5), (12.0, 9.5), (18.0, 9.5)], 1.2)],
        // Hide
        '\u{ED1A}' => {
            let mut eye = vec![(2.5, 12.0)];
            curve((2.5, 12.0), (12.0, 3.0), (21.5, 12.0), &mut eye);
            curve((21.5, 12.0), (12.0, 21.0), (2.5, 12.0), &mut eye);
            // The eye stops short of the line across it, and only half the pupil shows, so it
            // stays clear when small.
            let (from, to) = ((3.5, 3.5), (20.5, 20.5));
            let mut out = cut_around(&eye, true, from, to, 3.0);
            out.extend([Arc((12.0, 12.0), 3.2, 80.0, 190.0), line(from, to)]);
            out
        }
        // Shield
        '\u{E83D}' => {
            let mut outline = vec![(12.0, 2.5), (19.5, 5.5), (19.5, 11.0)];
            curve((19.5, 11.0), (19.5, 18.5), (12.0, 21.5), &mut outline);
            curve((12.0, 21.5), (4.5, 18.5), (4.5, 11.0), &mut outline);
            outline.push((4.5, 5.5));
            vec![Path(soften(&outline, 2.4, true), true), open(&[(8.6, 12.0), (11.0, 14.4), (15.4, 9.6)])]
        }
        // Tag
        '\u{E8EC}' => vec![round_closed(&[(3.5, 3.5), (11.5, 3.5), (20.5, 12.5), (12.5, 20.5), (3.5, 11.5)], 2.2), Dot((8.0, 8.0), 1.5)],
        // Play
        '\u{E768}' => vec![round_closed(&[(7.5, 4.5), (19.5, 12.0), (7.5, 19.5)], 2.6)],
        // Add
        '\u{E710}' => vec![line((12.0, 5.0), (12.0, 19.0)), line((5.0, 12.0), (19.0, 12.0))],
        // End task, Stop and Cancel: a cross in a circle
        '\u{E711}' => vec![Ring((12.0, 12.0), 8.5), line((9.2, 9.2), (14.8, 14.8)), line((14.8, 9.2), (9.2, 14.8))],
        // Info
        '\u{E946}' => vec![Ring((12.0, 12.0), 9.0), line((12.0, 11.0), (12.0, 16.5)), Dot((12.0, 7.75), 1.3)],
        // Warning
        '\u{E7BA}' => vec![round_closed(&[(12.0, 3.5), (21.5, 20.0), (2.5, 20.0)], 3.5), line((12.0, 9.5), (12.0, 13.5)), Dot((12.0, 16.75), 1.3)],
        // Back and forward
        '\u{E72B}' => vec![line((19.0, 12.0), (5.0, 12.0)), open(&[(11.0, 6.0), (5.0, 12.0), (11.0, 18.0)])],
        '\u{E72A}' => vec![line((5.0, 12.0), (19.0, 12.0)), open(&[(13.0, 6.0), (19.0, 12.0), (13.0, 18.0)])],
        // Check
        '\u{E73E}' => vec![open(&[(4.5, 12.5), (9.5, 17.5), (19.5, 7.0)])],
        // Picture
        '\u{EB9F}' => vec![
            Frame(3.0, 4.0, 18.0, 16.0, 3.8),
            Ring((8.5, 9.5), 1.8),
            round_open(&[(3.5, 17.5), (9.0, 12.0), (14.0, 17.0), (16.5, 14.5), (19.75, 17.75)], 1.6),
        ],
        // Terminal
        '\u{E756}' => vec![Frame(2.5, 4.0, 19.0, 16.0, 3.8), open(&[(7.0, 9.0), (10.0, 12.0), (7.0, 15.0)]), line((12.5, 15.0), (17.0, 15.0))],
        // Keyboard: a row of four keys and the space bar. Three keys read as a face.
        '\u{E765}' => vec![
            Frame(2.5, 5.5, 19.0, 13.0, 3.5),
            Dot((6.75, 9.75), 1.1),
            Dot((10.25, 9.75), 1.1),
            Dot((13.75, 9.75), 1.1),
            Dot((17.25, 9.75), 1.1),
            line((8.0, 14.5), (16.0, 14.5)),
        ],
        // Star, and filled
        '\u{E734}' => vec![Path(soften(&star(), 2.0, true), true)],
        '\u{E735}' => vec![Path(soften(&star(), 2.0, true), true), Fill(soften(&star(), 2.0, true))],
        // Chevrons
        '\u{E70E}' => vec![open(&[(5.5, 15.0), (12.0, 8.5), (18.5, 15.0)])],
        '\u{E70D}' => vec![open(&[(5.5, 9.0), (12.0, 15.5), (18.5, 9.0)])],
        // Arrows up and down, for keycaps
        '\u{E74A}' => vec![line((12.0, 19.5), (12.0, 5.0)), open(&[(6.0, 11.0), (12.0, 5.0), (18.0, 11.0)])],
        '\u{E74B}' => vec![line((12.0, 4.5), (12.0, 19.0)), open(&[(6.0, 13.0), (12.0, 19.0), (18.0, 13.0)])],
        // Enter
        '\u{E751}' => vec![round_open(&[(19.0, 5.0), (19.0, 13.5), (5.0, 13.5)], 3.0), open(&[(9.5, 9.0), (5.0, 13.5), (9.5, 18.0)])],
        // Key
        '\u{E8D7}' => vec![
            Ring((7.5, 16.5), 4.5),
            line((10.7, 13.3), (20.0, 4.0)),
            line((17.0, 7.0), (19.75, 9.75)),
            line((14.25, 9.75), (16.5, 12.0)),
        ],
        // Refresh: round clockwise, ending at the top in an arrowhead that points along the circle
        '\u{E72C}' => vec![Arc((12.0, 12.5), 7.5, 330.0, 630.0), round_open(&[(9.03, 7.68), (12.0, 5.0), (9.03, 2.32)], 0.8)],
        // Emoji
        '\u{E76E}' => vec![Ring((12.0, 12.0), 9.0), Dot((9.0, 9.75), 1.3), Dot((15.0, 9.75), 1.3), Arc((12.0, 12.0), 5.0, 30.0, 150.0)],
        // Settings: a gear with six soft teeth
        '\u{E713}' => {
            let mut gear = Vec::new();
            for k in 0..6 {
                let middle = k as f32 * 60.0;
                gear.push(at((12.0, 12.0), 7.2, middle - 22.0));
                gear.push(at((12.0, 12.0), 9.3, middle - 10.0));
                gear.push(at((12.0, 12.0), 9.3, middle + 10.0));
                gear.push(at((12.0, 12.0), 7.2, middle + 22.0));
            }
            vec![Path(soften(&gear, 1.2, true), true), Ring((12.0, 12.0), 3.0)]
        }
        // Power
        '\u{E7E8}' => vec![Arc((12.0, 13.0), 8.0, 300.0, 600.0), line((12.0, 3.0), (12.0, 11.5))],
        // Pulse, for diagnostics
        '\u{E9D9}' => vec![round_open(&[(2.5, 12.0), (6.5, 12.0), (9.5, 5.5), (14.5, 18.5), (17.5, 12.0), (21.5, 12.0)], 1.4)],
        // Help
        '\u{E897}' => {
            let end = at((12.0, 9.75), 2.75, 45.0);
            vec![Ring((12.0, 12.0), 9.0), Arc((12.0, 9.75), 2.75, 180.0, 405.0), open(&[end, (12.0, 13.25), (12.0, 14.0)]), Dot((12.0, 17.0), 1.3)]
        }
        // Lock
        '\u{E72E}' => vec![
            Frame(5.0, 10.5, 14.0, 11.0, 2.8),
            Arc((12.0, 8.0), 4.0, 180.0, 360.0),
            line((8.0, 8.0), (8.0, 10.5)),
            line((16.0, 8.0), (16.0, 10.5)),
            Dot((12.0, 16.0), 1.3),
        ],
        // Moon, for sleep
        '\u{E708}' => vec![Moon((11.5, 12.5), 8.5, (17.0, 7.0), 7.0)],
        // Laptop
        '\u{E7F8}' => vec![Frame(4.5, 5.0, 15.0, 10.5, 3.0), line((2.5, 19.0), (21.5, 19.0))],
        // Bolt
        '\u{E945}' => vec![round_closed(&[(13.5, 2.5), (5.0, 13.5), (11.0, 13.5), (10.0, 21.5), (19.0, 10.0), (13.0, 10.0)], 1.6)],
        // A window and where it goes on the screen
        '\u{F101}' => window_in(&[(5.5, 7.5), (11.0, 7.5), (11.0, 16.5), (5.5, 16.5)]),
        '\u{F102}' => window_in(&[(13.0, 7.5), (18.5, 7.5), (18.5, 16.5), (13.0, 16.5)]),
        '\u{F103}' => window_in(&[(5.5, 7.5), (18.5, 7.5), (18.5, 11.0), (5.5, 11.0)]),
        '\u{F104}' => window_in(&[(5.5, 13.0), (18.5, 13.0), (18.5, 16.5), (5.5, 16.5)]),
        '\u{F105}' => window_in(&[(5.5, 7.5), (18.5, 7.5), (18.5, 16.5), (5.5, 16.5)]),
        '\u{F106}' => window_in(&[(7.0, 9.0), (17.0, 9.0), (17.0, 15.0), (7.0, 15.0)]),
        '\u{F107}' => window_in(&[(9.0, 10.0), (15.0, 10.0), (15.0, 14.0), (9.0, 14.0)]),
        // To the next display: a screen on its stand, with an arrow across it
        '\u{F108}' => vec![
            Frame(2.5, 3.5, 19.0, 13.0, 3.5),
            line((12.0, 16.5), (12.0, 20.5)),
            line((8.0, 20.5), (16.0, 20.5)),
            line((7.0, 10.0), (16.5, 10.0)),
            head((16.5, 10.0), 0.0, 3.8),
        ],
        // Light and dark: a circle, half filled
        '\u{F109}' => {
            let half: Vec<P> = (0..=18).map(|k| at((12.0, 12.0), 8.5, -90.0 + k as f32 * 10.0)).collect();
            vec![Ring((12.0, 12.0), 8.5), Fill(half)]
        }
        // Bluetooth
        '\u{F10A}' => vec![round_open(&[(6.5, 7.5), (17.0, 16.0), (12.0, 20.5), (12.0, 3.5), (17.0, 8.0), (6.5, 16.5)], 1.2)],
        // Wi-Fi
        '\u{F10B}' => vec![
            Arc((12.0, 19.0), 13.0, 228.0, 312.0),
            Arc((12.0, 19.0), 8.5, 228.0, 312.0),
            Arc((12.0, 19.0), 4.0, 228.0, 312.0),
            Dot((12.0, 19.0), 1.6),
        ],
        // Sound
        '\u{F10C}' => vec![
            round_closed(&[(3.5, 9.5), (7.5, 9.5), (12.5, 5.0), (12.5, 19.0), (7.5, 14.5), (3.5, 14.5)], 1.3),
            Arc((13.0, 12.0), 3.75, 305.0, 415.0),
            Arc((13.0, 12.0), 7.75, 305.0, 415.0),
        ],
        // Timer: an hourglass, its glass and ends one soft outline, with sand run into the bottom, so
        // it isn't taken for the stopwatch
        '\u{F10D}' => vec![
            round_closed(
                &[(6.0, 3.5), (18.0, 3.5), (18.0, 6.0), (12.8, 12.0), (18.0, 18.0), (18.0, 20.5), (6.0, 20.5), (6.0, 18.0), (11.2, 12.0), (6.0, 6.0)],
                2.0,
            ),
            Fill(soften(&[(9.5, 19.5), (14.5, 19.5), (12.0, 16.8)], 0.8, true)),
        ],
        // Stopwatch
        '\u{F10E}' => vec![
            Ring((12.0, 13.5), 8.0),
            line((12.0, 13.5), (15.0, 10.5)),
            line((9.5, 2.5), (14.5, 2.5)),
            line((12.0, 2.5), (12.0, 5.5)),
            line((18.0, 6.5), (19.5, 5.0)),
        ],
        // Eyedropper: a glass tube from the tip, a collar, and the bulb you squeeze
        '\u{F10F}' => {
            let (base, top, radius) = ((15.3, 8.7), (18.6, 5.4), 2.4);
            let mut bulb: Vec<P> = (0..=12).map(|k| at(top, radius, 45.0 - k as f32 * 15.0)).collect();
            bulb.extend((0..=12).map(|k| at(base, radius, 225.0 - k as f32 * 15.0)));
            vec![
                round_open(&[(12.0, 9.0), (3.5, 17.5), (3.5, 20.5), (6.5, 20.5), (15.0, 12.0)], 1.3),
                line((10.5, 7.5), (16.5, 13.5)),
                Path(bulb, true),
            ]
        }
        // Extension: blocks, with one more being added, turned as it goes in. A puzzle piece read as
        // a mug when small.
        '\u{EA86}' => vec![
            Frame(3.0, 3.0, 6.5, 6.5, 2.2),
            Frame(3.0, 14.0, 6.5, 6.5, 2.2),
            Frame(14.0, 14.0, 6.5, 6.5, 2.2),
            round_closed(&[(17.25, 3.25), (20.75, 6.75), (17.25, 10.25), (13.75, 6.75)], 1.8),
        ],
        // Import: an arrow going into a box from the left
        '\u{E8B5}' => {
            let mut box_side = vec![(12.5, 3.5), (17.5, 3.5)];
            curve((17.5, 3.5), (20.5, 3.5), (20.5, 6.5), &mut box_side);
            box_side.push((20.5, 17.5));
            curve((20.5, 17.5), (20.5, 20.5), (17.5, 20.5), &mut box_side);
            box_side.push((12.5, 20.5));
            vec![Path(box_side, false), line((3.0, 12.0), (14.0, 12.0)), head((14.0, 12.0), 0.0, 4.5)]
        }
        // Update: an arrow coming down into a tray, as a download is drawn
        '\u{E896}' => {
            let mut tray = vec![(3.5, 14.5), (3.5, 17.5)];
            curve((3.5, 17.5), (3.5, 20.5), (6.5, 20.5), &mut tray);
            tray.push((17.5, 20.5));
            curve((17.5, 20.5), (20.5, 20.5), (20.5, 17.5), &mut tray);
            tray.push((20.5, 14.5));
            vec![Path(tray, false), line((12.0, 3.5), (12.0, 15.0)), head((12.0, 15.0), 90.0, 4.5)]
        }
        // Branch, for GitHub
        '\u{F110}' => {
            let mut joint = vec![(17.0, 9.0)];
            curve((17.0, 9.0), (17.0, 14.0), (7.0, 15.0), &mut joint);
            vec![Ring((7.0, 5.5), 2.5), Ring((7.0, 18.5), 2.5), Ring((17.0, 6.5), 2.5), line((7.0, 8.0), (7.0, 16.0)), Path(joint, false)]
        }
        // Open, as a link does: an arrow out of the top corner of a box
        '\u{E8A7}' => vec![
            round_open(&[(11.0, 6.0), (4.0, 6.0), (4.0, 20.0), (18.0, 20.0), (18.0, 13.0)], 3.5),
            line((11.5, 12.5), (20.0, 4.0)),
            round_open(&[(14.5, 4.0), (20.0, 4.0), (20.0, 9.5)], 1.0),
        ],
        // Hash, for a reference such as #482
        '\u{F111}' => vec![
            line((4.5, 9.0), (20.0, 9.0)),
            line((4.0, 15.0), (19.5, 15.0)),
            line((10.5, 3.5), (8.5, 20.5)),
            line((15.5, 3.5), (13.5, 20.5)),
        ],
        // Paste: the clipboard, with an arrow going into it
        '\u{F112}' => {
            let mut out = clipboard();
            out.extend([line((12.0, 10.0), (12.0, 17.5)), head((12.0, 17.5), 90.0, 3.6)]);
            out
        }
        // Person, for a username
        '\u{E77B}' => {
            let mut shoulders = vec![(5.0, 20.5)];
            curve((5.0, 20.5), (5.0, 14.0), (12.0, 14.0), &mut shoulders);
            curve((12.0, 14.0), (19.0, 14.0), (19.0, 20.5), &mut shoulders);
            vec![Ring((12.0, 7.5), 4.0), Path(shoulders, false)]
        }
        // Clock, for a code that changes with time
        '\u{E823}' => vec![Ring((12.0, 12.0), 9.0), open(&[(12.0, 7.0), (12.0, 12.0), (15.5, 14.0)])],
        // Swap: one arrow each way
        '\u{E8AB}' => vec![
            line((4.5, 8.0), (19.5, 8.0)),
            head((19.5, 8.0), 0.0, 4.0),
            line((19.5, 16.0), (4.5, 16.0)),
            head((4.5, 16.0), 180.0, 4.0),
        ],
        // Reset: Refresh turned the other way
        '\u{E7A7}' => vec![Arc((12.0, 12.5), 7.5, 270.0, 570.0), round_open(&[(14.97, 2.32), (12.0, 5.0), (14.97, 7.68)], 0.8)],
        // New window: a window with a plus in it
        '\u{E78B}' => vec![
            Frame(2.5, 4.0, 19.0, 16.0, 3.5),
            Dot((6.75, 8.25), 1.05),
            Dot((10.0, 8.25), 1.05),
            line((12.0, 11.0), (12.0, 17.0)),
            line((9.0, 14.0), (15.0, 14.0)),
        ],
        // Open with: a choice of apps
        '\u{E7AC}' => vec![
            Frame(3.5, 3.5, 7.0, 7.0, 2.4),
            Frame(13.5, 3.5, 7.0, 7.0, 2.4),
            Frame(3.5, 13.5, 7.0, 7.0, 2.4),
            Frame(13.5, 13.5, 7.0, 7.0, 2.4),
        ],
        // Text, plain: a T, as plain as the text it pastes
        '\u{E8D2}' => vec![line((5.5, 5.0), (18.5, 5.0)), line((12.0, 5.0), (12.0, 19.5))],
        // Pause
        '\u{E769}' => vec![line((8.5, 5.0), (8.5, 19.0)), line((15.5, 5.0), (15.5, 19.0))],
        _ => return None,
    })
}

/// A clipboard: the board stops either side of its clip, so the two don't run together.
fn clipboard() -> Vec<Shape> {
    let mut board = vec![(8.0, 4.5), (7.5, 4.5)];
    curve((7.5, 4.5), (4.5, 4.5), (4.5, 7.5), &mut board);
    board.push((4.5, 18.5));
    curve((4.5, 18.5), (4.5, 21.5), (7.5, 21.5), &mut board);
    board.push((16.5, 21.5));
    curve((16.5, 21.5), (19.5, 21.5), (19.5, 18.5), &mut board);
    board.push((19.5, 7.5));
    curve((19.5, 7.5), (19.5, 4.5), (16.5, 4.5), &mut board);
    board.push((16.0, 4.5));
    vec![Path(board, false), Frame(8.5, 2.5, 7.0, 4.0, 1.4)]
}

/// A window, with the part of the screen it goes to filled in.
fn window_in(part: &[P]) -> Vec<Shape> {
    vec![Frame(2.5, 4.5, 19.0, 15.0, 3.5), Fill(soften(part, 1.6, true))]
}

/// The line through `points`, less the parts within `gap` of the segment from `a` to `b`, as
/// separate strokes: for one line crossing another with room either side.
fn cut_around(points: &[P], closed: bool, a: P, b: P, gap: f32) -> Vec<Shape> {
    // A point every quarter of a unit, so the cut falls where it should.
    let mut fine = Vec::new();
    let sides = if closed { points.len() } else { points.len().saturating_sub(1) };
    for k in 0..sides {
        let (p, q) = (points[k], points[(k + 1) % points.len()]);
        let steps = ((q.0 - p.0).hypot(q.1 - p.1) * 4.0).ceil().max(1.0) as usize;
        fine.extend((0..steps).map(|step| {
            let t = step as f32 / steps as f32;
            (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t)
        }));
    }
    if !closed {
        fine.extend(points.last());
    }
    let kept = |p: &P| segment_distance(*p, a, b) > gap;
    let mut runs: Vec<Vec<P>> = vec![Vec::new()];
    for p in &fine {
        match (kept(p), runs.last_mut()) {
            (true, Some(run)) => run.push(*p),
            (false, Some(run)) if !run.is_empty() => runs.push(Vec::new()),
            _ => {}
        }
    }
    // Round a closed line, the last part carries on into the first.
    if closed && runs.len() > 1 && fine.first().is_some_and(kept) && fine.last().is_some_and(kept) {
        let mut last = runs.pop().unwrap_or_default();
        last.append(&mut runs[0]);
        runs[0] = last;
    }
    runs.into_iter().filter(|run| run.len() > 1).map(|run| Path(run, false)).collect()
}

/// Whether Orbit draws this symbol itself.
#[cfg(test)]
fn has(glyph: char) -> bool {
    shapes(glyph).is_some()
}

fn segment_distance(p: P, a: P, b: P) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0) } else { 0.0 };
    let (x, y) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    (x * x + y * y).sqrt()
}

fn path_distance(p: P, points: &[P], closed: bool) -> f32 {
    let mut best = f32::MAX;
    for pair in points.windows(2) {
        best = best.min(segment_distance(p, pair[0], pair[1]));
    }
    if closed && points.len() > 2 {
        best = best.min(segment_distance(p, points[points.len() - 1], points[0]));
    }
    best
}

fn inside(p: P, points: &[P]) -> bool {
    let mut odd = false;
    let mut last = points[points.len() - 1];
    for &point in points {
        if (point.1 > p.1) != (last.1 > p.1) && p.0 < (last.0 - point.0) * (p.1 - point.1) / (last.1 - point.1) + point.0 {
            odd = !odd;
        }
        last = point;
    }
    odd
}

fn length(p: P, center: P) -> f32 {
    ((p.0 - center.0).powi(2) + (p.1 - center.1).powi(2)).sqrt()
}

/// How much of the pixel at `p` a shape covers, with `half` the half width of a stroke and `unit`
/// the size of one grid unit in pixels.
fn coverage_of(shape: &Shape, p: P, half: f32, unit: f32) -> f32 {
    let stroke = |distance: f32| ((half - distance) * unit + 0.5).clamp(0.0, 1.0);
    let fill = |signed: f32| (0.5 - signed * unit).clamp(0.0, 1.0);
    match shape {
        Path(points, closed) => stroke(path_distance(p, points, *closed)),
        Arc(center, radius, from, to) => {
            let angle = (p.1 - center.1).atan2(p.0 - center.0).to_degrees();
            let distance = if (angle - from).rem_euclid(360.0) <= to - from {
                (length(p, *center) - radius).abs()
            } else {
                length(p, at(*center, *radius, *from)).min(length(p, at(*center, *radius, *to)))
            };
            stroke(distance)
        }
        Ring(center, radius) => stroke((length(p, *center) - radius).abs()),
        Dot(center, radius) => fill(length(p, *center) - radius),
        Frame(x, y, w, h, r) => stroke(crate::gfx::round_rect_distance(p.0 - x, p.1 - y, *w, *h, *r).abs()),
        Fill(points) => {
            let distance = path_distance(p, points, true);
            fill(if inside(p, points) { -distance } else { distance })
        }
        Moon(center, radius, bite, bite_radius) => stroke((length(p, *center) - radius).max(bite_radius - length(p, *bite)).abs()),
    }
}

/// Draws `glyph` in `ink`, `size` pixels square, placed in `rect` as text would be. `heavy` is for
/// a symbol on a coloured tile. Returns false, having drawn nothing, for a symbol Orbit doesn't
/// draw itself.
pub fn draw(buffer: &Backbuffer, glyph: char, size: f32, heavy: bool, ink: Rgb, rect: RECT, align: crate::text::Align) -> bool {
    draw_turned(buffer, glyph, size, heavy, 0.0, ink, rect, align)
}

/// Draws `glyph` as [`draw`] does, turned clockwise about its middle by `degrees`, such as a
/// chevron that turns as what it opens slides out.
#[allow(clippy::too_many_arguments)]
pub fn draw_turned(buffer: &Backbuffer, glyph: char, size: f32, heavy: bool, degrees: f32, ink: Rgb, rect: RECT, align: crate::text::Align) -> bool {
    // The grid leaves room around each symbol, as the icon font does, but a little less.
    let size = size * 1.12;
    let left = match align {
        crate::text::Align::Left => rect.left as f32,
        crate::text::Align::Right => rect.right as f32 - size,
        crate::text::Align::Center => (rect.left + rect.right) as f32 / 2.0 - size / 2.0,
    };
    let top = (rect.top + rect.bottom) as f32 / 2.0 - size / 2.0;
    // Worked out once for each size, turn and place within a pixel, then only blended after that:
    // measuring every pixel's distance from every stroke took most of a frame.
    let (left_px, top_px) = (left.floor(), top.floor());
    let key = MaskKey {
        glyph,
        size: size.to_bits(),
        heavy,
        degrees: degrees.to_bits(),
        offset: (((left - left_px) * 64.0) as u8, ((top - top_px) * 64.0) as u8),
    };
    MASKS.with(|masks| {
        let mut masks = masks.borrow_mut();
        if !masks.contains_key(&key) {
            let Some(mask) = rasterize(glyph, size, heavy, degrees, left - left_px, top - top_px) else { return false };
            if masks.len() >= MASKS_MOST {
                masks.clear();
            }
            masks.insert(key, mask);
        }
        let mask = &masks[&key];
        blend_mask(buffer, mask, left_px as i32, top_px as i32, ink);
        true
    })
}

/// What makes one drawing of a symbol different from another.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MaskKey {
    glyph: char,
    size: u32,
    heavy: bool,
    degrees: u32,
    /// Where it starts within its first pixel, in 64ths.
    offset: (u8, u8),
}

/// How much of each pixel a symbol covers, from `x`, `y` relative to the pixel it starts in.
struct Mask {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    coverage: Vec<u8>,
}

/// Enough for every symbol on screen at a few sizes. A mask is a few hundred bytes.
const MASKS_MOST: usize = 384;

thread_local! {
    static MASKS: std::cell::RefCell<std::collections::HashMap<MaskKey, Mask>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Lets go of the symbols worked out so far, as when memory is short.
pub fn forget_masks() {
    MASKS.with(|masks| *masks.borrow_mut() = std::collections::HashMap::new());
}

/// Measures how much of each pixel the symbol covers, starting `fx`, `fy` into its first pixel.
fn rasterize(glyph: char, size: f32, heavy: bool, degrees: f32, fx: f32, fy: f32) -> Option<Mask> {
    let shapes = shapes(glyph)?;
    // Each pixel is turned back the other way to find where it falls on the grid.
    let (sin, cos) = (-degrees).to_radians().sin_cos();
    let turn = |p: P| {
        if degrees == 0.0 {
            return p;
        }
        let (x, y) = (p.0 - 12.0, p.1 - 12.0);
        (12.0 + x * cos - y * sin, 12.0 + x * sin + y * cos)
    };
    let unit = size / 24.0;
    // Strokes 2 units wide, or 2.4 on a tile, but never under a pixel and a quarter, or a pixel and
    // a half on a tile, so that small ones don't go thin.
    let width = if heavy { (2.4f32).max(1.5 / unit) } else { (2.0f32).max(1.25 / unit) };
    let half = width / 2.0;
    let reach = half * unit + 1.0;
    let (x0, x1) = ((fx - reach).floor() as i32, (fx + size + reach).ceil() as i32);
    let (y0, y1) = ((fy - reach).floor() as i32, (fy + size + reach).ceil() as i32);
    let (w, h) = ((x1 - x0).max(0), (y1 - y0).max(0));
    let mut coverage = vec![0u8; (w * h) as usize];
    for py in y0..y1 {
        for px in x0..x1 {
            let p = turn(((px as f32 + 0.5 - fx) / unit, (py as f32 + 0.5 - fy) / unit));
            let mut amount = 0.0f32;
            for shape in &shapes {
                amount = amount.max(coverage_of(shape, p, half, unit));
                if amount >= 1.0 {
                    break;
                }
            }
            coverage[((py - y0) * w + (px - x0)) as usize] = (amount * 255.0).round() as u8;
        }
    }
    Some(Mask { x: x0, y: y0, width: w, height: h, coverage })
}

/// Blends `ink` into the buffer by the mask, with its first pixel at `left`, `top`.
fn blend_mask(buffer: &Backbuffer, mask: &Mask, left: i32, top: i32, ink: Rgb) {
    let (bits, width_px, height_px) = buffer.raw();
    let (x0, y0) = (left + mask.x, top + mask.y);
    for row in 0..mask.height {
        let py = y0 + row;
        if py < 0 || py >= height_px {
            continue;
        }
        let line = &mask.coverage[(row * mask.width) as usize..((row + 1) * mask.width) as usize];
        for (column, &amount) in line.iter().enumerate() {
            let px = x0 + column as i32;
            if amount == 0 || px < 0 || px >= width_px {
                continue;
            }
            // Safety: (px, py) is inside the buffer, which `raw` describes.
            unsafe {
                let pixel = bits.add((py * width_px + px) as usize);
                *pixel = blend_coverage(*pixel, ink, amount);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symbol_orbit_uses_is_drawn() {
        // Every symbol named in rows.rs, so a new one can't quietly come from the thin icon font.
        let named: Vec<(&str, char)> = include_str!("rows.rs")
            .lines()
            .filter_map(|line| {
                let (name, value) = line.trim().strip_prefix("pub const GLYPH_")?.split_once(": char = '\\u{")?;
                Some((name, char::from_u32(u32::from_str_radix(value.split('}').next()?, 16).ok()?)?))
            })
            .collect();
        assert!(named.len() > 50, "found {}", named.len());
        for (name, glyph) in named {
            assert!(has(glyph), "GLYPH_{name} ({:X}) isn't drawn", glyph as u32);
        }
        for glyph in ['\u{E72E}', '\u{E708}', '\u{E751}', '\u{E74A}', '\u{E74B}', '\u{E72A}', '\u{E8A5}'] {
            assert!(has(glyph), "{:X}", glyph as u32);
        }
    }

    /// Draws the symbols named in ORBIT_SYMBOLS (as hex code points, comma separated) to the raw
    /// file named in ORBIT_SYMBOL_SHEET: each large, with the size the menus use under it.
    #[test]
    #[ignore = "writes a picture for looking at"]
    fn symbol_sheet() {
        let (Ok(out), Ok(list)) = (std::env::var("ORBIT_SYMBOL_SHEET"), std::env::var("ORBIT_SYMBOLS")) else { return };
        let glyphs: Vec<char> = list.split(',').filter_map(|hex| char::from_u32(u32::from_str_radix(hex.trim(), 16).ok()?)).collect();
        let (columns, cell_w, cell_h) = (10, 76, 100);
        let lines = glyphs.len().div_ceil(columns) as i32;
        let mut sheet = crate::gfx::Backbuffer::new(columns as i32 * cell_w, lines * cell_h).unwrap();
        sheet.fill(crate::gfx::Rgb(36, 36, 40));
        let ink = crate::gfx::Rgb(205, 205, 212);
        for (at, glyph) in glyphs.iter().enumerate() {
            let (x, y) = ((at % columns) as i32 * cell_w, (at / columns) as i32 * cell_h);
            let big = RECT { left: x, top: y + 8, right: x + cell_w, bottom: y + 64 };
            assert!(draw(&sheet, *glyph, 48.0, false, ink, big, crate::text::Align::Center), "{:X}", *glyph as u32);
            let small = RECT { left: x, top: y + 70, right: x + cell_w, bottom: y + 94 };
            draw(&sheet, *glyph, 15.0, false, ink, small, crate::text::Align::Center);
        }
        sheet.write_raw(std::path::Path::new(&out)).unwrap();
    }

    #[test]
    fn strokes_are_solid_on_the_line_and_clear_away_from_it() {
        let shape = line((0.0, 12.0), (24.0, 12.0));
        assert_eq!(coverage_of(&shape, (12.0, 12.0), 1.0, 1.0), 1.0);
        assert_eq!(coverage_of(&shape, (12.0, 16.0), 1.0, 1.0), 0.0);
        let ring = Ring((12.0, 12.0), 9.0);
        assert_eq!(coverage_of(&ring, (21.0, 12.0), 1.0, 1.0), 1.0);
        assert_eq!(coverage_of(&ring, (12.0, 12.0), 1.0, 1.0), 0.0);
    }

    #[test]
    fn an_arc_covers_only_its_part_of_the_circle() {
        // The top half: from nine o'clock, clockwise, round to three.
        let arc = Arc((12.0, 12.0), 8.0, 180.0, 360.0);
        assert_eq!(coverage_of(&arc, (12.0, 4.0), 1.0, 1.0), 1.0);
        assert_eq!(coverage_of(&arc, (12.0, 20.0), 1.0, 1.0), 0.0);
    }

    #[test]
    fn a_symbol_is_drawn_the_same_from_the_cache() {
        let ink = crate::gfx::Rgb(230, 230, 236);
        let draw_at = |x: i32, y: i32| {
            let buffer = crate::gfx::Backbuffer::new(80, 60).unwrap();
            let rect = RECT { left: x, top: y, right: x + 25, bottom: y + 25 };
            assert!(draw(&buffer, '\u{E721}', 16.0, false, ink, rect, crate::text::Align::Center));
            buffer
        };
        forget_masks();
        let first = draw_at(10, 10);
        let again = draw_at(10, 10);
        assert_eq!(first.snapshot(), again.snapshot());
        assert!(first.snapshot().iter().any(|&pixel| pixel != 0), "something was drawn");
        // Somewhere else, it's the same picture, moved.
        let moved = draw_at(40, 21).snapshot();
        let first = first.snapshot();
        for y in 0..35 {
            for x in 0..35 {
                assert_eq!(moved[((y + 11) * 80 + x + 30) as usize], first[(y * 80 + x) as usize], "at {x},{y}");
            }
        }
        forget_masks();
    }

    #[test]
    fn a_fill_covers_its_inside() {
        let square = Fill(vec![(4.0, 4.0), (20.0, 4.0), (20.0, 20.0), (4.0, 20.0)]);
        assert_eq!(coverage_of(&square, (12.0, 12.0), 1.0, 1.0), 1.0);
        assert_eq!(coverage_of(&square, (2.0, 12.0), 1.0, 1.0), 0.0);
    }
}
