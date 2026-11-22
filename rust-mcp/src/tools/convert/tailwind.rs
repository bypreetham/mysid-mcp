//! Tailwind class names -> a `Style` (the styling half of the IR).
//!
//! Mobile first, like Tailwind: unprefixed classes apply; `sm:`/`md:`/`lg:`/`hover:`/`focus:` and the other variants do not
//! (a phone is narrower than every breakpoint and has no hover), except `disabled:opacity-*` which is kept for buttons.

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f64,
}

impl Rgba {
    pub const fn rgb(v: u32) -> Rgba {
        Rgba { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8, a: 1.0 }
    }

    /// Dart: `Color(0xFF2563EB)` or `Color(0x802563EB)`.
    pub fn dart(&self) -> String {
        let alpha = (self.a * 255.0).round().clamp(0.0, 255.0) as u8;
        if self.r == 0 && self.g == 0 && self.b == 0 && alpha == 0 {
            return "Colors.transparent".to_string();
        }
        if alpha == 255 && self.r == 255 && self.g == 255 && self.b == 255 {
            return "Colors.white".to_string();
        }
        format!("Color(0x{:02X}{:02X}{:02X}{:02X})", alpha, self.r, self.g, self.b)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Px(f64),
    Full,
    Screen,
    Frac(f64),
    Auto,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gradient {
    pub dir: String,
    pub from: Option<Rgba>,
    pub via: Option<Rgba>,
    pub to: Option<Rgba>,
}

#[derive(Clone, Debug, Default)]
pub struct Style {
    pub hidden: bool,
    pub flex: bool,
    pub inline_flex: bool,
    pub grid: bool,
    pub col: bool,
    pub wrap: bool,
    pub justify: Option<String>,
    pub items: Option<String>,
    pub self_align: Option<String>,
    pub gap_x: Option<f64>,
    pub gap_y: Option<f64>,
    pub space_x: Option<f64>,
    pub space_y: Option<f64>,
    pub grid_cols: Option<u32>,
    pub flex_1: bool,
    pub grow: bool,
    pub w: Option<Size>,
    pub h: Option<Size>,
    pub min_w: Option<Size>,
    pub min_h: Option<Size>,
    pub max_w: Option<Size>,
    pub max_h: Option<Size>,
    /// top, right, bottom, left
    pub pad: [Option<f64>; 4],
    pub mar: [Option<f64>; 4],
    pub mx_auto: bool,
    pub bg: Option<Rgba>,
    pub gradient: Option<Gradient>,
    pub color: Option<Rgba>,
    pub size: Option<f64>,
    pub leading: Option<f64>,
    pub weight: Option<u16>,
    pub align: Option<String>,
    pub italic: bool,
    pub underline: bool,
    pub line_through: bool,
    pub uppercase: bool,
    pub lowercase: bool,
    pub capitalize: bool,
    pub truncate: bool,
    pub line_clamp: Option<u32>,
    pub tracking: Option<f64>,
    pub nowrap: bool,
    pub border: [Option<f64>; 4],
    pub border_color: Option<Rgba>,
    pub dashed: bool,
    /// top-left, top-right, bottom-right, bottom-left
    pub radius: [Option<f64>; 4],
    pub shadow: Option<String>,
    pub opacity: Option<f64>,
    pub opacity_disabled: Option<f64>,
    pub overflow_hidden: bool,
    pub scroll_x: bool,
    pub scroll_y: bool,
    pub position: Option<String>,
    /// top, right, bottom, left
    pub inset: [Option<Size>; 4],
    pub translate_x_half: Option<f64>,
    pub translate_y_half: Option<f64>,
    pub z: Option<i32>,
    pub spin: bool,
    pub pulse: bool,
    pub divide_y: Option<Rgba>,
    pub divide_x: Option<Rgba>,
    pub ring: Option<(f64, Option<Rgba>)>,
    pub aspect: Option<f64>,
    pub object_fit: Option<String>,
    pub container: bool,
    pub fill: Option<Rgba>,
    /// classes this table does not know (reported, never fatal)
    pub unknown: Vec<String>,
}

const PALETTE: &[(&str, &str)] = &[
    ("slate", "f8fafc,f1f5f9,e2e8f0,cbd5e1,94a3b8,64748b,475569,334155,1e293b,0f172a"),
    ("gray", "f9fafb,f3f4f6,e5e7eb,d1d5db,9ca3af,6b7280,4b5563,374151,1f2937,111827"),
    ("zinc", "fafafa,f4f4f5,e4e4e7,d4d4d8,a1a1aa,71717a,52525b,3f3f46,27272a,18181b"),
    ("neutral", "fafafa,f5f5f5,e5e5e5,d4d4d4,a3a3a3,737373,525252,404040,262626,171717"),
    ("stone", "fafaf9,f5f5f4,e7e5e4,d6d3d1,a8a29e,78716c,57534e,44403c,292524,1c1917"),
    ("red", "fef2f2,fee2e2,fecaca,fca5a5,f87171,ef4444,dc2626,b91c1c,991b1b,7f1d1d"),
    ("orange", "fff7ed,ffedd5,fed7aa,fdba74,fb923c,f97316,ea580c,c2410c,9a3412,7c2d12"),
    ("amber", "fffbeb,fef3c7,fde68a,fcd34d,fbbf24,f59e0b,d97706,b45309,92400e,78350f"),
    ("yellow", "fefce8,fef9c3,fef08a,fde047,facc15,eab308,ca8a04,a16207,854d0e,713f12"),
    ("lime", "f7fee7,ecfccb,d9f99d,bef264,a3e635,84cc16,65a30d,4d7c0f,3f6212,365314"),
    ("green", "f0fdf4,dcfce7,bbf7d0,86efac,4ade80,22c55e,16a34a,15803d,166534,14532d"),
    ("emerald", "ecfdf5,d1fae5,a7f3d0,6ee7b7,34d399,10b981,059669,047857,065f46,064e3b"),
    ("teal", "f0fdfa,ccfbf1,99f6e4,5eead4,2dd4bf,14b8a6,0d9488,0f766e,115e59,134e4a"),
    ("cyan", "ecfeff,cffafe,a5f3fc,67e8f9,22d3ee,06b6d4,0891b2,0e7490,155e75,164e63"),
    ("sky", "f0f9ff,e0f2fe,bae6fd,7dd3fc,38bdf8,0ea5e9,0284c7,0369a1,075985,0c4a6e"),
    ("blue", "eff6ff,dbeafe,bfdbfe,93c5fd,60a5fa,3b82f6,2563eb,1d4ed8,1e40af,1e3a8a"),
    ("indigo", "eef2ff,e0e7ff,c7d2fe,a5b4fc,818cf8,6366f1,4f46e5,4338ca,3730a3,312e81"),
    ("violet", "f5f3ff,ede9fe,ddd6fe,c4b5fd,a78bfa,8b5cf6,7c3aed,6d28d9,5b21b6,4c1d95"),
    ("purple", "faf5ff,f3e8ff,e9d5ff,d8b4fe,c084fc,a855f7,9333ea,7e22ce,6b21a8,581c87"),
    ("fuchsia", "fdf4ff,fae8ff,f5d0fe,f0abfc,e879f9,d946ef,c026d3,a21caf,86198f,701a75"),
    ("pink", "fdf2f8,fce7f3,fbcfe8,f9a8d4,f472b6,ec4899,db2777,be185d,9d174d,831843"),
    ("rose", "fff1f2,ffe4e6,fecdd3,fda4af,fb7185,f43f5e,e11d48,be123c,9f1239,881337"),
];

/// The palette as Dart map entries (`'blue-600': 0xFF2563EB`), for the runtime resolver that ships with the output.
pub fn palette_dart() -> String {
    let shades = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900];
    let mut out = Vec::new();
    for (name, hexes) in PALETTE {
        for (shade, hex) in shades.iter().zip(hexes.split(',')) {
            out.push(format!("  '{}-{}': 0xFF{},", name, shade, hex.to_uppercase()));
        }
    }
    out.join("\n")
}

fn named_color(name: &str) -> Option<Rgba> {
    match name {
        "white" => return Some(Rgba::rgb(0xFFFFFF)),
        "black" => return Some(Rgba::rgb(0x000000)),
        "transparent" => return Some(Rgba { r: 0, g: 0, b: 0, a: 0.0 }),
        "current" | "inherit" => return None,
        _ => {}
    }
    let (base, shade) = name.rsplit_once('-')?;
    let idx = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900].iter().position(|s| s.to_string() == shade)?;
    let hexes = PALETTE.iter().find(|(n, _)| *n == base)?.1;
    let hex = hexes.split(',').nth(idx)?;
    u32::from_str_radix(hex, 16).ok().map(Rgba::rgb)
}

