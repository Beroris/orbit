//! The launcher's text, through DirectWrite. GDI places letters on whole pixels and hints them to
//! fit, which is what made the text look dated. This places them where the font wants them, and
//! can ask a variable font for exact weights and optical sizes.
//!
//! Direct2D isn't needed. DirectWrite hands back how much of each pixel a run of letters covers,
//! and that's blended into the same CPU-drawn buffer as everything else.

use crate::gfx::{Backbuffer, Rgb, blend_coverage};
use std::sync::OnceLock;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{BOOL, IUnknown, Interface, PCWSTR, Ref, Result, implement, w};

const FLUENT_ICONS: PCWSTR = w!("Segoe Fluent Icons");
const OLDER_ICONS: PCWSTR = w!("Segoe MDL2 Assets");
const GEIST: PCWSTR = w!("Geist");
const VARIABLE_UI: PCWSTR = w!("Segoe UI Variable");
const VARIABLE_UI_TEXT: PCWSTR = w!("Segoe UI Variable Text");
const OLDER_UI: PCWSTR = w!("Segoe UI");
const EMOJI_FONT: PCWSTR = w!("Segoe UI Emoji");
const LOCALE: PCWSTR = w!("en-us");

/// Where a line of text sits in its box, across. It's always centred down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Right,
    Center,
}

struct Engine {
    factory: IDWriteFactory2,
    /// Geist, from the bytes built into the exe, for exact weights. On Windows 10 2004 and later.
    geist: Option<(IDWriteFactory6, IDWriteFontCollection2)>,
    /// For exact weights in a variable font, on Windows 10 2004 and later.
    variable: Option<(IDWriteFactory6, IDWriteFontCollection2)>,
    renderer: IDWriteTextRenderer,
    /// Marks a stretch of letters as drawn in the second colour. Only its presence means anything.
    marker: IUnknown,
}

// The interfaces themselves are free-threaded, and only the launcher's own thread draws.
unsafe impl Send for Engine {}
unsafe impl Sync for Engine {}

fn engine() -> Option<&'static Engine> {
    static ENGINE: OnceLock<Option<Engine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| unsafe {
            let factory: IDWriteFactory2 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let renderer: IDWriteTextRenderer = Sink.into();
            let marker: IUnknown = renderer.cast().ok()?;
            let variable = factory.cast::<IDWriteFactory6>().ok().and_then(|six| {
                let fonts = six.GetSystemFontCollection(false, DWRITE_FONT_FAMILY_MODEL_TYPOGRAPHIC).ok()?;
                has_family(&fonts.cast().ok()?, VARIABLE_UI).then_some((six, fonts))
            });
            let geist = factory.cast::<IDWriteFactory6>().ok().and_then(|six| Some((six.clone(), geist_fonts(&six)?)));
            Some(Engine { factory, geist, variable, renderer, marker })
        })
        .as_ref()
}

/// Geist as a font collection of its own, read from memory. Nothing is installed on the system,
/// and letters Geist lacks still come from Windows' fallback fonts.
fn geist_fonts(factory: &IDWriteFactory6) -> Option<IDWriteFontCollection2> {
    unsafe {
        let loader = factory.CreateInMemoryFontFileLoader().ok()?;
        factory.RegisterFontFileLoader(&loader).ok()?;
        let bytes = crate::gfx::GEIST;
        let file = loader.CreateInMemoryFontFileReference(factory, bytes.as_ptr() as *const _, bytes.len() as u32, None).ok()?;
        let builder: IDWriteFontSetBuilder1 = factory.CreateFontSetBuilder().ok()?.cast().ok()?;
        builder.AddFontFile(&file).ok()?;
        let set = builder.CreateFontSet().ok()?;
        let fonts = factory.CreateFontCollectionFromFontSet(&set, DWRITE_FONT_FAMILY_MODEL_TYPOGRAPHIC).ok()?;
        has_family(&fonts.cast().ok()?, GEIST).then_some(fonts)
    }
}

fn has_family(fonts: &IDWriteFontCollection, name: PCWSTR) -> bool {
    let (mut index, mut found) = (0u32, BOOL(0));
    unsafe { fonts.FindFamilyName(name, &mut index, &mut found).is_ok() && found.as_bool() }
}

