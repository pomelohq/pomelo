//! Lifting a text color until it reads against its background, by APCA lightness contrast (the 0.0.98G-4g
//! constants). Terminal themes ship ANSI colors that can vanish on the theme background; this keeps them legible
//! while preserving hue where possible.

use ui::Rgba;

const MAIN_TRC: f32 = 2.4;
const RED: f32 = 0.2126729;
const GREEN: f32 = 0.7151522;
const BLUE: f32 = 0.0721750;
const NORMAL_BACKGROUND: f32 = 0.56;
const NORMAL_TEXT: f32 = 0.57;
const REVERSE_TEXT: f32 = 0.62;
const REVERSE_BACKGROUND: f32 = 0.65;
const BLACK_THRESHOLD: f32 = 0.022;
const BLACK_CLAMP: f32 = 1.414;
const SCALE: f32 = 1.14;
const LOW_OFFSET: f32 = 0.027;
const DELTA_Y_MIN: f32 = 0.0005;
const LOW_CLIP: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Hsl {
    h: f32,
    s: f32,
    l: f32,
    a: f32,
}

fn to_hsl(color: Rgba) -> Hsl {
    let max = color.r.max(color.g).max(color.b);
    let min = color.r.min(color.g).min(color.b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return Hsl {
            h: 0.0,
            s: 0.0,
            l,
            a: color.a,
        };
    }
    let delta = max - min;
    let s = if l > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let h = if max == color.r {
        (color.g - color.b) / delta + if color.g < color.b { 6.0 } else { 0.0 }
    } else if max == color.g {
        (color.b - color.r) / delta + 2.0
    } else {
        (color.r - color.g) / delta + 4.0
    } / 6.0;
    Hsl {
        h,
        s,
        l,
        a: color.a,
    }
}

fn to_rgba(color: Hsl) -> Rgba {
    if color.s == 0.0 {
        return Rgba::new(color.l, color.l, color.l, color.a);
    }
    let q = if color.l < 0.5 {
        color.l * (1.0 + color.s)
    } else {
        color.l + color.s - color.l * color.s
    };
    let p = 2.0 * color.l - q;
    let channel = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    Rgba::new(
        channel(color.h + 1.0 / 3.0),
        channel(color.h),
        channel(color.h - 1.0 / 3.0),
        color.a,
    )
}

fn luminance(color: Rgba) -> f32 {
    RED * color.r.powf(MAIN_TRC) + GREEN * color.g.powf(MAIN_TRC) + BLUE * color.b.powf(MAIN_TRC)
}

fn soft_clamp(y: f32) -> f32 {
    if y > BLACK_THRESHOLD {
        y
    } else {
        y + (BLACK_THRESHOLD - y).powf(BLACK_CLAMP)
    }
}

/// Lightness contrast Lc: positive for dark text on light, negative for light text on dark.
pub fn apca_contrast(text: Rgba, background: Rgba) -> f32 {
    let text_y = soft_clamp(luminance(text));
    let background_y = soft_clamp(luminance(background));
    if (background_y - text_y).abs() < DELTA_Y_MIN {
        return 0.0;
    }
    let contrast = if background_y > text_y {
        let sapc = (background_y.powf(NORMAL_BACKGROUND) - text_y.powf(NORMAL_TEXT)) * SCALE;
        if sapc < LOW_CLIP {
            0.0
        } else {
            sapc - LOW_OFFSET
        }
    } else {
        let sapc = (background_y.powf(REVERSE_BACKGROUND) - text_y.powf(REVERSE_TEXT)) * SCALE;
        if sapc > -LOW_CLIP {
            0.0
        } else {
            sapc + LOW_OFFSET
        }
    };
    contrast * 100.0
}

fn contrast_of(color: Hsl, background: Rgba) -> f32 {
    apca_contrast(to_rgba(color), background).abs()
}

/// Binary-search lightness (toward dark on light backgrounds, light otherwise) for the least change that
/// reaches `minimum`.
fn adjust_lightness(foreground: Hsl, background: Rgba, minimum: f32) -> Hsl {
    let darker = luminance(background) > 0.5;
    let (mut low, mut high) = if darker {
        (0.0, foreground.l)
    } else {
        (foreground.l, 1.0)
    };
    let mut best = foreground.l;
    for _ in 0..20 {
        let mid = (low + high) / 2.0;
        let contrast = contrast_of(
            Hsl {
                l: mid,
                ..foreground
            },
            background,
        );
        if contrast >= minimum {
            best = mid;
            if darker {
                low = mid;
            } else {
                high = mid;
            }
        } else if darker {
            high = mid;
        } else {
            low = mid;
        }
        if (contrast - minimum).abs() < 1.0 {
            best = mid;
            break;
        }
    }
    Hsl {
        l: best,
        ..foreground
    }
}

pub fn ensure_minimum_contrast(foreground: Rgba, background: Rgba, minimum: f32) -> Rgba {
    if minimum <= 0.0 || apca_contrast(foreground, background).abs() >= minimum {
        return foreground;
    }
    let original = to_hsl(foreground);
    let lighter = adjust_lightness(original, background, minimum);
    if contrast_of(lighter, background) >= minimum {
        return to_rgba(lighter);
    }
    for saturation in [1.0, 0.8, 0.6, 0.4, 0.2, 0.0] {
        let muted = Hsl {
            s: original.s * saturation,
            ..original
        };
        let adjusted = adjust_lightness(muted, background, minimum);
        if contrast_of(adjusted, background) >= minimum {
            return to_rgba(adjusted);
        }
    }
    let black = Rgba::new(0.0, 0.0, 0.0, foreground.a);
    let white = Rgba::new(1.0, 1.0, 1.0, foreground.a);
    if apca_contrast(white, background).abs() > apca_contrast(black, background).abs() {
        white
    } else {
        black
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_polarity_and_magnitude() {
        let white = Rgba::new(1.0, 1.0, 1.0, 1.0);
        let black = Rgba::new(0.0, 0.0, 0.0, 1.0);
        assert!(apca_contrast(black, white) > 100.0);
        assert!(apca_contrast(white, black) < -100.0);
        assert_eq!(apca_contrast(white, white), 0.0);
    }

    #[test]
    fn low_contrast_text_is_lifted_and_good_text_kept() {
        let background = Rgba::hex("#282c34");
        let faint = Rgba::hex("#2e333c");
        let lifted = ensure_minimum_contrast(faint, background, 45.0);
        assert!(apca_contrast(lifted, background).abs() >= 45.0);
        let readable = Rgba::hex("#abb2bf");
        assert_eq!(
            ensure_minimum_contrast(readable, background, 45.0),
            readable
        );
    }

    #[test]
    fn hsl_round_trips() {
        for hex in ["#e06c75", "#98c379", "#61afef", "#808080"] {
            let color = Rgba::hex(hex);
            let back = to_rgba(to_hsl(color));
            assert!((back.r - color.r).abs() < 1e-4);
            assert!((back.g - color.g).abs() < 1e-4);
            assert!((back.b - color.b).abs() < 1e-4);
        }
    }
}