/// `blue-600`, `white/50`, `[#ff00aa]`, `blue-600/20`.
pub fn color(spec: &str) -> Option<Rgba> {
    let (name, alpha) = match spec.split_once('/') {
        Some((n, a)) => (n, a.parse::<f64>().ok().map(|v| v / 100.0)),
        None => (spec, None),
    };
    let mut c = if let Some(hex) = name.strip_prefix("[#").and_then(|s| s.strip_suffix(']')) {
        let h = if hex.len() == 3 { hex.chars().flat_map(|c| [c, c]).collect::<String>() } else { hex.to_string() };
        Rgba::rgb(u32::from_str_radix(&h, 16).ok()?)
    } else {
        named_color(name)?
    };
    if let Some(a) = alpha {
        c.a = a;
    }
    Some(c)
}

/// The spacing scale: `4` -> 16 px, `0.5` -> 2 px, `px` -> 1 px, `[12px]` -> 12.
fn space(v: &str) -> Option<f64> {
    if v == "px" {
        return Some(1.0);
    }
    if let Some(a) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return a.trim_end_matches("px").parse().ok().or_else(|| a.strip_suffix("rem").and_then(|r| r.parse::<f64>().ok()).map(|r| r * 16.0));
    }
    v.parse::<f64>().ok().map(|n| n * 4.0)
}