/// A font at a size and weight.
pub struct Face {
    format: IDWriteTextFormat,
    ellipsis: Option<IDWriteInlineObject>,
    /// For a face of symbols, their size in pixels and whether they're heavy. Orbit draws most of
    /// its symbols itself (see `symbols.rs`) and only falls back to the font for the rest.
    symbol: Option<(f32, bool)>,
}

impl Face {
    fn from_format(engine: &Engine, format: IDWriteTextFormat) -> Option<Face> {
        unsafe {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).ok()?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok()?;
        }
        let ellipsis = unsafe { engine.factory.CreateEllipsisTrimmingSign(&format) }.ok();
        Some(Face { format, ellipsis, symbol: None })
    }

    /// Text. `px` is its size on this screen, `dip` the same size at 100%, which picks the cut.
    pub fn ui(px: f32, dip: f32, weight: u32) -> Option<Face> {
        let engine = engine()?;
        Face::geist(engine, px, weight).or_else(|| Face::variable(engine, px, dip, weight)).or_else(|| Face::fixed(engine, px, weight))
    }

    /// Geist has one axis, weight, and no optical sizes.
    fn geist(engine: &Engine, px: f32, weight: u32) -> Option<Face> {
        let (factory, fonts) = engine.geist.as_ref()?;
        let axes = [DWRITE_FONT_AXIS_VALUE { axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT, value: weight as f32 }];
        let format: IDWriteTextFormat3 = unsafe { factory.CreateTextFormat(GEIST, fonts, &axes, px, LOCALE) }.ok()?;
        Face::from_format(engine, format.cast().ok()?)
    }

    fn variable(engine: &Engine, px: f32, dip: f32, weight: u32) -> Option<Face> {
        let (factory, fonts) = engine.variable.as_ref()?;
        // Optical size is in points. Large type gets the tighter cut, small type the open one.
        let axes = [
            DWRITE_FONT_AXIS_VALUE { axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT, value: weight as f32 },
            DWRITE_FONT_AXIS_VALUE { axisTag: DWRITE_FONT_AXIS_TAG_OPTICAL_SIZE, value: (dip * 0.75).clamp(8.0, 36.0) },
        ];
        let format: IDWriteTextFormat3 = unsafe { factory.CreateTextFormat(VARIABLE_UI, fonts, &axes, px, LOCALE) }.ok()?;
        Face::from_format(engine, format.cast().ok()?)
    }

    fn fixed(engine: &Engine, px: f32, weight: u32) -> Option<Face> {
        let system = system_fonts(engine)?;
        let family = if has_family(&system, VARIABLE_UI_TEXT) { VARIABLE_UI_TEXT } else { OLDER_UI };
        // The fixed families stop at these.
        let weight = if weight >= 550 { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
        let format = unsafe { engine.factory.CreateTextFormat(family, &system, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, px, LOCALE) }.ok()?;
        Face::from_format(engine, format)
    }

    /// Symbols, from Windows' icon font.
    pub fn icons(px: f32) -> Option<Face> {
        Face::icons_weighted(px, DWRITE_FONT_WEIGHT_NORMAL)
    }

    /// The symbols, made heavier. The icon fonts come in one weight only, so DirectWrite thickens
    /// their strokes itself, which suits a symbol on a coloured tile.
    pub fn icons_bold(px: f32) -> Option<Face> {
        Face::icons_weighted(px, DWRITE_FONT_WEIGHT_BOLD)
    }

    fn icons_weighted(px: f32, weight: DWRITE_FONT_WEIGHT) -> Option<Face> {
        let engine = engine()?;
        let system = system_fonts(engine)?;
        let family = if has_family(&system, FLUENT_ICONS) { FLUENT_ICONS } else { OLDER_ICONS };
        let format = unsafe { engine.factory.CreateTextFormat(family, &system, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, px, LOCALE) }.ok()?;
        let mut face = Face::from_format(engine, format)?;
        face.symbol = Some((px, weight == DWRITE_FONT_WEIGHT_BOLD));
        Some(face)
    }
}

fn system_fonts(engine: &Engine) -> Option<IDWriteFontCollection> {
    let mut fonts = None;
    unsafe { engine.factory.GetSystemFontCollection(&mut fonts, false) }.ok()?;
    fonts
}

/// One line only: line breaks and tabs become spaces, one for one, so positions don't move.
fn one_line(text: &str) -> Vec<u16> {
    text.encode_utf16()
        .map(|unit| if unit < 0x20 || unit == 0x2028 || unit == 0x2029 { 0x20 } else { unit })
        .collect()
}

