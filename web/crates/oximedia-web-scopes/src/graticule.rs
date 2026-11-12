// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Broadcast graticule overlays and their labels.
//!
//! Ported from the native `oximedia-scopes` `render.rs` graticule functions and
//! extended so the labels actually render (the upstream 5x7 font dropped every
//! letter — see [`crate::font`]). Everything draws onto a borrowed
//! [`CanvasMut`]; no allocation, no panics.

use crate::canvas::{CanvasMut, Color, GRATICULE};
use crate::font;

const WHITE: Color = [255, 255, 255, 255];
const RED: Color = [255, 0, 0, 255];
const GREEN: Color = [0, 255, 0, 255];
const BLUE: Color = [0, 0, 255, 255];
const YELLOW: Color = [255, 255, 0, 255];
const CYAN: Color = [0, 255, 255, 255];
const MAGENTA: Color = [255, 0, 255, 255];
/// Warm tint for the skin-tone / +I axis line.
const SKIN: Color = [255, 200, 150, 160];

/// Draws the waveform IRE graticule (0/10/50/75/100 IRE horizontals, quarter
/// verticals) and, when `labels`, the IRE numbers down the left edge.
pub fn waveform(canvas: &mut CanvasMut<'_>, labels: bool) {
    let (w, h) = (canvas.width(), canvas.height());
    if w == 0 || h == 0 {
        return;
    }
    for &ire in &[0u32, 10, 50, 75, 100] {
        let y = h.saturating_sub(ire * h / 100).min(h - 1);
        canvas.draw_hline(0, w - 1, y, GRATICULE);
    }
    for i in 1..4 {
        canvas.draw_vline(w * i / 4, 0, h - 1, GRATICULE);
    }
    if labels {
        for &(ire, text) in &[(100u32, "100"), (75, "75"), (50, "50"), (0, "0")] {
            let y = h.saturating_sub(ire * h / 100);
            let ly = y.saturating_sub(font::GLYPH_HEIGHT).min(h.saturating_sub(1));
            canvas.draw_text(2, ly, text, WHITE);
        }
    }
}

/// Draws a parade graticule: IRE horizontals, `sections - 1` vertical
/// separators, and (when `labels`) each pane's caption centred at the top.
pub fn parade(canvas: &mut CanvasMut<'_>, sections: u32, captions: &[&str], labels: bool) {
    let (w, h) = (canvas.width(), canvas.height());
    if w == 0 || h == 0 || sections == 0 {
        return;
    }
    for &ire in &[0u32, 25, 50, 75, 100] {
        let y = h.saturating_sub(ire * h / 100).min(h - 1);
        canvas.draw_hline(0, w - 1, y, GRATICULE);
    }
    let section_w = w / sections;
    if section_w == 0 {
        return;
    }
    for i in 1..sections {
        canvas.draw_vline(section_w * i, 0, h - 1, GRATICULE);
    }
    if labels {
        for (i, caption) in captions.iter().enumerate().take(sections as usize) {
            let centre = section_w * i as u32 + section_w / 2;
            let tx = centre.saturating_sub(font::text_width(caption) / 2);
            canvas.draw_text(tx, 2, caption, WHITE);
        }
    }
}

/// Draws the histogram graticule: quarter horizontals and vertical markers at
/// luminance 0 / 16 / 128 / 235 / 255.
pub fn histogram(canvas: &mut CanvasMut<'_>) {
    let (w, h) = (canvas.width(), canvas.height());
    if w == 0 || h == 0 {
        return;
    }
    for i in 1..=4 {
        let y = (h * i / 4).min(h);
        canvas.draw_hline(0, w - 1, h - y, GRATICULE);
    }
    for &level in &[0u32, 16, 128, 235, 255] {
        canvas.draw_vline((level * w / 255).min(w - 1), 0, h - 1, GRATICULE);
    }
}

/// Radius of the outermost graticule ring, in pixels.
#[must_use]
pub fn vectorscope_radius(w: u32, h: u32) -> f32 {
    (w.min(h) / 2).saturating_sub(10) as f32
}

/// Draws the vectorscope graticule: crosshair, four saturation rings, the six
/// SMPTE 75 % primary/secondary target boxes (labelled R Y G C B M when
/// `labels`), the +Q axis, and — when `skin_tone` — the 123 deg skin-tone /
/// +I line (labelled I).
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
pub fn vectorscope(canvas: &mut CanvasMut<'_>, skin_tone: bool, labels: bool) {
    let (w, h) = (canvas.width(), canvas.height());
    if w == 0 || h == 0 {
        return;
    }
    let (cx, cy) = (w / 2, h / 2);
    let max_radius = vectorscope_radius(w, h);

    canvas.draw_hline(0, w - 1, cy, GRATICULE);
    canvas.draw_vline(cx, 0, h - 1, GRATICULE);

    let max_r = max_radius as u32;
    for i in 1..=4 {
        canvas.draw_circle(cx, cy, max_r * i / 4, GRATICULE);
    }

    let targets = [
        (104.0_f32, RED, "R"),
        (168.0, YELLOW, "Y"),
        (241.0, GREEN, "G"),
        (284.0, CYAN, "C"),
        (348.0, BLUE, "B"),
        (61.0, MAGENTA, "M"),
    ];
    let r75 = max_radius * 0.75;
    for (angle, color, label) in targets {
        let rad = angle.to_radians();
        let x = (cx as f32 + rad.cos() * r75) as i64;
        let y = (cy as f32 - rad.sin() * r75) as i64;
        if x >= 2 && y >= 2 && (x as u32) < w - 2 && (y as u32) < h - 2 {
            canvas.draw_rect(x as u32 - 2, y as u32 - 2, 5, 5, color);
            if labels {
                let lr = max_radius * 0.88;
                let lx = (cx as f32 + rad.cos() * lr) as i64;
                let ly = (cy as f32 - rad.sin() * lr) as i64;
                if lx >= 0 && ly >= 0 {
                    canvas.draw_text(lx as u32, ly as u32, label, WHITE);
                }
            }
        }
    }

    // +Q axis (33 deg): a short broadcast reference chroma axis.
    axis_line(canvas, cx, cy, 33.0, max_radius, GRATICULE, labels.then_some("Q"));

    if skin_tone {
        // +I / skin-tone line at 123 deg.
        axis_line(canvas, cx, cy, 123.0, max_radius, SKIN, labels.then_some("I"));
    }
}

#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
fn axis_line(
    canvas: &mut CanvasMut<'_>,
    cx: u32,
    cy: u32,
    angle_deg: f32,
    radius: f32,
    color: Color,
    label: Option<&str>,
) {
    let rad = angle_deg.to_radians();
    let x = (cx as f32 + rad.cos() * radius) as i64;
    let y = (cy as f32 - rad.sin() * radius) as i64;
    if x >= 0 && y >= 0 {
        canvas.draw_line(cx, cy, x as u32, y as u32, color);
        if let Some(text) = label {
            canvas.draw_text(x as u32, y as u32, text, WHITE);
        }
    }
}