fn size_of(v: &str) -> Option<Size> {
    match v {
        "full" => Some(Size::Full),
        "screen" | "svw" | "svh" | "dvh" => Some(Size::Screen),
        "auto" => Some(Size::Auto),
        "fit" | "min" | "max" => Some(Size::Auto),
        _ => {
            if let Some((a, b)) = v.split_once('/') {
                if let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>()) {
                    return Some(Size::Frac(a / b));
                }
            }
            space(v).map(Size::Px)
        }
    }
}

fn max_width(v: &str) -> Option<Size> {
    let px = match v {
        "xs" => 320.0,
        "sm" => 384.0,
        "md" => 448.0,
        "lg" => 512.0,
        "xl" => 576.0,
        "2xl" => 672.0,
        "3xl" => 768.0,
        "4xl" => 896.0,
        "5xl" => 1024.0,
        "6xl" => 1152.0,
        "7xl" => 1280.0,
        "prose" => 650.0,
        "none" => return Some(Size::Auto),
        "screen-sm" => 640.0,
        "screen-md" => 768.0,
        "screen-lg" => 1024.0,
        "screen-xl" => 1280.0,
        _ => return size_of(v),
    };
    Some(Size::Px(px))
}

fn radius_value(v: &str) -> Option<f64> {
    Some(match v {
        "" => 4.0,
        "none" => 0.0,
        "sm" => 2.0,
        "md" => 6.0,
        "lg" => 8.0,
        "xl" => 12.0,
        "2xl" => 16.0,
        "3xl" => 24.0,
        "full" => 9999.0,
        _ => return space(v),
    })
}

/// Sets one side or several: `t`, `r`, `b`, `l`, `x`, `y`, or none for all four.
fn sides(arr: &mut [Option<f64>; 4], which: &str, v: f64) {
    let idx: &[usize] = match which {
        "t" => &[0],
        "r" | "e" => &[1],
        "b" => &[2],
        "l" | "s" => &[3],
        "x" => &[1, 3],
        "y" => &[0, 2],
        _ => &[0, 1, 2, 3],
    };
    for &i in idx {
        arr[i] = Some(v);
    }
}