fn layout(engine: &Engine, text: &[u16], face: &Face, width: f32, height: f32) -> Option<IDWriteTextLayout> {
    unsafe { engine.factory.CreateTextLayout(text, &face.format, width, height) }.ok()
}

/// How wide `text` is, in pixels, rounded up.
pub fn width(text: &str, face: &Face) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let Some(engine) = engine() else { return 0 };
    let units = one_line(text);
    let Some(layout) = layout(engine, &units, face, 100_000.0, 100.0) else { return 0 };
    let mut metrics = DWRITE_TEXT_METRICS::default();
    if unsafe { layout.GetMetrics(&mut metrics) }.is_err() {
        return 0;
    }
    metrics.widthIncludingTrailingWhitespace.ceil() as i32
}

/// What a draw needs on the far side of DirectWrite's callbacks.
struct Target {
    factory: IDWriteFactory2,
    bits: *mut u32,
    width: i32,
    clip: RECT,
    ink: Rgb,
    marked_ink: Rgb,
    weights: [u8; 256],
    coverage: Vec<u8>,
    /// Draw a font's own colours, for emoji.
    in_colour: bool,
}

/// A stretch of a line to draw in the second colour, in UTF-16 units from its start.
pub type Span = (u32, u32);

/// One line of text in `rect`, cut short with an ellipsis if it doesn't fit.
pub fn draw(buffer: &Backbuffer, text: &str, face: &Face, ink: Rgb, rect: RECT, align: Align) {
    draw_marked(buffer, text, face, ink, rect, align, &[], ink);
}

/// The same, with some stretches of it in `marked_ink`.
#[allow(clippy::too_many_arguments)]
pub fn draw_marked(buffer: &Backbuffer, text: &str, face: &Face, ink: Rgb, rect: RECT, align: Align, spans: &[Span], marked_ink: Rgb) {
    if let Some((size, heavy)) = face.symbol {
        let mut chars = text.chars();
        if let (Some(glyph), None) = (chars.next(), chars.next())
            && crate::symbols::draw(buffer, glyph, size, heavy, ink, rect, align)
        {
            return;
        }
    }
    let Some(engine) = engine() else { return };
    if text.is_empty() {
        return;
    }
    let units = one_line(text);
    let (box_width, box_height) = ((rect.right - rect.left).max(1) as f32, (rect.bottom - rect.top).max(1) as f32);
    let Some(line) = layout(engine, &units, face, box_width, box_height) else { return };
    unsafe {
        let alignment = match align {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
        };
        let _ = line.SetTextAlignment(alignment);
        let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        if let Some(ellipsis) = &face.ellipsis {
            let _ = line.SetTrimming(&trimming, ellipsis);
        }
        for &(start, length) in spans {
            let _ = line.SetDrawingEffect(&engine.marker, DWRITE_TEXT_RANGE { startPosition: start, length });
        }
    }
    render(engine, buffer, &line, ink, marked_ink, rect.left as f32, rect.top as f32, rect);
}

/// Text over several lines from the top of `rect`, broken between words, and inside a word only
/// when one is wider than the box. Whatever doesn't fit is cut off at the bottom.
pub fn draw_wrapped(buffer: &Backbuffer, text: &str, face: &Face, ink: Rgb, rect: RECT) {
    let Some(engine) = engine() else { return };
    if text.is_empty() {
        return;
    }
    // Line breaks and tabs stay, and other control characters become spaces.
    let units: Vec<u16> = text.encode_utf16().map(|unit| if unit < 0x20 && !matches!(unit, 0x09 | 0x0A | 0x0D) { 0x20 } else { unit }).collect();
    let (box_width, box_height) = ((rect.right - rect.left).max(1) as f32, (rect.bottom - rect.top).max(1) as f32);
    let Some(block) = layout(engine, &units, face, box_width, box_height) else { return };
    unsafe {
        if block.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK).is_err() {
            let _ = block.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
        }
        let _ = block.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        let _ = block.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    }
    render(engine, buffer, &block, ink, ink, rect.left as f32, rect.top as f32, rect);
}

/// How an editable block of text shows its selection and caret.
pub struct EditLook {
    pub selection: Rgb,
    pub selection_alpha: u8,
    /// The caret's colour, when it's showing.
    pub caret: Option<Rgb>,
    pub caret_width: i32,
}

/// Text someone is typing over several lines, wrapped like `draw_wrapped`, with its selection and
/// caret. `selection` is in UTF-16 units, as an edit control counts them, with a line break as
/// `\r\n`. When the text is taller than `rect`, it scrolls to keep the caret in view.
pub fn draw_editable(buffer: &mut Backbuffer, text: &str, face: &Face, ink: Rgb, rect: RECT, selection: (usize, usize), look: EditLook) {
    let Some(engine) = engine() else { return };
    let units: Vec<u16> = text.encode_utf16().map(|unit| if unit < 0x20 && !matches!(unit, 0x09 | 0x0A | 0x0D) { 0x20 } else { unit }).collect();
    let (box_width, box_height) = ((rect.right - rect.left).max(1) as f32, (rect.bottom - rect.top).max(1) as f32);
    let Some(block) = layout(engine, &units, face, box_width, box_height) else { return };
    unsafe {
        if block.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK).is_err() {
            let _ = block.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
        }
        let _ = block.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        let _ = block.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    }
    let (start, end) = (selection.0.min(selection.1).min(units.len()), selection.0.max(selection.1).min(units.len()));
    let caret_at = selection.1.min(units.len());
    // Where the caret is with nothing scrolled, which decides how far to scroll.
    let (mut caret_x, mut caret_y) = (0f32, 0f32);
    let mut caret = DWRITE_HIT_TEST_METRICS::default();
    let found = unsafe { block.HitTestTextPosition(caret_at as u32, false, &mut caret_x, &mut caret_y, &mut caret) }.is_ok();
    let line_height = if found && caret.height > 0.0 { caret.height } else { box_height.min(20.0) };
    let scroll = if found { (caret_y + line_height - box_height).max(0.0) } else { 0.0 };
    let (left, top) = (rect.left as f32, rect.top as f32 - scroll);
    if start < end {
        let mut spans = vec![DWRITE_HIT_TEST_METRICS::default(); 64];
        let mut count = 0u32;
        if unsafe { block.HitTestTextRange(start as u32, (end - start) as u32, left, top, Some(&mut spans), &mut count) }.is_ok() {
            for span in spans.iter().take(count as usize) {
                let (x, y) = (span.left.floor() as i32, span.top.floor() as i32);
                // An empty line in the selection still shows a sliver, so it reads as selected.
                let width = (span.width.ceil() as i32).max(look.caret_width * 2);
                let (y0, y1) = (y.max(rect.top), (y + span.height.ceil() as i32).min(rect.bottom));
                if y1 > y0 {
                    buffer.fill_rect_alpha(x.max(rect.left), y0, width.min(rect.right - x.max(rect.left)), y1 - y0, look.selection, look.selection_alpha);
                }
            }
        }
    }
    if !units.is_empty() {
        render(engine, buffer, &block, ink, ink, left, top, rect);
    }
    if let (Some(color), true, true) = (look.caret, found, start == end) {
        let x = (left + caret_x).round() as i32;
        let (y0, y1) = (((top + caret_y).round() as i32).max(rect.top), ((top + caret_y + line_height).round() as i32).min(rect.bottom));
        if y1 > y0 {
            buffer.fill_rect_alpha(x.min(rect.right - look.caret_width), y0, look.caret_width, y1 - y0, color, 255);
        }
    }
}

/// Text starting at `x`, which may be left of `clip`, showing only what's inside `clip`.
pub fn draw_clipped(buffer: &Backbuffer, text: &str, face: &Face, ink: Rgb, x: i32, clip: RECT) {
    let Some(engine) = engine() else { return };
    if text.is_empty() {
        return;
    }
    let units = one_line(text);
    let Some(line) = layout(engine, &units, face, 100_000.0, (clip.bottom - clip.top).max(1) as f32) else { return };
    render(engine, buffer, &line, ink, ink, x as f32, clip.top as f32, clip);
}