pub fn parse(classes: &str) -> Style {
    let mut s = Style::default();
    for raw in classes.split_whitespace() {
        // variants: only `disabled:opacity-N` is kept
        if let Some(rest) = raw.strip_prefix("disabled:") {
            if let Some(o) = rest.strip_prefix("opacity-").and_then(|n| n.parse::<f64>().ok()) {
                s.opacity_disabled = Some(o / 100.0);
            }
            continue;
        }
        if raw.contains(':') && !raw.starts_with('[') {
            // `[&>*]:...` style arbitrary variants and every responsive / state prefix
            let head = raw.split(':').next().unwrap_or("");
            if !head.contains('[') || head.starts_with('[') {
                continue;
            }
        }
        if !apply(&mut s, raw) {
            s.unknown.push(raw.to_string());
        }
    }
    s
}

fn apply(s: &mut Style, class: &str) -> bool {
    let (neg, c) = match class.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, class),
    };
    let sign = if neg { -1.0 } else { 1.0 };
    match c {
        "hidden" | "sr-only" | "invisible" => s.hidden = true,
        "block" | "inline-block" | "inline" | "contents" | "flow-root" | "table" => {}
        "flex" => s.flex = true,
        "inline-flex" => {
            s.flex = true;
            s.inline_flex = true;
        }
        "grid" | "inline-grid" => s.grid = true,
        "flex-col" | "flex-col-reverse" => s.col = true,
        "flex-row" | "flex-row-reverse" => s.col = false,
        "flex-wrap" | "flex-wrap-reverse" => s.wrap = true,
        "flex-nowrap" => s.wrap = false,
        "flex-1" | "flex-auto" => s.flex_1 = true,
        "flex-none" | "flex-initial" | "shrink-0" | "flex-shrink-0" | "shrink" | "flex-shrink" => {}
        "grow" | "flex-grow" => s.grow = true,
        "grow-0" | "flex-grow-0" => {}
        "relative" | "absolute" | "fixed" | "sticky" => s.position = Some(c.to_string()),
        "static" => s.position = None,
        "mx-auto" => s.mx_auto = true,
        "my-auto" | "m-auto" => s.mx_auto = true,
        "container" => s.container = true,
        "italic" => s.italic = true,
        "not-italic" => s.italic = false,
        "underline" => s.underline = true,
        "no-underline" => s.underline = false,
        "line-through" => s.line_through = true,
        "uppercase" => s.uppercase = true,
        "lowercase" => s.lowercase = true,
        "capitalize" => s.capitalize = true,
        "truncate" => s.truncate = true,
        "text-ellipsis" | "text-clip" => s.truncate = true,
        "whitespace-nowrap" => s.nowrap = true,
        "whitespace-normal" | "whitespace-pre-wrap" | "whitespace-pre-line" | "break-words" | "break-all" | "break-normal" | "whitespace-pre" => {}
        "overflow-hidden" | "overflow-clip" => s.overflow_hidden = true,
        "overflow-auto" | "overflow-scroll" | "overflow-y-auto" | "overflow-y-scroll" => s.scroll_y = true,
        "overflow-x-auto" | "overflow-x-scroll" => s.scroll_x = true,
        "overflow-visible" | "overflow-x-hidden" | "overflow-y-hidden" => {}
        "animate-spin" => s.spin = true,
        "animate-pulse" | "animate-bounce" | "animate-ping" => s.pulse = true,
        "border" => sides(&mut s.border, "", 1.0),
        "border-solid" => {}
        "border-dashed" | "border-dotted" => s.dashed = true,
        "border-none" => sides(&mut s.border, "", 0.0),
        "outline-none" | "outline" | "select-none" | "select-all" | "pointer-events-none" | "pointer-events-auto" | "transition" | "transition-all" | "transition-colors"
        | "transition-opacity" | "transition-shadow" | "transition-transform" | "transform" | "resize-none" | "resize" | "appearance-none" | "backdrop-blur" | "backdrop-blur-sm"
        | "list-none" | "list-disc" | "list-decimal" | "list-inside" | "antialiased" | "subpixel-antialiased" | "align-middle" | "align-top" | "align-bottom" | "table-auto"
        | "border-collapse" | "no-scrollbar" | "blur" | "filter" | "sticky-top" | "ring-offset-2" | "ring-inset" | "shadow-inner" | "touch-none" | "outline-hidden" => {}
        "shadow" => s.shadow = Some("base".to_string()),
        "ring" => s.ring = Some((3.0, None)),
        "min-h-screen" => s.min_h = Some(Size::Screen),
        "w-screen" => s.w = Some(Size::Screen),
        "h-screen" => s.h = Some(Size::Screen),
        "inset-0" => s.inset = [Some(Size::Px(0.0)); 4],
        "inset-x-0" => {
            s.inset[1] = Some(Size::Px(0.0));
            s.inset[3] = Some(Size::Px(0.0));
        }
        "inset-y-0" => {
            s.inset[0] = Some(Size::Px(0.0));
            s.inset[2] = Some(Size::Px(0.0));
        }
        "text-left" | "text-start" => s.align = Some("left".to_string()),
        "text-center" => s.align = Some("center".to_string()),
        "text-right" | "text-end" => s.align = Some("right".to_string()),
        "text-justify" => s.align = Some("justify".to_string()),
        "leading-none" => s.leading = Some(1.0),
        "leading-tight" => s.leading = Some(1.25),
        "leading-snug" => s.leading = Some(1.375),
        "leading-normal" => s.leading = Some(1.5),
        "leading-relaxed" => s.leading = Some(1.625),
        "leading-loose" => s.leading = Some(2.0),
        "tracking-tighter" => s.tracking = Some(-0.8),
        "tracking-tight" => s.tracking = Some(-0.4),
        "tracking-normal" => s.tracking = Some(0.0),
        "tracking-wide" => s.tracking = Some(0.4),
        "tracking-wider" => s.tracking = Some(0.8),
        "tracking-widest" => s.tracking = Some(1.6),
        "object-cover" | "object-contain" | "object-fill" | "object-center" => s.object_fit = Some(c.trim_start_matches("object-").to_string()),
        "aspect-square" => s.aspect = Some(1.0),
        "aspect-video" => s.aspect = Some(16.0 / 9.0),
        "divide-y" => s.divide_y = Some(Rgba::rgb(0xE5E7EB)),
        "divide-x" => s.divide_x = Some(Rgba::rgb(0xE5E7EB)),
        _ => return apply_valued(s, c, sign),
    }
    true
}