#[allow(clippy::too_many_arguments)]
fn render(engine: &Engine, buffer: &Backbuffer, line: &IDWriteTextLayout, ink: Rgb, marked_ink: Rgb, x: f32, y: f32, clip: RECT) {
    let (bits, width, height) = buffer.raw();
    let clip = RECT { left: clip.left.clamp(0, width), top: clip.top.clamp(0, height), right: clip.right.clamp(0, width), bottom: clip.bottom.clamp(0, height) };
    if clip.left >= clip.right || clip.top >= clip.bottom {
        return;
    }
    let mut target = Target {
        factory: engine.factory.clone(),
        bits,
        width,
        clip,
        ink,
        marked_ink,
        weights: weights_for(ink),
        coverage: Vec::new(),
        in_colour: false,
    };
    let context = &mut target as *mut Target as *const core::ffi::c_void;
    let _ = unsafe { line.Draw(Some(context), &engine.renderer, x, y) };
}

/// One emoji in its own colours, as premultiplied BGRA in a `size` by `size` square. Kept apart from
/// the frame so that each one is only worked out once, however often the list is drawn.
pub fn emoji_pixels(glyph: &str, size: i32) -> Option<Vec<u8>> {
    let engine = engine()?;
    let system = system_fonts(engine)?;
    let px = size as f32 * 0.76;
    let format = unsafe { engine.factory.CreateTextFormat(EMOJI_FONT, &system, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, px, LOCALE) }.ok()?;
    unsafe {
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).ok()?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok()?;
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok()?;
    }
    let units: Vec<u16> = glyph.encode_utf16().collect();
    let line = unsafe { engine.factory.CreateTextLayout(&units, &format, size as f32, size as f32) }.ok()?;
    let mut pixels = vec![0u32; (size * size) as usize];
    let ink = Rgb(128, 128, 136);
    let mut target = Target {
        factory: engine.factory.clone(),
        bits: pixels.as_mut_ptr(),
        width: size,
        clip: RECT { left: 0, top: 0, right: size, bottom: size },
        ink,
        marked_ink: ink,
        weights: weights_for(ink),
        coverage: Vec::new(),
        in_colour: true,
    };
    let context = &mut target as *mut Target as *const core::ffi::c_void;
    unsafe { line.Draw(Some(context), &engine.renderer, 0.0, 0.0) }.ok()?;
    Some(pixels.iter().flat_map(|pixel| pixel.to_le_bytes()).collect())
}

/// Adds one layer of letters, each pixel by how much of it they cover.
fn blend_run(target: &mut Target, run: *const DWRITE_GLYPH_RUN, measuring: DWRITE_MEASURING_MODE, x: f32, y: f32, ink: Rgb, opacity: f32, curve: Option<[u8; 256]>) -> Result<()> {
    let (mode, fit) = if curve.is_some() {
        (DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, DWRITE_GRID_FIT_MODE_ENABLED)
    } else {
        (DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, DWRITE_GRID_FIT_MODE_DISABLED)
    };
    let analysis = unsafe { target.factory.CreateGlyphRunAnalysis(run, None, mode, measuring, fit, DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE, x, y)? };
    let bounds = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)? };
    let area = RECT {
        left: bounds.left.max(target.clip.left),
        top: bounds.top.max(target.clip.top),
        right: bounds.right.min(target.clip.right),
        bottom: bounds.bottom.min(target.clip.bottom),
    };
    if area.left >= area.right || area.top >= area.bottom {
        return Ok(());
    }
    let span = (area.right - area.left) as usize;
    let rows = (area.bottom - area.top) as usize;
    target.coverage.resize(span * rows, 0);
    unsafe { analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &area, &mut target.coverage)? };
    for row in 0..rows {
        let start = (area.top as usize + row) * target.width as usize + area.left as usize;
        let pixels = unsafe { std::slice::from_raw_parts_mut(target.bits.add(start), span) };
        for (pixel, &amount) in pixels.iter_mut().zip(&target.coverage[row * span..(row + 1) * span]) {
            if amount != 0 {
                let amount = match &curve {
                    Some(table) => table[amount as usize],
                    None => (amount as f32 * opacity) as u8,
                };
                *pixel = blend_coverage(*pixel, ink, amount);
            }
        }
    }
    Ok(())
}

/// Blending straight in screen values makes light letters on dark look thin, and dark letters on
/// light look heavy. This bends the coverage to even that out, as DirectWrite's own drawing does.
fn weights_for(ink: Rgb) -> [u8; 256] {
    let brightness = (ink.0 as f32 * 0.3 + ink.1 as f32 * 0.59 + ink.2 as f32 * 0.11) / 255.0;
    let power = 1.08 - 0.34 * brightness;
    let mut table = [0u8; 256];
    for (value, slot) in table.iter_mut().enumerate() {
        *slot = ((value as f32 / 255.0).powf(power) * 255.0 + 0.5) as u8;
    }
    table
}

#[implement(IDWriteTextRenderer)]
struct Sink;

#[allow(non_snake_case)]
impl IDWritePixelSnapping_Impl for Sink_Impl {
    fn IsPixelSnappingDisabled(&self, _context: *const core::ffi::c_void) -> Result<BOOL> {
        // Keeps each line's baseline on a whole pixel, which keeps its flat edges sharp.
        Ok(BOOL(0))
    }

    fn GetCurrentTransform(&self, _context: *const core::ffi::c_void, transform: *mut DWRITE_MATRIX) -> Result<()> {
        unsafe { *transform = DWRITE_MATRIX { m11: 1.0, m12: 0.0, m21: 0.0, m22: 1.0, dx: 0.0, dy: 0.0 } };
        Ok(())
    }

    fn GetPixelsPerDip(&self, _context: *const core::ffi::c_void) -> Result<f32> {
        // Sizes are already in pixels.
        Ok(1.0)
    }
}

#[allow(non_snake_case)]
impl IDWriteTextRenderer_Impl for Sink_Impl {
    fn DrawGlyphRun(&self, context: *const core::ffi::c_void, x: f32, y: f32, measuring: DWRITE_MEASURING_MODE, run: *const DWRITE_GLYPH_RUN, _description: *const DWRITE_GLYPH_RUN_DESCRIPTION, effect: Ref<IUnknown>) -> Result<()> {
        let target = unsafe { &mut *(context as *mut Target) };
        if target.in_colour {
            // Each emoji arrives as stacked flat layers, each with its own colour.
            let layers = unsafe { target.factory.TranslateColorGlyphRun(x, y, run, None, measuring, None, 0) };
            if let Ok(layers) = layers {
                while unsafe { layers.MoveNext() }.is_ok_and(|more| more.as_bool()) {
                    let layer = unsafe { &*layers.GetCurrentRun()? };
                    let (ink, opacity) = if layer.paletteIndex == 0xFFFF {
                        (target.ink, 1.0)
                    } else {
                        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        (Rgb(channel(layer.runColor.r), channel(layer.runColor.g), channel(layer.runColor.b)), layer.runColor.a)
                    };
                    blend_run(target, &layer.glyphRun, measuring, layer.baselineOriginX, layer.baselineOriginY, ink, opacity, None)?;
                }
                return Ok(());
            }
            // A symbol with no colours of its own.
            let ink = target.ink;
            return blend_run(target, run, measuring, x, y, ink, 1.0, None);
        }
        let ink = if effect.is_some() { target.marked_ink } else { target.ink };
        let curve = target.weights;
        blend_run(target, run, measuring, x, y, ink, 1.0, Some(curve))
    }

    fn DrawUnderline(&self, _context: *const core::ffi::c_void, _x: f32, _y: f32, _underline: *const DWRITE_UNDERLINE, _effect: Ref<IUnknown>) -> Result<()> {
        Ok(())
    }

    fn DrawStrikethrough(&self, _context: *const core::ffi::c_void, _x: f32, _y: f32, _strikethrough: *const DWRITE_STRIKETHROUGH, _effect: Ref<IUnknown>) -> Result<()> {
        Ok(())
    }

    /// The ellipsis arrives this way, and draws itself back through this same object.
    fn DrawInlineObject(&self, context: *const core::ffi::c_void, x: f32, y: f32, object: Ref<IDWriteInlineObject>, sideways: BOOL, right_to_left: BOOL, _effect: Ref<IUnknown>) -> Result<()> {
        let Some(engine) = engine() else { return Ok(()) };
        let Some(object) = object.as_ref() else { return Ok(()) };
        unsafe { object.Draw(Some(context), &engine.renderer, x, y, sideways.as_bool(), right_to_left.as_bool(), None) }
    }
}