fn apply_valued(s: &mut Style, c: &str, sign: f64) -> bool {
    // font weights and sizes
    if let Some(w) = c.strip_prefix("font-") {
        let weight = match w {
            "thin" => 100,
            "extralight" => 200,
            "light" => 300,
            "normal" => 400,
            "medium" => 500,
            "semibold" => 600,
            "bold" => 700,
            "extrabold" => 800,
            "black" => 900,
            // font families
            "sans" | "serif" | "mono" => return true,
            _ => return false,
        };
        s.weight = Some(weight);
        return true;
    }
    if let Some(v) = c.strip_prefix("text-") {
        let sizes: &[(&str, f64, f64)] = &[
            ("xs", 12.0, 16.0), ("sm", 14.0, 20.0), ("base", 16.0, 24.0), ("lg", 18.0, 28.0), ("xl", 20.0, 28.0), ("2xl", 24.0, 32.0), ("3xl", 30.0, 36.0),
            ("4xl", 36.0, 40.0), ("5xl", 48.0, 48.0), ("6xl", 60.0, 60.0), ("7xl", 72.0, 72.0), ("8xl", 96.0, 96.0), ("9xl", 128.0, 128.0),
        ];
        if let Some((_, px, lh)) = sizes.iter().find(|(n, _, _)| *n == v) {
            s.size = Some(*px);
            if s.leading.is_none() {
                s.leading = Some(lh / px);
            }
            return true;
        }
        if let Some(px) = v.strip_prefix('[').and_then(|x| x.strip_suffix("px]")).and_then(|x| x.parse::<f64>().ok()) {
            s.size = Some(px);
            return true;
        }
        if let Some(col) = color(v) {
            s.color = Some(col);
            return true;
        }
        return false;
    }
    if let Some(v) = c.strip_prefix("bg-") {
        if let Some(dir) = v.strip_prefix("gradient-to-") {
            s.gradient.get_or_insert_with(Gradient::default).dir = dir.to_string();
            return true;
        }
        if matches!(v, "cover" | "center" | "no-repeat" | "contain" | "fixed" | "repeat") {
            return true;
        }
        if let Some(col) = color(v) {
            s.bg = Some(col);
            return true;
        }
        return false;
    }
    for (prefix, slot) in [("from-", 0), ("via-", 1), ("to-", 2)] {
        if let Some(v) = c.strip_prefix(prefix) {
            let Some(col) = color(v) else { return false };
            let g = s.gradient.get_or_insert_with(Gradient::default);
            match slot {
                0 => g.from = Some(col),
                1 => g.via = Some(col),
                _ => g.to = Some(col),
            }
            return true;
        }
    }
    if let Some(v) = c.strip_prefix("fill-") {
        s.fill = color(v);
        return s.fill.is_some() || v == "none" || v == "current";
    }
    if c.starts_with("stroke-") || c.starts_with("placeholder-") || c.starts_with("accent-") || c.starts_with("caret-") || c.starts_with("decoration-") || c.starts_with("outline-") {
        return true;
    }
    if let Some(v) = c.strip_prefix("border-") {
        let (side, rest) = match v.split_once('-') {
            Some((a @ ("t" | "r" | "b" | "l" | "x" | "y"), r)) => (a, r),
            _ if matches!(v, "t" | "r" | "b" | "l" | "x" | "y") => (v, ""),
            _ => ("", v),
        };
        if rest.is_empty() {
            sides(&mut s.border, side, 1.0);
            return true;
        }
        if let Ok(w) = rest.parse::<f64>() {
            sides(&mut s.border, side, w);
            return true;
        }
        if let Some(col) = color(rest) {
            s.border_color = Some(col);
            if s.border.iter().all(|b| b.is_none()) {
                // a colour alone does not draw a border, but `border-gray-200` without `border` is a mistake worth keeping visible
            }
            return true;
        }
        return false;
    }
    if let Some(v) = c.strip_prefix("divide-") {
        if let Some(col) = color(v) {
            if s.divide_y.is_some() || s.divide_x.is_some() {
                s.divide_y = s.divide_y.map(|_| col);
                s.divide_x = s.divide_x.map(|_| col);
            } else {
                s.divide_y = Some(col);
            }
            return true;
        }
        return matches!(v, "y-reverse" | "x-reverse" | "solid" | "dashed") || v.parse::<f64>().is_ok();
    }
    if let Some(v) = c.strip_prefix("ring-") {
        if let Ok(w) = v.parse::<f64>() {
            s.ring = Some((w, s.ring.and_then(|r| r.1)));
            return true;
        }
        if let Some(col) = color(v) {
            let w = s.ring.map_or(3.0, |r| r.0);
            s.ring = Some((w, Some(col)));
            return true;
        }
        return v.starts_with("offset");
    }
    if let Some(v) = c.strip_prefix("rounded") {
        let v = v.strip_prefix('-').unwrap_or(v);
        let (corner, rest) = match v.split_once('-') {
            Some((a @ ("t" | "r" | "b" | "l" | "tl" | "tr" | "br" | "bl"), r)) => (a, r),
            _ if matches!(v, "t" | "r" | "b" | "l" | "tl" | "tr" | "br" | "bl") => (v, ""),
            _ => ("", v),
        };
        let Some(px) = radius_value(rest) else { return false };
        let idx: &[usize] = match corner {
            "t" => &[0, 1],
            "r" => &[1, 2],
            "b" => &[2, 3],
            "l" => &[3, 0],
            "tl" => &[0],
            "tr" => &[1],
            "br" => &[2],
            "bl" => &[3],
            _ => &[0, 1, 2, 3],
        };
        for &i in idx {
            s.radius[i] = Some(px);
        }
        return true;
    }
    if let Some(v) = c.strip_prefix("shadow-") {
        if matches!(v, "sm" | "md" | "lg" | "xl" | "2xl" | "none" | "inner") {
            s.shadow = if v == "none" { None } else { Some(v.to_string()) };
            return true;
        }
        return color(v).is_some();
    }
    if let Some(v) = c.strip_prefix("opacity-") {
        return v.parse::<f64>().map(|n| s.opacity = Some(n / 100.0)).is_ok();
    }
    if let Some(v) = c.strip_prefix("z-") {
        return v.parse::<i32>().map(|n| s.z = Some(n)).is_ok() || v == "auto";
    }
    if let Some(v) = c.strip_prefix("line-clamp-") {
        return v.parse::<u32>().map(|n| s.line_clamp = Some(n)).is_ok();
    }
    if let Some(v) = c.strip_prefix("grid-cols-") {
        if let Ok(n) = v.parse::<u32>() {
            s.grid_cols = Some(n);
            return true;
        }
        return false;
    }
    if c.starts_with("grid-rows-") || c.starts_with("col-span-") || c.starts_with("row-span-") || c.starts_with("col-start-") || c == "col-auto" {
        return true;
    }
    if let Some(v) = c.strip_prefix("gap-x-") {
        return space(v).map(|n| s.gap_x = Some(n)).is_some();
    }
    if let Some(v) = c.strip_prefix("gap-y-") {
        return space(v).map(|n| s.gap_y = Some(n)).is_some();
    }
    if let Some(v) = c.strip_prefix("gap-") {
        return space(v).map(|n| {
            s.gap_x = Some(n);
            s.gap_y = Some(n);
        }).is_some();
    }
    if let Some(v) = c.strip_prefix("space-x-") {
        return space(v).map(|n| s.space_x = Some(n)).is_some() || v == "reverse";
    }
    if let Some(v) = c.strip_prefix("space-y-") {
        return space(v).map(|n| s.space_y = Some(n)).is_some() || v == "reverse";
    }
    if let Some(v) = c.strip_prefix("items-") {
        s.items = Some(v.to_string());
        return true;
    }
    if let Some(v) = c.strip_prefix("justify-") {
        if v.starts_with("items") || v.starts_with("self") {
            return true;
        }
        s.justify = Some(v.to_string());
        return true;
    }
    if let Some(v) = c.strip_prefix("self-") {
        s.self_align = Some(v.to_string());
        return true;
    }
    if c.starts_with("content-") || c.starts_with("place-") || c.starts_with("order-") || c.starts_with("cursor-") || c.starts_with("duration-") || c.starts_with("ease-")
        || c.starts_with("delay-") || c.starts_with("transition-") || c.starts_with("scale-") || c.starts_with("rotate-") || c.starts_with("origin-") || c.starts_with("backdrop-")
        || c.starts_with("blur-") || c.starts_with("brightness-") || c.starts_with("sm:") || c.starts_with("touch-") || c.starts_with("snap-") || c.starts_with("scroll-")
        || c.starts_with("will-change") || c.starts_with("mix-") || c.starts_with("indent-") || c.starts_with("underline-offset") || c.starts_with("tab-") || c.starts_with("break-")
        || c.starts_with("object-") || c.starts_with("basis-")
    {
        return true;
    }
    if let Some(v) = c.strip_prefix("translate-x-") {
        if v == "1/2" {
            s.translate_x_half = Some(sign * 0.5);
        }
        return true;
    }
    if let Some(v) = c.strip_prefix("translate-y-") {
        if v == "1/2" {
            s.translate_y_half = Some(sign * 0.5);
        }
        return true;
    }
    // sizes
    let sized: &[(&str, u8)] = &[("min-w-", 0), ("min-h-", 1), ("max-w-", 2), ("max-h-", 3), ("size-", 4), ("w-", 5), ("h-", 6)];
    for (prefix, kind) in sized {
        if let Some(v) = c.strip_prefix(prefix) {
            let Some(sz) = (if *kind == 2 { max_width(v) } else { size_of(v) }) else { return false };
            match kind {
                0 => s.min_w = Some(sz),
                1 => s.min_h = Some(sz),
                2 => s.max_w = Some(sz),
                3 => s.max_h = Some(sz),
                4 => {
                    s.w = Some(sz);
                    s.h = Some(sz);
                }
                5 => s.w = Some(sz),
                _ => s.h = Some(sz),
            }
            return true;
        }
    }
    // spacing: p-, px-, pt-, m-, mx-, mt-
    for (prefix, is_pad) in [("p", true), ("m", false)] {
        if let Some(v) = c.strip_prefix(prefix) {
            let (side, val) = match v.split_once('-') {
                Some((a, b)) if matches!(a, "" | "t" | "r" | "b" | "l" | "x" | "y" | "s" | "e") => (a, b),
                _ => continue,
            };
            if val == "auto" {
                if !is_pad {
                    s.mx_auto = true;
                }
                return true;
            }
            let Some(px) = space(val) else { return false };
            sides(if is_pad { &mut s.pad } else { &mut s.mar }, side, px * sign);
            return true;
        }
    }
    // positions
    for (prefix, idx) in [("top-", 0usize), ("right-", 1), ("bottom-", 2), ("left-", 3)] {
        if let Some(v) = c.strip_prefix(prefix) {
            let Some(sz) = size_of(v) else { return false };
            s.inset[idx] = Some(match sz {
                Size::Px(p) => Size::Px(p * sign),
                other => other,
            });
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_come_from_the_palette_with_opacity() {
        assert_eq!(color("blue-600").map(|c| c.dart()), Some("Color(0xFF2563EB)".to_string()));
        assert_eq!(color("white").map(|c| c.dart()), Some("Colors.white".to_string()));
        assert_eq!(color("black/50").map(|c| c.dart()), Some("Color(0x80000000)".to_string()));
        assert!(color("nope-600").is_none());
        assert_eq!(color("[#abc]").map(|c| c.dart()), Some("Color(0xFFAABBCC)".to_string()));
    }

    #[test]
    fn layout_spacing_and_text_classes_resolve() {
        let s = parse("flex items-center justify-between gap-4 px-4 py-2 mt-6 -mb-2 text-lg font-semibold text-gray-900 rounded-lg bg-white border border-gray-200 shadow-md");
        assert!(s.flex && !s.col);
        assert_eq!((s.items.as_deref(), s.justify.as_deref(), s.gap_x), (Some("center"), Some("between"), Some(16.0)));
        assert_eq!((s.pad[1], s.pad[0], s.mar[0], s.mar[2]), (Some(16.0), Some(8.0), Some(24.0), Some(-8.0)));
        assert_eq!((s.size, s.weight), (Some(18.0), Some(600)));
        assert_eq!(s.radius, [Some(8.0); 4]);
        assert_eq!((s.border, s.shadow.as_deref()), ([Some(1.0); 4], Some("md")));
        assert!(s.unknown.is_empty(), "{:?}", s.unknown);
    }

    #[test]
    fn variants_are_ignored_except_disabled_opacity() {
        let s = parse("bg-blue-600 hover:bg-blue-700 md:grid-cols-3 focus:ring-2 disabled:opacity-50 lg:flex");
        assert!(s.bg.is_some() && s.grid_cols.is_none() && !s.flex && s.ring.is_none());
        assert_eq!(s.opacity_disabled, Some(0.5));
        assert!(s.unknown.is_empty(), "{:?}", s.unknown);
    }

    #[test]
    fn sizes_gradients_and_positions_resolve() {
        let s = parse("w-full max-w-md h-12 min-h-screen bg-gradient-to-br from-blue-50 to-indigo-100 absolute left-3 top-1/2 -translate-y-1/2 inset-0 z-10");
        assert_eq!((s.w, s.max_w, s.h, s.min_h), (Some(Size::Full), Some(Size::Px(448.0)), Some(Size::Px(48.0)), Some(Size::Screen)));
        let g = s.gradient.unwrap();
        assert_eq!((g.dir.as_str(), g.from.is_some(), g.to.is_some()), ("br", true, true));
        assert_eq!((s.position.as_deref(), s.inset[3], s.inset[0], s.z), (Some("absolute"), Some(Size::Px(0.0)), Some(Size::Px(0.0)), Some(10)));
        assert_eq!(s.translate_y_half, Some(-0.5));
    }

    #[test]
    fn unknown_classes_are_collected() {
        let s = parse("flex weird-class custom");
        assert_eq!(s.unknown, vec!["weird-class", "custom"]);
    }
}