/// Which letters of `title` a search for `query` found, as UTF-16 stretches, so they can stand out.
/// A word start beats the middle of a word, and initials count, so `vsc` finds Visual Studio Code.
/// Nothing is marked when the title isn't what matched, such as a keyword or a corrected typo.
pub fn found_spans(title: &str, query: &str) -> Vec<Span> {
    let query: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace()).collect();
    if query.is_empty() {
        return Vec::new();
    }
    // Letters of the title, lower case, each with where it sits in UTF-16.
    let mut letters: Vec<(char, u32, u32, bool)> = Vec::new();
    let (mut at, mut after_gap) = (0u32, true);
    for c in title.chars() {
        let length = c.len_utf16() as u32;
        if c.is_whitespace() || c == '-' || c == '_' || c == '.' {
            after_gap = true;
        } else {
            let lower = c.to_lowercase().next().unwrap_or(c);
            letters.push((lower, at, length, after_gap || c.is_uppercase()));
            after_gap = false;
        }
        at += length;
    }
    let matches_at = |start: usize| start + query.len() <= letters.len() && query.iter().enumerate().all(|(i, c)| letters[start + i].0 == *c);
    let run = (0..letters.len())
        .filter(|&start| matches_at(start))
        .max_by_key(|&start| (letters[start].3, std::cmp::Reverse(start)));
    let chosen: Vec<usize> = if let Some(start) = run {
        (start..start + query.len()).collect()
    } else {
        // Initials, in order, and every query letter has to land on one.
        let mut picked = Vec::new();
        let mut wanted = query.iter();
        let mut next = wanted.next();
        for (index, letter) in letters.iter().enumerate() {
            if letter.3 && Some(&letter.0) == next {
                picked.push(index);
                next = wanted.next();
            }
        }
        if next.is_some() {
            return Vec::new();
        }
        picked
    };
    let mut spans: Vec<Span> = Vec::new();
    for index in chosen {
        let (_, start, length, _) = letters[index];
        match spans.last_mut() {
            Some((first, total)) if *first + *total == start => *total += length,
            _ => spans.push((start, length)),
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face() -> Face {
        Face::ui(14.0, 14.0, 400).expect("a font")
    }

    #[test]
    fn the_suite_typeface_is_geist_in_both_kinds_of_text() {
        assert_eq!(crate::gfx::ui_face(), crate::gfx::GEIST_FACE, "GDI, for Settings and forms");
        assert!(engine().is_some_and(|engine| engine.geist.is_some()), "DirectWrite, for the launcher");
        // A heavier weight is wider, so the weight axis really reaches the font.
        let (light, heavy) = (Face::ui(14.0, 14.0, 400).unwrap(), Face::ui(14.0, 14.0, 700).unwrap());
        assert!(width("Orbit launcher", &heavy) > width("Orbit launcher", &light));
    }

    fn inked(buffer: &Backbuffer) -> usize {
        let (bits, width, height) = buffer.raw();
        let pixels = unsafe { std::slice::from_raw_parts(bits, (width * height) as usize) };
        pixels.iter().filter(|pixel| **pixel != 0).count()
    }

    #[test]
    fn text_draws_and_measures() {
        let buffer = Backbuffer::new(200, 30).unwrap();
        let rect = RECT { left: 0, top: 0, right: 200, bottom: 30 };
        draw(&buffer, "Remote Desktop", &face(), Rgb(255, 255, 255), rect, Align::Left);
        assert!(inked(&buffer) > 50);
        let (narrow, wide) = (width("il", &face()), width("Remote Desktop", &face()));
        assert!(narrow > 0 && wide > narrow * 3, "{narrow} {wide}");
    }

    #[test]
    fn empty_text_draws_nothing() {
        let buffer = Backbuffer::new(40, 20).unwrap();
        let rect = RECT { left: 0, top: 0, right: 40, bottom: 20 };
        draw(&buffer, "", &face(), Rgb(255, 255, 255), rect, Align::Left);
        draw_clipped(&buffer, "", &face(), Rgb(255, 255, 255), 0, rect);
        assert_eq!(inked(&buffer), 0);
        assert_eq!(width("", &face()), 0);
    }

    #[test]
    fn nothing_lands_outside_the_box() {
        let buffer = Backbuffer::new(100, 30).unwrap();
        let clip = RECT { left: 20, top: 5, right: 40, bottom: 25 };
        draw_clipped(&buffer, "WWWWWWWWWWWWWW", &face(), Rgb(255, 255, 255), -30, clip);
        let (bits, width, height) = buffer.raw();
        let pixels = unsafe { std::slice::from_raw_parts(bits, (width * height) as usize) };
        for (index, pixel) in pixels.iter().enumerate() {
            let (x, y) = (index as i32 % width, index as i32 / width);
            let inside = x >= 20 && x < 40 && y >= 5 && y < 25;
            assert!(inside || *pixel == 0, "ink at {x},{y}");
        }
        assert!(inked(&buffer) > 0);
    }

    #[test]
    fn a_long_line_is_cut_with_an_ellipsis_and_stays_in_its_box() {
        let buffer = Backbuffer::new(120, 30).unwrap();
        let rect = RECT { left: 10, top: 0, right: 80, bottom: 30 };
        draw(&buffer, &"A very long title ".repeat(20), &face(), Rgb(255, 255, 255), rect, Align::Left);
        let (bits, width, height) = buffer.raw();
        let pixels = unsafe { std::slice::from_raw_parts(bits, (width * height) as usize) };
        assert!(pixels.iter().enumerate().all(|(i, p)| *p == 0 || (i as i32 % width) < 80));
        assert!(inked(&buffer) > 0);
    }

    #[test]
    fn wrapped_text_uses_several_lines_and_stays_in_its_box() {
        let buffer = Backbuffer::new(120, 200).unwrap();
        let rect = RECT { left: 10, top: 10, right: 90, bottom: 120 };
        draw_wrapped(&buffer, &format!("{} {}", "words that wrap ".repeat(30), "x".repeat(200)), &face(), Rgb(255, 255, 255), rect);
        let (bits, width, height) = buffer.raw();
        let pixels = unsafe { std::slice::from_raw_parts(bits, (width * height) as usize) };
        let mut inked_rows = std::collections::BTreeSet::new();
        for (index, pixel) in pixels.iter().enumerate() {
            let (x, y) = (index as i32 % width, index as i32 / width);
            if *pixel != 0 {
                assert!(x >= 10 && x < 90 && y >= 10 && y < 120, "ink at {x},{y}");
                inked_rows.insert(y);
            }
        }
        assert!(inked_rows.iter().any(|&y| y > 60), "it carried on to later lines");
    }

    #[test]
    fn line_breaks_stay_on_one_line_and_keep_their_positions() {
        assert_eq!(one_line("a\r\nb\tc"), "a  b c".encode_utf16().collect::<Vec<_>>());
    }

    #[test]
    fn right_to_left_and_other_scripts_dont_fail() {
        let buffer = Backbuffer::new(200, 30).unwrap();
        let rect = RECT { left: 0, top: 0, right: 200, bottom: 30 };
        for text in ["مرحبا بالعالم", "日本語のテキスト", "🙂 smile"] {
            draw(&buffer, text, &face(), Rgb(255, 255, 255), rect, Align::Left);
        }
        assert!(inked(&buffer) > 0);
    }

    #[test]
    fn found_letters_prefer_a_word_start() {
        assert_eq!(found_spans("Calculator", "calc"), [(0, 4)]);
        assert_eq!(found_spans("Remote Desktop", "desk"), [(7, 4)]);
        assert_eq!(found_spans("Mesh Desk", "es"), [(1, 2)], "no word starts with it, so the first");
        assert_eq!(found_spans("Esther Mesh", "es"), [(0, 2)]);
        assert_eq!(found_spans("Mesh Espresso", "es"), [(5, 2)], "the word start, not the first");
    }

    #[test]
    fn found_letters_include_initials() {
        assert_eq!(found_spans("Visual Studio Code", "vsc"), [(0, 1), (7, 1), (14, 1)]);
        assert_eq!(found_spans("OneDrive", "od"), [(0, 1), (3, 1)]);
    }

    #[test]
    fn nothing_is_marked_when_the_title_isnt_what_matched() {
        assert_eq!(found_spans("Remote Desktop Connection", "test"), []);
        assert_eq!(found_spans("Zen", "zne"), []);
        assert_eq!(found_spans("Zen", "  "), []);
    }

    #[test]
    fn found_letters_count_utf16_and_ignore_case() {
        assert_eq!(found_spans("🙂 ZEN", "zen"), [(3, 3)]);
    }

    #[test]
    fn light_ink_gets_more_weight_than_dark() {
        let (light, dark) = (weights_for(Rgb(255, 255, 255)), weights_for(Rgb(0, 0, 0)));
        assert!(light[128] > 128 && dark[128] < 128);
        assert_eq!((light[0], light[255]), (0, 255));
        assert_eq!((dark[0], dark[255]), (0, 255));
    }
}
