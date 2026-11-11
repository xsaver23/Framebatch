//! Lightroom / Camera Raw preset import (.xmp and legacy .lrtemplate) and its approximate
//! rendering on 8-bit sRGB photos. Adobe does not publish Camera Raw's processing, so every
//! slider here is an approximation; point tone curves use the preset's own points.
//! Preset files are only parsed as data: nothing in them is ever executed.
use crate::contracts::{Grading, Preset};
use image::RgbaImage;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

const MAX_PRESET_BYTES: usize = 2 * 1024 * 1024;
/// Glow per point of negative Lightroom Clarity (Clarity -50 is Glow 30).
const CLARITY_TO_GLOW: f64 = 0.6;
const CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";

enum Raw {
    Text(String),
    List(Vec<String>),
    /// A structured setting (profile, masks) that Framebatch only reports.
    Present,
}

/// Framebatch's original Dreamy Pastel and Golden Glow presets.
const BUILTIN: [(&str, &str); 2] = [
    (
        "Framebatch_Dreamy_Pastel.xmp",
        include_str!("../presets/Framebatch_Dreamy_Pastel.xmp"),
    ),
    (
        "Framebatch_Golden_Glow.xmp",
        include_str!("../presets/Framebatch_Golden_Glow.xmp"),
    ),
];
pub fn builtins() -> Vec<Preset> {
    BUILTIN
        .iter()
        .filter_map(|(file, text)| parse(text, file).ok())
        .collect()
}
pub fn parse(text: &str, file: &str) -> Result<Preset, String> {
    if text.len() > MAX_PRESET_BYTES {
        return Err("This file is too large to be a Lightroom preset.".into());
    }
    let text = text.trim_start_matches('\u{feff}');
    let (name, settings) = if text.trim_start().starts_with('<') {
        parse_xmp(text)?
    } else {
        parse_lrtemplate(text)?
    };
    let file = file.rsplit(['\\', '/']).next().unwrap_or(file);
    let fallback = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let preset = build(
        name.map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| fallback.into()),
        file,
        &settings,
    )?;
    preset.validate()?;
    Ok(preset)
}

// ---------------------------------------------------------------------------------------
// .xmp (Lightroom 7.3+ / Camera Raw): settings are crs: attributes or simple crs: elements;
// curves and the name are rdf:Seq / rdf:Alt lists.
fn parse_xmp(text: &str) -> Result<(Option<String>, HashMap<String, Raw>), String> {
    use quick_xml::{Reader, events::Event};
    let bad = |e: &dyn std::fmt::Display| format!("This XMP file could not be read: {e}");
    let mut reader = Reader::from_str(text);
    let mut settings = HashMap::new();
    let mut prefix: Option<String> = None;
    // Open crs: element: qualified name, setting, text, list items, has nested structure.
    let mut open: Option<(String, String, String, Vec<String>, bool)> = None;
    let mut item: Option<String> = None;
    loop {
        let event = reader.read_event().map_err(|e| bad(&e))?;
        let (start, empty) = match &event {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            let mut attributes = vec![];
            for a in e.attributes() {
                let a = a.map_err(|e| bad(&e))?;
                let key = a.key.as_ref().to_owned();
                let value = a
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|e| bad(&e))?
                    .into_owned();
                if let Some(p) = key.strip_prefix("xmlns:")
                    && value == CRS
                {
                    prefix = Some(p.into());
                }
                attributes.push((key, value));
            }
            let crs = |name: &str| -> Option<String> {
                let p = prefix.as_deref()?;
                name.strip_prefix(p)?.strip_prefix(':').map(str::to_owned)
            };
            if open.is_none() {
                for (key, value) in attributes {
                    if let Some(setting) = crs(&key) {
                        settings.insert(setting, Raw::Text(value));
                    }
                }
            }
            let qname = e.name().as_ref().to_owned();
            let local = e.local_name().as_ref().to_owned();
            match &mut open {
                None => {
                    if let Some(setting) = crs(&qname) {
                        if empty {
                            settings.entry(setting).or_insert(Raw::Present);
                        } else {
                            open = Some((qname, setting, String::new(), vec![], false));
                        }
                    }
                }
                Some((_, _, _, _, nested)) => {
                    if local == "li" && !empty {
                        item = Some(String::new());
                    } else if !matches!(local.as_str(), "Seq" | "Alt" | "Bag" | "li") {
                        *nested = true;
                    }
                }
            }
            continue;
        }
        match event {
            Event::Text(t) => push_text(&mut open, &mut item, t.as_ref()),
            Event::CData(t) => push_text(&mut open, &mut item, t.as_ref()),
            Event::GeneralRef(r) => {
                let c = match r.resolve_char_ref().map_err(|e| bad(&e))? {
                    Some(c) => c,
                    None => match r.as_ref() {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        other => return Err(bad(&format!("unknown entity &{other};"))),
                    },
                };
                push_text(&mut open, &mut item, c.encode_utf8(&mut [0; 4]));
            }
            Event::End(e) => {
                if e.local_name().as_ref() == "li"
                    && let (Some(text), Some((_, _, _, items, _))) = (item.take(), &mut open)
                {
                    items.push(text.trim().into());
                } else if open.as_ref().is_some_and(|o| o.0 == e.name().as_ref()) {
                    let (_, setting, text, items, nested) = open.take().unwrap();
                    let value = if nested {
                        Raw::Present
                    } else if !items.is_empty() {
                        Raw::List(items)
                    } else {
                        Raw::Text(text.trim().into())
                    };
                    settings.entry(setting).or_insert(value);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if prefix.is_none() {
        return Err("This XMP file is not a Lightroom or Camera Raw preset.".into());
    }
    if matches!(settings.get("PresetType"), Some(Raw::Text(t)) if t == "Look") {
        return Err(
            "This file is a Lightroom profile, not a develop preset. Profiles are not supported."
                .into(),
        );
    }
    let name = match settings.get("Name") {
        Some(Raw::List(items)) => items.first().cloned(),
        Some(Raw::Text(t)) => Some(t.clone()),
        _ => None,
    };
    Ok((name, settings))
}
fn push_text(
    open: &mut Option<(String, String, String, Vec<String>, bool)>,
    item: &mut Option<String>,
    s: &str,
) {
    if let Some(text) = item {
        text.push_str(s);
    } else if let Some((_, _, text, _, _)) = open {
        text.push_str(s);
    }
}

// ---------------------------------------------------------------------------------------
// .lrtemplate (Lightroom 1–7.2): a Lua table literal `s = { title = ..., value = { settings =
// {...} } }`. Parsed with a strict data-only grammar; function calls other than a string
// argument (`ZSTR "..."`) are rejected.
#[derive(Debug)]
enum Lua {
    Num(f64),
    Str(String),
    Bool(bool),
    Table(Vec<(Option<String>, Lua)>),
}
struct LuaParser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
    items: usize,
}
impl LuaParser<'_> {
    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!(
            "This .lrtemplate file could not be read ({what} near byte {}).",
            self.i
        ))
    }
    fn skip(&mut self) {
        loop {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--[[") {
                self.i = find(self.s, self.i, b"]]").map_or(self.s.len(), |j| j + 2);
            } else if self.s[self.i..].starts_with(b"--") {
                while self.i < self.s.len() && self.s[self.i] != b'\n' {
                    self.i += 1;
                }
            } else {
                return;
            }
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        let found = self.peek() == Some(c);
        self.i += usize::from(found);
        found
    }
    fn ident(&mut self) -> Option<String> {
        self.skip();
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
        {
            self.i += 1;
        }
        let ok = self.i > start && !self.s[start].is_ascii_digit();
        if !ok {
            self.i = start;
        }
        ok.then(|| String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }
    fn string(&mut self) -> Result<String, String> {
        let quote = self.s[self.i];
        if quote == b'[' {
            let end = find(self.s, self.i + 2, b"]]")
                .ok_or("unterminated string")
                .or_else(|e| self.err(e))?;
            let text = String::from_utf8_lossy(&self.s[self.i + 2..end]).into_owned();
            self.i = end + 2;
            return Ok(text);
        }
        self.i += 1;
        let mut out = vec![];
        while let Some(&c) = self.s.get(self.i) {
            self.i += 1;
            match c {
                b'\\' => {
                    let e = *self
                        .s
                        .get(self.i)
                        .ok_or("bad escape")
                        .or_else(|e| self.err(e))?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'0'..=b'9' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.s.get(self.i) {
                                    Some(d @ b'0'..=b'9') => {
                                        v = v * 10 + u32::from(d - b'0');
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(u8::try_from(v).or_else(|_| self.err("bad escape"))?);
                        }
                        other => out.push(other),
                    }
                }
                c if c == quote => return Ok(String::from_utf8_lossy(&out).into_owned()),
                c => out.push(c),
            }
        }
        self.err("unterminated string")
    }
    fn value(&mut self) -> Result<Lua, String> {
        self.items += 1;
        if self.items > 200_000 {
            return self.err("too many values");
        }
        match self.peek() {
            Some(b'{') => {
                self.depth += 1;
                if self.depth > 32 {
                    return self.err("nested too deeply");
                }
                self.i += 1;
                let mut fields = vec![];
                while !self.eat(b'}') {
                    let save = self.i;
                    let key = if self.eat(b'[') {
                        let k = match self.value()? {
                            Lua::Str(s) => s,
                            Lua::Num(n) => n.to_string(),
                            _ => return self.err("unsupported key"),
                        };
                        if !self.eat(b']') || !self.eat(b'=') {
                            return self.err("expected ] =");
                        }
                        Some(k)
                    } else if let Some(k) = self.ident() {
                        if self.eat(b'=') {
                            Some(k)
                        } else {
                            self.i = save;
                            None
                        }
                    } else {
                        None
                    };
                    fields.push((key, self.value()?));
                    if !self.eat(b',') && !self.eat(b';') && self.peek() != Some(b'}') {
                        return self.err("expected , or }");
                    }
                }
                self.depth -= 1;
                Ok(Lua::Table(fields))
            }
            Some(b'"' | b'\'') => Ok(Lua::Str(self.string()?)),
            Some(b'[') if self.s[self.i..].starts_with(b"[[") => Ok(Lua::Str(self.string()?)),
            Some(c) if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_alphanumeric() || b".+-".contains(&self.s[self.i]))
                {
                    self.i += 1;
                }
                let t = String::from_utf8_lossy(&self.s[start..self.i]);
                t.parse().map(Lua::Num).or_else(|_| self.err("bad number"))
            }
            _ => match self.ident().as_deref() {
                Some("true") => Ok(Lua::Bool(true)),
                Some("false") => Ok(Lua::Bool(false)),
                Some("nil") => Ok(Lua::Str(String::new())),
                // Localized strings such as `ZSTR "$$$/Key=Text"`.
                Some(_) if matches!(self.peek(), Some(b'"' | b'\'')) => {
                    let s = self.string()?;
                    Ok(Lua::Str(
                        s.rsplit_once('=').map_or(s.clone(), |(_, v)| v.into()),
                    ))
                }
                _ => self.err("unsupported value"),
            },
        }
    }
}
fn find(s: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    s.get(from..)?
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|j| j + from)
}
fn field<'a>(t: &'a Lua, key: &str) -> Option<&'a Lua> {
    match t {
        Lua::Table(fields) => fields
            .iter()
            .find(|(k, _)| k.as_deref() == Some(key))
            .map(|(_, v)| v),
        _ => None,
    }
}
fn parse_lrtemplate(text: &str) -> Result<(Option<String>, HashMap<String, Raw>), String> {
    let mut p = LuaParser {
        s: text.as_bytes(),
        i: 0,
        depth: 0,
        items: 0,
    };
    if p.peek() != Some(b'{') {
        match p.ident().as_deref() {
            Some("return") => {}
            Some(_) if p.eat(b'=') => {}
            _ => return Err("This file is not a Lightroom preset (.xmp or .lrtemplate).".into()),
        }
    }
    let root = p.value()?;
    if p.peek().is_some() {
        return p.err("unexpected text after the preset");
    }
    let settings = field(&root, "value")
        .and_then(|v| field(v, "settings"))
        .ok_or("This .lrtemplate file has no develop settings.")?;
    if field(&root, "type").is_some_and(|t| !matches!(t, Lua::Str(t) if t == "Develop")) {
        return Err("This .lrtemplate file is not a develop preset.".into());
    }
    let name = match field(&root, "title") {
        Some(Lua::Str(s)) => Some(s.clone()),
        _ => None,
    };
    let mut out = HashMap::new();
    if let Lua::Table(fields) = settings {
        for (key, value) in fields {
            let Some(key) = key else { continue };
            let raw = match value {
                Lua::Num(n) => Raw::Text(n.to_string()),
                Lua::Str(s) => Raw::Text(s.clone()),
                Lua::Bool(b) => Raw::Text(if *b { "True" } else { "False" }.into()),
                // Curves are flat x, y, x, y ... lists; anything else is structured.
                Lua::Table(items)
                    if items
                        .iter()
                        .all(|(k, v)| k.is_none() && matches!(v, Lua::Num(_))) =>
                {
                    let n: Vec<f64> = items
                        .iter()
                        .map(|(_, v)| if let Lua::Num(n) = v { *n } else { 0. })
                        .collect();
                    Raw::List(
                        n.chunks(2)
                            .filter(|c| c.len() == 2)
                            .map(|c| format!("{}, {}", c[0], c[1]))
                            .collect(),
                    )
                }
                Lua::Table(_) => Raw::Present,
            };
            out.insert(key.clone(), raw);
        }
    }
    Ok((name, out))
}

// ---------------------------------------------------------------------------------------
// Settings → Preset, with a readable account of what is applied and what is not.
fn build(name: String, file: &str, s: &HashMap<String, Raw>) -> Result<Preset, String> {
    let mut p = Preset {
        name: name.chars().take(200).collect(),
        file: file.chars().take(260).collect(),
        ..Preset::default()
    };
    let num = |key: &str| match s.get(key) {
        Some(Raw::Text(t)) => t.trim().parse::<f64>().ok().filter(|v| v.is_finite()),
        _ => None,
    };
    let flag =
        |key: &str| matches!(s.get(key), Some(Raw::Text(t)) if t.eq_ignore_ascii_case("true"));
    let signed = |v: f64, digits: usize| format!("{v:+.digits$}");
    let mut clamped = false;
    let mut slider = |key: &str, label: &str, lo: f64, hi: f64, applied: &mut Vec<String>| -> f64 {
        let Some(v) = num(key) else { return 0. };
        let c = v.clamp(lo, hi);
        clamped |= c != v;
        if c != 0. {
            applied.push(format!(
                "{label} {}",
                signed(c, if hi <= 5. { 2 } else { 0 })
            ));
        }
        c
    };
    let mut applied = vec![];
    p.exposure = slider("Exposure2012", "Exposure", -5., 5., &mut applied);
    p.contrast = slider("Contrast2012", "Contrast", -100., 100., &mut applied);
    p.highlights = slider("Highlights2012", "Highlights", -100., 100., &mut applied);
    p.shadows = slider("Shadows2012", "Shadows", -100., 100., &mut applied);
    p.whites = slider("Whites2012", "Whites", -100., 100., &mut applied);
    p.blacks = slider("Blacks2012", "Blacks", -100., 100., &mut applied);
    p.temperature = slider(
        "IncrementalTemperature",
        "Temperature",
        -100.,
        100.,
        &mut applied,
    );
    p.tint = slider("IncrementalTint", "Tint", -100., 100., &mut applied);
    p.vibrance = slider("Vibrance", "Vibrance", -100., 100., &mut applied);
    p.saturation = slider("Saturation", "Saturation", -100., 100., &mut applied);
    p.parametric = [
        slider(
            "ParametricShadows",
            "Curve shadows",
            -100.,
            100.,
            &mut applied,
        ),
        slider("ParametricDarks", "Curve darks", -100., 100., &mut applied),
        slider(
            "ParametricLights",
            "Curve lights",
            -100.,
            100.,
            &mut applied,
        ),
        slider(
            "ParametricHighlights",
            "Curve highlights",
            -100.,
            100.,
            &mut applied,
        ),
    ];
    let splits = [
        num("ParametricShadowSplit").unwrap_or(25.),
        num("ParametricMidtoneSplit").unwrap_or(50.),
        num("ParametricHighlightSplit").unwrap_or(75.),
    ];
    if splits.iter().all(|v| (0.0..=100.).contains(v))
        && splits[0] < splits[1]
        && splits[1] < splits[2]
    {
        p.splits = splits;
    }
    if let Some(v) = num("Sharpness").filter(|v| *v > 0.) {
        p.sharpness = v.min(150.);
        applied.push(format!("Sharpening {}", p.sharpness.round()));
    }
    if flag("ConvertToGrayscale") {
        p.grayscale = true;
        applied.push("Black & white".into());
    }
    for (i, key, label) in [
        (0, "ToneCurvePV2012", "Tone curve"),
        (1, "ToneCurvePV2012Red", "Red curve"),
        (2, "ToneCurvePV2012Green", "Green curve"),
        (3, "ToneCurvePV2012Blue", "Blue curve"),
    ] {
        if let Some(Raw::List(items)) = s.get(key) {
            let mut points: Vec<[f64; 2]> = items
                .iter()
                .filter_map(|t| {
                    let (x, y) = t.split_once(',')?;
                    Some([
                        x.trim().parse::<f64>().ok()?.clamp(0., 255.),
                        y.trim().parse::<f64>().ok()?.clamp(0., 255.),
                    ])
                })
                .take(64)
                .collect();
            points.sort_by(|a, b| a[0].total_cmp(&b[0]));
            points.dedup_by(|b, a| a[0] == b[0]);
            let identity = points.iter().all(|q| q[0] == q[1]);
            if points.len() >= 2 && !identity {
                applied.push(format!(
                    "{label} ({} points, from the preset's own data)",
                    points.len()
                ));
                p.curves[i] = points;
            }
        }
    }
    // Color mixer, color grading, calibration and effects.
    let bands = [
        "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
    ];
    let mut get = |key: &str, lo: f64, hi: f64, default: f64| -> f64 {
        num(key).map_or(default, |v| {
            let c = v.clamp(lo, hi);
            clamped |= c != v;
            c
        })
    };
    for (b, band) in bands.iter().enumerate() {
        p.hsl[b] = [
            "HueAdjustment",
            "SaturationAdjustment",
            "LuminanceAdjustment",
        ]
        .map(|k| get(&format!("{k}{band}"), -100., 100., 0.));
    }
    if p.hsl.iter().flatten().any(|v| *v != 0.) {
        applied.push("Color mixer (HSL)".into());
    }
    // Lightroom stores the shadow and highlight wheels in the older split-toning keys.
    p.grading = Grading {
        shadow_hue: get("SplitToningShadowHue", 0., 360., 0.),
        shadow_sat: get("SplitToningShadowSaturation", 0., 100., 0.),
        shadow_lum: get("ColorGradeShadowLum", -100., 100., 0.),
        midtone_hue: get("ColorGradeMidtoneHue", 0., 360., 0.),
        midtone_sat: get("ColorGradeMidtoneSat", 0., 100., 0.),
        midtone_lum: get("ColorGradeMidtoneLum", -100., 100., 0.),
        highlight_hue: get("SplitToningHighlightHue", 0., 360., 0.),
        highlight_sat: get("SplitToningHighlightSaturation", 0., 100., 0.),
        highlight_lum: get("ColorGradeHighlightLum", -100., 100., 0.),
        global_hue: get("ColorGradeGlobalHue", 0., 360., 0.),
        global_sat: get("ColorGradeGlobalSat", 0., 100., 0.),
        global_lum: get("ColorGradeGlobalLum", -100., 100., 0.),
        balance: get("SplitToningBalance", -100., 100., 0.),
        blending: get("ColorGradeBlending", 0., 100., 50.),
    };
    if !p.grading.is_neutral() {
        applied.push("Color grading / split toning".into());
    }
    p.calibration = [
        "ShadowTint",
        "RedHue",
        "RedSaturation",
        "GreenHue",
        "GreenSaturation",
        "BlueHue",
        "BlueSaturation",
    ]
    .map(|k| get(k, -100., 100., 0.));
    if p.calibration.iter().any(|v| *v != 0.) {
        applied.push("Camera calibration".into());
    }
    p.vignette = [
        get("PostCropVignetteAmount", -100., 100., 0.),
        get("PostCropVignetteMidpoint", 0., 100., 50.),
        get("PostCropVignetteFeather", 0., 100., 50.),
        get("PostCropVignetteRoundness", -100., 100., 0.),
    ];
    if p.vignette[0] != 0. {
        applied.push(format!("Vignette {:+.0}", p.vignette[0]));
    }
    p.grain = [
        get("GrainAmount", 0., 100., 0.),
        get("GrainSize", 0., 100., 25.),
        get("GrainFrequency", 0., 100., 50.),
    ];
    if p.grain[0] > 0. {
        applied.push(format!("Grain {:.0}", p.grain[0]));
    }
    // Negative Clarity softens local contrast; Framebatch renders it with its Glow effect.
    // Positive Clarity (added local contrast) is not reproduced.
    if let Some(c) = num("Clarity2012").filter(|v| *v < 0.) {
        let c = c.max(-100.);
        clamped |= c != num("Clarity2012").unwrap_or(c);
        p.glow = (-c * CLARITY_TO_GLOW).round();
        applied.push(format!("Clarity {c:+.0} (as Glow {})", p.glow));
    }
    p.applied = applied;
    if clamped {
        p.ignored
            .push("Values beyond Lightroom's range (limited)".into());
    }
    // Recognized but not reproduced. Only non-default values are listed.
    let mut ignored = |label: &str| p.ignored.push(label.into());
    if num("Clarity2012").is_some_and(|v| v > 0.) {
        ignored("Clarity");
    }
    for (key, label) in [
        ("Texture", "Texture"),
        ("Dehaze", "Dehaze"),
        ("LuminanceSmoothing", "Noise reduction"),
    ] {
        if num(key).is_some_and(|v| v != 0.) {
            ignored(label);
        }
    }
    // Point Color stores -1 for every unused slot.
    if let Some(Raw::List(items)) = s.get("PointColors")
        && items
            .iter()
            .flat_map(|i| i.split(','))
            .any(|v| v.trim().parse::<f64>().is_ok_and(|v| v != -1.))
    {
        ignored("Point color");
    }
    if p.grayscale
        && bands
            .iter()
            .any(|b| num(&format!("GrayMixer{b}")).is_some_and(|v| v != 0.))
    {
        ignored("Black & white mix");
    }
    if s.contains_key("Temperature") || s.contains_key("Tint") {
        p.ignored
            .push("Raw white balance (Kelvin), which only applies to raw files".into());
    }
    if s.contains_key("Look") || num("ProfileAmount").is_some() {
        p.ignored.push("Profile".into());
    }
    if [
        "MaskGroupBasedCorrections",
        "GradientBasedCorrections",
        "CircularGradientBasedCorrections",
        "PaintBasedCorrections",
    ]
    .iter()
    .any(|k| s.contains_key(*k))
    {
        p.ignored.push("Masks and local adjustments".into());
    }
    if flag("LensProfileEnable")
        || flag("AutoLateralCA")
        || num("PerspectiveUpright").is_some_and(|v| v != 0.)
    {
        p.ignored.push("Lens and perspective corrections".into());
    }
    if flag("HasCrop") {
        p.ignored
            .push("Crop (Framebatch uses its own image size)".into());
    }
    if [
        "Exposure",
        "Contrast",
        "Brightness",
        "FillLight",
        "HighlightRecovery",
        "Clarity",
    ]
    .iter()
    .any(|k| num(k).is_some_and(|v| v != 0.))
    {
        p.ignored
            .push("Settings from Lightroom versions before 2012".into());
    }
    p.ignored.dedup();
    if p.applied.is_empty() && p.ignored.is_empty() {
        return Err("This preset has no settings that change a photo.".into());
    }
    Ok(p)
}

// ---------------------------------------------------------------------------------------
// Rendering. Order: white balance, exposure, contrast, whites/blacks, parametric curve and
// point curves per channel (folded into the engine's lookup tables), then highlights/shadows,
// vibrance, saturation and black & white per pixel.
fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn to_srgb(v: f32) -> f32 {
    let v = v.max(0.);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }
}
/// Monotone cubic (Fritsch–Carlson) through the preset's points, flat outside them.
struct Spline {
    x: Vec<f32>,
    y: Vec<f32>,
    m: Vec<f32>,
}
impl Spline {
    fn new(points: &[[f64; 2]]) -> Option<Self> {
        if points.len() < 2 {
            return None;
        }
        let x: Vec<f32> = points.iter().map(|p| p[0] as f32).collect();
        let y: Vec<f32> = points.iter().map(|p| p[1] as f32).collect();
        let n = x.len();
        let d: Vec<f32> = (0..n - 1)
            .map(|i| (y[i + 1] - y[i]) / (x[i + 1] - x[i]))
            .collect();
        let mut m = vec![0.; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            m[i] = if d[i - 1] * d[i] <= 0. {
                0.
            } else {
                (d[i - 1] + d[i]) / 2.
            };
        }
        for i in 0..n - 1 {
            if d[i] == 0. {
                (m[i], m[i + 1]) = (0., 0.);
            } else {
                let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
                let h = a.hypot(b);
                if h > 3. {
                    (m[i], m[i + 1]) = (3. * a / h * d[i], 3. * b / h * d[i]);
                }
            }
        }
        Some(Self { x, y, m })
    }
    fn eval(&self, v: f32) -> f32 {
        let n = self.x.len();
        if v <= self.x[0] {
            return self.y[0];
        }
        if v >= self.x[n - 1] {
            return self.y[n - 1];
        }
        let i = self.x.partition_point(|x| *x <= v) - 1;
        let h = self.x[i + 1] - self.x[i];
        let t = (v - self.x[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2. * t3 - 3. * t2 + 1.) * self.y[i]
            + (t3 - 2. * t2 + t) * h * self.m[i]
            + (-2. * t3 + 3. * t2) * self.y[i + 1]
            + (t3 - t2) * h * self.m[i + 1]
    }
}
/// Where an image being processed sits in the exported photo: vignette and grain use
/// output-pixel positions so preview, the 100% detail crop and the export line up.
#[derive(Clone, Copy)]
pub struct Frame {
    pub x0: u32,
    pub y0: u32,
    /// Exported size in pixels.
    pub width: u32,
    pub height: u32,
    /// Exported pixels per pixel of the image being processed (above 1 for previews).
    pub scale: f32,
}
impl Frame {
    pub fn whole(width: u32, height: u32) -> Self {
        Self {
            x0: 0,
            y0: 0,
            width,
            height,
            scale: 1.,
        }
    }
}
fn luma(v: &[f32; 3]) -> f32 {
    0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]
}
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
/// HSV hue (degrees), saturation (0..1), value (0..1) of an RGB triple in 0..1.
fn to_hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let (hi, lo) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    let d = hi - lo;
    if d <= 1e-6 || hi <= 0. {
        return (0., 0., hi);
    }
    let h = if hi == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.)
    } else if hi == c[1] {
        (c[2] - c[0]) / d + 2.
    } else {
        (c[0] - c[1]) / d + 4.
    };
    (h * 60., d / hi, hi)
}
fn from_hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.) / 60.;
    let f = |n: f32| {
        let k = (n + h) % 6.;
        v - v * s * k.min(4. - k).clamp(0., 1.)
    };
    [f(5.), f(3.), f(1.)]
}
/// Zero-luminance color offset for a hue: adding it tints without changing brightness.
fn tint(hue: f32) -> [f32; 3] {
    let c = from_hsv(hue, 1., 1.);
    let y = luma(&c);
    c.map(|v| v - y)
}
/// Lightroom's eight color-mixer bands.
const BAND_HUES: [f32; 8] = [0., 30., 60., 120., 180., 240., 270., 300.];
fn band_weights(h: f32) -> [f32; 8] {
    let mut w = [0.; 8];
    for i in 0..8 {
        let (a, b) = (BAND_HUES[i], if i == 7 { 360. } else { BAND_HUES[i + 1] });
        if (a..b).contains(&h) {
            let k = 0.5 + 0.5 * (std::f32::consts::PI * (h - a) / (b - a)).cos();
            w[i] = k;
            w[(i + 1) % 8] = 1. - k;
        }
    }
    w
}
struct Grade {
    /// Shadow, midtone, highlight, global: tint offset (0..255 scale) and luminance shift.
    tints: [[f32; 3]; 4],
    lums: [f32; 4],
    pivot: f32,
    soft: f32,
    mid_power: f32,
}
struct Calibration {
    matrix: [[f32; 3]; 3],
    shadow_tint: f32,
    to_lin: Vec<f32>,
    to_srgb: Vec<f32>,
}
const TABLE: usize = 4096;
impl Calibration {
    fn new(c: &[f64; 7]) -> Option<Self> {
        if c.iter().all(|v| *v == 0.) {
            return None;
        }
        // Each primary's hue turns up to 30 degrees and its saturation changes up to 50%;
        // the matrix maps the original primaries to the moved ones in linear light.
        let mut cols = [[0f32; 3]; 3];
        for (i, col) in cols.iter_mut().enumerate() {
            let (hue, sat) = (c[1 + 2 * i] as f32 / 100., c[2 + 2 * i] as f32 / 100.);
            let p = from_hsv(i as f32 * 120. + hue * 30., 1., 1.).map(to_linear);
            let gray = luma(&p);
            *col = p.map(|v| gray + (v - gray) * (1. + 0.5 * sat));
        }
        let mut matrix = [[0f32; 3]; 3];
        for (r, row) in matrix.iter_mut().enumerate() {
            let sum: f32 = cols.iter().map(|col| col[r]).sum();
            for (k, cell) in row.iter_mut().enumerate() {
                // Normalized so white stays white.
                *cell = cols[k][r] / sum;
            }
        }
        Some(Self {
            matrix,
            shadow_tint: c[0] as f32 / 100.,
            to_lin: (0..=TABLE)
                .map(|i| to_linear(i as f32 / TABLE as f32))
                .collect(),
            // Indexed by the square root of linear light for precision in the shadows.
            to_srgb: (0..=TABLE)
                .map(|i| to_srgb((i as f32 / TABLE as f32).powi(2)))
                .collect(),
        })
    }
    fn lookup(table: &[f32], x: f32) -> f32 {
        let f = x.clamp(0., 1.) * TABLE as f32;
        let i = (f as usize).min(TABLE - 1);
        table[i] + (table[i + 1] - table[i]) * (f - i as f32)
    }
    fn apply(&self, rgb: &mut [f32; 3]) {
        let lin = rgb.map(|v| Self::lookup(&self.to_lin, v / 255.));
        for (r, out) in rgb.iter_mut().enumerate() {
            let l = (0..3).map(|k| self.matrix[r][k] * lin[k]).sum::<f32>();
            *out = Self::lookup(&self.to_srgb, l.max(0.).sqrt()) * 255.;
        }
        if self.shadow_tint != 0. {
            // Positive is magenta, negative green, mostly in the shadows.
            let y = (luma(rgb) / 255.).clamp(0., 1.);
            let k = self.shadow_tint * 0.06 * 255. * (1. - y) * (1. - y);
            rgb[0] += 0.5 * k;
            rgb[1] -= k;
            rgb[2] += 0.5 * k;
        }
    }
}
fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h as f32 / u32::MAX as f32 * 2. - 1.
}
/// Smooth value noise in -1..1 whose lattice repeats every `period` cells: a pure
/// function of position, so batches stay deterministic.
fn noise(x: f32, y: f32, period: i32) -> f32 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (sx, sy) = (fx * fx * (3. - 2. * fx), fy * fy * (3. - 2. * fy));
    let (ix, iy) = (ix as i32, iy as i32);
    let h = |x: i32, y: i32| hash(x.rem_euclid(period), y.rem_euclid(period));
    let top = h(ix, iy) + (h(ix + 1, iy) - h(ix, iy)) * sx;
    let bottom = h(ix, iy + 1) + (h(ix + 1, iy + 1) - h(ix, iy + 1)) * sx;
    top + (bottom - top) * sy
}
/// Color table size per axis, and the grain tile size in exported pixels.
const GRID: usize = 33;
const TILE: usize = 1024;
/// The grain pattern depends only on grain size and roughness; it is cached separately
/// because manual slider changes rebuild a look many times.
type GrainCache = Vec<((i32, u32), Arc<Vec<f32>>)>;
fn grain_tile(period: i32, rough: f32) -> Arc<Vec<f32>> {
    static CACHE: Mutex<GrainCache> = Mutex::new(Vec::new());
    let key = (period, rough.to_bits());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, tile)) = cache.iter().find(|(k, _)| *k == key) {
        return tile.clone();
    }
    let cell = TILE as f32 / period as f32;
    let tile: Arc<Vec<f32>> = Arc::new(
        (0..TILE * TILE)
            .map(|i| {
                let x = ((i % TILE) as f32 + 0.5) / cell;
                let y = ((i / TILE) as f32 + 0.5) / cell;
                noise(x, y, period) * (1. - 0.5 * rough)
                    + noise(2. * x + 17., 2. * y + 3., 2 * period) * 0.5 * rough
            })
            .collect(),
    );
    if cache.len() == 4 {
        cache.remove(0);
    }
    cache.push((key, tile.clone()));
    tile
}
/// Looks are built once per preset (their tables take milliseconds) and shared by every
/// photo and preview that uses it.
pub fn look_for(p: &Preset) -> Arc<Look> {
    static CACHE: Mutex<Vec<(String, Arc<Look>)>> = Mutex::new(Vec::new());
    let key = serde_json::to_string(p).unwrap_or_default();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, look)) = cache.iter().find(|(k, _)| *k == key) {
        return look.clone();
    }
    let look = Arc::new(Look::new(p));
    if cache.len() == 4 {
        cache.remove(0);
    }
    cache.push((key, look.clone()));
    look
}
pub struct Look {
    gains: [f32; 3],
    exposure: f32,
    white: f32,
    contrast: f32,
    whites: f32,
    blacks: f32,
    parametric: [f32; 4],
    bounds: [f32; 5],
    curves: [Option<Spline>; 4],
    highlights: f32,
    shadows: f32,
    vibrance: f32,
    saturation: f32,
    grayscale: bool,
    hsl: Option<[[f32; 3]; 8]>,
    grade: Option<Grade>,
    calibration: Option<Calibration>,
    vignette: Option<[f32; 4]>,
    grain: Option<[f32; 3]>,
    /// Per-pixel color steps baked into a GRID x GRID x GRID table.
    lut: Option<Vec<f32>>,
    /// Vignette strength by (|x|^p + |y|^p) / 2 over the frame, and the exponent p.
    vignette_table: Vec<f32>,
    vignette_p: f32,
    /// Seamless TILE x TILE grain pattern and its grain cell size in exported pixels.
    grain_tile: Arc<Vec<f32>>,
    grain_cell: f32,
    pub sharpen: f32,
    /// Glow amount, 0..1.
    pub glow: f32,
}
const SHOULDER: f32 = 0.5;
impl Look {
    pub fn new(p: &Preset) -> Self {
        let (t, u) = (p.temperature as f32 / 100., p.tint as f32 / 100.);
        let mut gains = [
            2f32.powf(0.3 * t),
            2f32.powf(-0.25 * u),
            2f32.powf(-0.3 * t),
        ];
        let k = 0.2126 * gains[0] + 0.7152 * gains[1] + 0.0722 * gains[2];
        gains = gains.map(|g| g / k);
        let exposure = 2f32.powf(p.exposure as f32);
        let s = p.splits.map(|v| v as f32 / 100.);
        let g = &p.grading;
        let grade = (!g.is_neutral()).then(|| {
            let wheel =
                |hue: f64, sat: f64| tint(hue as f32).map(|v| v * sat as f32 / 100. * 0.3 * 255.);
            Grade {
                tints: [
                    wheel(g.shadow_hue, g.shadow_sat),
                    wheel(g.midtone_hue, g.midtone_sat),
                    wheel(g.highlight_hue, g.highlight_sat),
                    wheel(g.global_hue, g.global_sat),
                ],
                lums: [g.shadow_lum, g.midtone_lum, g.highlight_lum, g.global_lum]
                    .map(|v| v as f32 / 100. * 0.15 * 255.),
                // Positive balance favors the highlight wheel.
                pivot: (0.5 - g.balance as f32 / 200.).clamp(0.1, 0.9),
                soft: 0.15 + 0.5 * g.blending as f32 / 100.,
                mid_power: 2. - g.blending as f32 / 100.,
            }
        });
        let any_hsl = p.hsl.iter().flatten().any(|v| *v != 0.);
        let mut look = Self {
            white: exposure * gains.iter().copied().fold(0., f32::max),
            gains,
            exposure,
            contrast: p.contrast as f32 / 100.,
            whites: p.whites as f32 / 100.,
            blacks: p.blacks as f32 / 100.,
            parametric: p.parametric.map(|v| v as f32 / 100.),
            bounds: [0., s[0], s[1], s[2], 1.],
            curves: std::array::from_fn(|i| Spline::new(&p.curves[i])),
            highlights: p.highlights as f32 / 100.,
            shadows: p.shadows as f32 / 100.,
            vibrance: p.vibrance as f32 / 100.,
            saturation: p.saturation as f32 / 100.,
            grayscale: p.grayscale,
            hsl: any_hsl.then(|| p.hsl.map(|b| b.map(|v| v as f32 / 100.))),
            grade,
            calibration: Calibration::new(&p.calibration),
            vignette: (p.vignette[0] != 0.).then(|| p.vignette.map(|v| v as f32 / 100.)),
            grain: (p.grain[0] > 0.).then(|| p.grain.map(|v| v as f32 / 100.)),
            lut: None,
            vignette_table: vec![],
            vignette_p: 2.,
            grain_tile: Arc::default(),
            grain_cell: 1.,
            sharpen: p.sharpness as f32 / 150. * 0.5,
            glow: p.glow as f32 / 100.,
        };
        look.build_tables();
        look
    }
    fn build_tables(&mut self) {
        if self.has_pixel_steps() {
            let step = 255. / (GRID - 1) as f32;
            let mut lut = Vec::with_capacity(GRID * GRID * GRID * 3);
            for r in 0..GRID {
                for g in 0..GRID {
                    for b in 0..GRID {
                        let mut c = [r as f32 * step, g as f32 * step, b as f32 * step];
                        self.pixel(&mut c);
                        lut.extend(c);
                    }
                }
            }
            self.lut = Some(lut);
        }
        if let Some([amount, mid, feather, round]) = self.vignette {
            // Negative roundness squares the shape off; corners reach 1.
            self.vignette_p = 2. + 4. * (-round).max(0.);
            let (r0, width) = (0.25 + 0.6 * mid, 0.1 + 0.8 * feather);
            self.vignette_table = (0..=TABLE)
                .map(|i| {
                    let d = (i as f32 / TABLE as f32).powf(1. / self.vignette_p);
                    smoothstep(r0 - width / 2., r0 + width / 2., d) * 0.85 * amount
                })
                .collect();
        }
        if let Some([_, size, rough]) = self.grain {
            // Lattice periods divide the tile exactly, so the tile repeats seamlessly.
            let period = ((TILE as f32 / (1.5 + 4.5 * size)).round() as i32).max(1);
            self.grain_cell = TILE as f32 / period as f32;
            self.grain_tile = grain_tile(period, rough);
        }
    }
    /// The per-pixel steps through the precomputed color table. Tetrahedral interpolation:
    /// four table entries per pixel, and grays interpolate along the neutral axis.
    pub fn pixel_fast(&self, rgb: &mut [f32; 3]) {
        let Some(lut) = &self.lut else { return };
        const DB: usize = 3;
        const DG: usize = GRID * 3;
        const DR: usize = GRID * GRID * 3;
        let top = (GRID - 1) as f32;
        let f = rgb.map(|v| (v * (top / 255.)).clamp(0., top));
        let i = f.map(|v| (v as usize).min(GRID - 2));
        let (r, g, b) = (f[0] - i[0] as f32, f[1] - i[1] as f32, f[2] - i[2] as f32);
        let base = i[0] * DR + i[1] * DG + i[2] * DB;
        // The cube corner path that bounds this point, with its three weights.
        let (a, c, wa, wc, wd, w0) = if r >= g {
            if g >= b {
                (DR, DR + DG, r - g, g - b, b, 1. - r)
            } else if r >= b {
                (DR, DR + DB, r - b, b - g, g, 1. - r)
            } else {
                (DB, DR + DB, b - r, r - g, g, 1. - b)
            }
        } else if b >= g {
            (DB, DG + DB, b - g, g - r, r, 1. - b)
        } else if b >= r {
            (DG, DG + DB, g - b, b - r, r, 1. - g)
        } else {
            (DG, DR + DG, g - r, r - b, b, 1. - g)
        };
        let end = DR + DG + DB;
        for (k, out) in rgb.iter_mut().enumerate() {
            let at = |o: usize| lut[base + o + k];
            *out = w0 * at(0) + wa * at(a) + wc * at(c) + wd * at(end);
        }
    }
    /// Per-channel part: 0..255 in, 0..255 out.
    pub fn channel(&self, c: usize, x: f32) -> f32 {
        let mut lin = to_linear((x / 255.).clamp(0., 1.)) * self.gains[c] * self.exposure;
        // Highlight roll-off: when exposure/white balance would push white past 1, compress
        // [SHOULDER, white] into [SHOULDER, 1] with slope 1 at the shoulder (identity at 0 EV).
        if self.white > 1. && lin > SHOULDER {
            let n = (self.white - SHOULDER) / (1. - SHOULDER);
            let t = ((lin - SHOULDER) / (self.white - SHOULDER)).clamp(0., 1.);
            lin = SHOULDER + (1. - SHOULDER) * (1. - (1. - t).powf(n));
        }
        let mut v = to_srgb(lin).clamp(0., 1.);
        let s = v * v * (3. - 2. * v);
        v += self.contrast * if self.contrast > 0. { 1.5 } else { 0.75 } * (s - v);
        v = v.clamp(0., 1.);
        v += self.whites * 0.15 * v.powi(4) + self.blacks * 0.15 * (1. - v).powi(4);
        v = v.clamp(0., 1.);
        if self.parametric.iter().any(|a| *a != 0.) {
            let taper = 1. - (2. * v - 1.).abs().powi(8);
            let mut delta = 0.;
            for r in 0..4 {
                let (lo, hi) = (self.bounds[r], self.bounds[r + 1]);
                let (center, half) = ((lo + hi) / 2., hi - lo);
                let d = (v - center) / half;
                if d.abs() < 1. {
                    delta +=
                        self.parametric[r] * 0.15 * (d * std::f32::consts::FRAC_PI_2).cos().powi(2);
                }
            }
            v = (v + delta * taper).clamp(0., 1.);
        }
        let mut x = v * 255.;
        if let Some(master) = &self.curves[0] {
            x = master.eval(x);
        }
        if let Some(curve) = &self.curves[c + 1] {
            x = curve.eval(x);
        }
        x.clamp(0., 255.)
    }
    pub fn has_pixel_steps(&self) -> bool {
        self.highlights != 0.
            || self.shadows != 0.
            || self.vibrance != 0.
            || self.saturation != 0.
            || self.grayscale
            || self.hsl.is_some()
            || self.grade.is_some()
            || self.calibration.is_some()
    }
    /// Per-pixel part on 0..255 values (results may exceed the range; the caller clamps).
    /// Order: calibration, highlights/shadows, color mixer, vibrance, saturation, color
    /// grading, black & white.
    pub fn pixel(&self, rgb: &mut [f32; 3]) {
        if let Some(c) = &self.calibration {
            c.apply(rgb);
        }
        if self.highlights != 0. || self.shadows != 0. {
            let y = (luma(rgb) / 255.).clamp(0., 1.);
            let ws = 6.75 * y * (1. - y) * (1. - y);
            let wh = 6.75 * (1. - y) * y * y;
            let target = (y + 0.2 * (self.shadows * ws + self.highlights * wh)).clamp(0., 1.);
            if y > 1e-4 {
                let k = target / y;
                rgb.iter_mut().for_each(|v| *v *= k);
            } else {
                rgb.iter_mut().for_each(|v| *v += target * 255.);
            }
        }
        if let Some(hsl) = &self.hsl {
            let (h, s, v) = to_hsv(rgb.map(|c| (c / 255.).clamp(0., 1.)));
            if s > 1e-3 {
                let w = band_weights(h);
                let mix = |k: usize| (0..8).map(|b| w[b] * hsl[b][k]).sum::<f32>();
                let (dh, ds, dl) = (mix(0), mix(1), mix(2));
                // Grays have no hue; weak colors get a proportionally weak change.
                let strength = (s / 0.25).min(1.);
                *rgb = from_hsv(h + dh * 30. * strength, s, v).map(|c| c * 255.);
                let y = luma(rgb);
                let k = (1. + ds * strength).max(0.);
                rgb.iter_mut().for_each(|c| *c = y + (*c - y) * k);
                let l = 1. + 0.5 * dl * strength;
                rgb.iter_mut().for_each(|c| *c *= l);
            }
        }
        if self.vibrance != 0. {
            let y = luma(rgb);
            let (hi, lo) = (
                rgb[0].max(rgb[1]).max(rgb[2]),
                rgb[0].min(rgb[1]).min(rgb[2]),
            );
            let sat = if hi > 1. {
                ((hi - lo) / hi).clamp(0., 1.)
            } else {
                0.
            };
            let k = 1. + self.vibrance * (1. - sat);
            rgb.iter_mut().for_each(|v| *v = y + (*v - y) * k);
        }
        if self.saturation != 0. {
            let y = luma(rgb);
            rgb.iter_mut()
                .for_each(|v| *v = y + (*v - y) * (1. + self.saturation));
        }
        if let Some(g) = &self.grade {
            let y = (luma(rgb) / 255.).clamp(0., 1.);
            let wh = smoothstep(g.pivot - g.soft, g.pivot + g.soft, y);
            let wm = (4. * y * (1. - y)).powf(g.mid_power);
            // Keeps pure black and white neutral.
            let ends = 1. - (2. * y - 1.).abs().powi(8);
            for (r, w) in [(0, 1. - wh), (1, wm), (2, wh), (3, 1.)] {
                for (c, value) in rgb.iter_mut().enumerate() {
                    *value += w * ends * g.tints[r][c] + w * g.lums[r];
                }
            }
        }
        if self.grayscale {
            let y = luma(rgb);
            *rgb = [y; 3];
        }
    }
    pub fn has_effects(&self) -> bool {
        self.vignette.is_some() || self.grain.is_some()
    }
    /// Vignette term and grain tile coordinate along one axis, at exported position `o`.
    fn axis(&self, o: f32, size: u32) -> (f32, usize) {
        let n = (o / size as f32 * 2. - 1.).abs();
        (
            n.powf(self.vignette_p),
            (o.floor() as i64).rem_euclid(TILE as i64) as usize,
        )
    }
    fn effect_at(&self, rgb: &mut [f32; 3], vignette: f32, tile: usize, visible: f32) {
        if self.vignette.is_some() {
            let f = vignette.clamp(0., 1.) * TABLE as f32;
            let i = (f as usize).min(TABLE - 1);
            let t = &self.vignette_table;
            let w = t[i] + (t[i + 1] - t[i]) * (f - i as f32);
            for c in rgb.iter_mut() {
                *c = if w < 0. {
                    *c * (1. + w)
                } else {
                    *c + (255. - *c) * w
                };
            }
        }
        if let Some([amount, ..]) = self.grain {
            let l = (luma(rgb) / 255.).clamp(0., 1.);
            let k =
                self.grain_tile[tile] * amount * 0.1 * 255. * (0.25 + 3. * l * (1. - l)) * visible;
            rgb.iter_mut().for_each(|c| *c += k);
        }
    }
    /// Vignette and grain at pixel (x, y) of an image placed in the exported photo as
    /// `frame` describes.
    pub fn effects(&self, rgb: &mut [f32; 3], x: u32, y: u32, frame: &Frame) {
        let (vx, tx) = self.axis(
            frame.x0 as f32 + (x as f32 + 0.5) * frame.scale,
            frame.width,
        );
        let (vy, ty) = self.axis(
            frame.y0 as f32 + (y as f32 + 0.5) * frame.scale,
            frame.height,
        );
        // A preview pixel covering several grain cells averages them out.
        let visible = (self.grain_cell / frame.scale).min(1.);
        self.effect_at(rgb, (vx + vy) / 2., ty * TILE + tx, visible);
    }
    /// Vignette and grain over a whole image, applied last (after sharpening).
    pub fn effects_image(
        &self,
        img: &mut RgbaImage,
        frame: &Frame,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let width = img.width() as usize;
        let columns: Vec<(f32, usize)> = (0..img.width())
            .map(|x| {
                self.axis(
                    frame.x0 as f32 + (x as f32 + 0.5) * frame.scale,
                    frame.width,
                )
            })
            .collect();
        let visible = (self.grain_cell / frame.scale).min(1.);
        for (y, row) in img.as_mut().chunks_exact_mut(width * 4).enumerate() {
            if y % 64 == 0 {
                crate::engine::cancelled(cancel)?;
            }
            let (vy, ty) = self.axis(
                frame.y0 as f32 + (y as f32 + 0.5) * frame.scale,
                frame.height,
            );
            for (px, (vx, tx)) in row.as_chunks_mut::<4>().0.iter_mut().zip(&columns) {
                let mut rgb = [px[0], px[1], px[2]].map(f32::from);
                self.effect_at(&mut rgb, (vx + vy) / 2., ty * TILE + tx, visible);
                for c in 0..3 {
                    px[c] = crate::engine::to_u8(rgb[c]);
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Glow: a mist-filter style effect. Each pixel is pulled toward a blurred copy of the photo
// (most in the midtones, like negative Clarity), and bright parts of the blurred copy are
// screened back on so light areas bloom softly into their surroundings.
/// Blur radius (Gaussian sigma) as a fraction of the exported photo's long edge, so the
/// preview, the 100% detail view and every export size get the same look.
const GLOW_RADIUS: f32 = 0.012;
const GLOW_SOFTEN: f32 = 0.6;
const GLOW_BLOOM: f32 = 0.45;
/// Glow blur radius in pixels of the image being processed.
pub fn glow_sigma(frame: &Frame) -> f32 {
    GLOW_RADIUS * frame.width.max(frame.height) as f32 / frame.scale
}
/// Applies `amount` (0..1) of glow to an image placed in the export as `frame` describes.
pub fn glow_image(
    img: &mut RgbaImage,
    amount: f32,
    frame: &Frame,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let amount = amount.clamp(0., 1.);
    if amount <= 0. {
        return Ok(());
    }
    let (w, h) = img.dimensions();
    let sigma = glow_sigma(frame).max(0.5);
    // The blur runs on a reduced copy (about 2.5 of its pixels of blur) and is sampled back
    // with bilinear interpolation: a wide, smooth blur for a small fraction of the time and
    // memory of a full-size one.
    let f = (sigma / 2.5).floor().max(1.);
    let sw = ((w as f32 / f).round() as u32).max(1);
    let sh = ((h as f32 / f).round() as u32).max(1);
    let small = if (sw, sh) == (w, h) {
        img.clone()
    } else {
        image::imageops::thumbnail(img, sw, sh)
    };
    crate::engine::cancelled(cancel)?;
    let s = sigma * sw as f32 / w as f32;
    // Bloom spreads only the bright parts, twice as wide as the softening blur.
    let mut bright = small.clone();
    for p in bright.pixels_mut() {
        let rgb = [p[0], p[1], p[2]].map(f32::from);
        let k = smoothstep(0.55, 1., luma(&rgb) / 255.);
        for c in 0..3 {
            p[c] = crate::engine::to_u8(rgb[c] * k);
        }
    }
    let blurred = image::imageops::blur(&small, s);
    drop(small);
    let glowing = image::imageops::blur(&bright, 2. * s);
    drop(bright);
    crate::engine::cancelled(cancel)?;
    // Bilinear sample positions per column and row.
    let taps = |n: u32, small: u32| -> Vec<(usize, usize, f32)> {
        (0..n)
            .map(|i| {
                let u = ((i as f32 + 0.5) * small as f32 / n as f32 - 0.5)
                    .clamp(0., (small - 1) as f32);
                let a = u.floor() as usize;
                (a, (a + 1).min(small as usize - 1), u - a as f32)
            })
            .collect()
    };
    let (columns, rows) = (taps(w, sw), taps(h, sh));
    let stride = sw as usize * 4;
    let sample =
        |buf: &[u8], (r0, r1, ty): (usize, usize, f32), (c0, c1, tx): (usize, usize, f32)| {
            let (top, bottom) = (&buf[r0 * stride..], &buf[r1 * stride..]);
            std::array::from_fn::<f32, 3, _>(|c| {
                let at = |line: &[u8], col: usize| f32::from(line[col * 4 + c]);
                let upper = at(top, c0) + (at(top, c1) - at(top, c0)) * tx;
                let lower = at(bottom, c0) + (at(bottom, c1) - at(bottom, c0)) * tx;
                upper + (lower - upper) * ty
            })
        };
    let (soft, glow) = (blurred.as_raw(), glowing.as_raw());
    let (soften, bloom) = (GLOW_SOFTEN * amount, GLOW_BLOOM * amount / 255.);
    for (y, row) in img.as_mut().chunks_exact_mut(w as usize * 4).enumerate() {
        if y % 64 == 0 {
            crate::engine::cancelled(cancel)?;
        }
        for (px, &column) in row.as_chunks_mut::<4>().0.iter_mut().zip(&columns) {
            let b = sample(soft, rows[y], column);
            let g = sample(glow, rows[y], column);
            let rgb = [px[0], px[1], px[2]].map(f32::from);
            let l = (luma(&rgb) / 255.).clamp(0., 1.);
            // Strongest in the midtones and fading out at white, so highlights never dim.
            let k = soften * (0.3 + 2.8 * l * (1. - l)) * (1. - l.powi(4));
            for c in 0..3 {
                let v = rgb[c] + (b[c] - rgb[c]) * k;
                // Screen blend of the bloom: lightens without clipping.
                px[c] = crate::engine::to_u8(255. - (255. - v) * (1. - bloom * g[c]));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 5.6-c140">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:Version="15.0"
   crs:ProcessVersion="11.0"
   crs:Exposure2012="+0.35"
   crs:Contrast2012="-12"
   crs:Highlights2012="-30"
   crs:IncrementalTemperature="+15"
   crs:Vibrance="+20"
   crs:Clarity2012="+10"
   crs:SaturationAdjustmentBlue="-25"
   crs:Sharpness="40"
   crs:HasSettings="True">
   <crs:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Warm &amp; Faded</rdf:li>
    </rdf:Alt>
   </crs:Name>
   <crs:Shadows2012>+25</crs:Shadows2012>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 24</rdf:li>
     <rdf:li>128, 136</rdf:li>
     <rdf:li>255, 250</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
   <crs:ToneCurvePV2012Red>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012Red>
   <crs:Look>
    <rdf:Description crs:Name="Adobe Color" crs:Amount="1"/>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
    const LRTEMPLATE: &str = r#"s = {
	id = "6A9C4C5A-0000",
	internalName = "Moody BW",
	title = "Moody BW",
	type = "Develop",
	value = {
		settings = {
			ConvertToGrayscale = true,
			Contrast2012 = 35,
			Blacks2012 = -20,
			GrayMixerRed = -10,
			ToneCurvePV2012 = { 0, 0, 64, 52, 192, 205, 255, 255, },
			ToneCurveName2012 = "Custom",
		},
		uuid = "6A9C4C5A-0000",
	},
	version = 0,
}
"#;
    #[test]
    fn reads_xmp_attributes_elements_lists_and_reports() {
        let p = parse(XMP, r"C:\presets\warm.xmp").unwrap();
        assert_eq!(p.name, "Warm & Faded");
        assert_eq!(p.file, "warm.xmp");
        assert_eq!(
            (p.exposure, p.contrast, p.highlights, p.shadows),
            (0.35, -12., -30., 25.)
        );
        assert_eq!((p.temperature, p.vibrance, p.sharpness), (15., 20., 40.));
        assert_eq!(p.curves[0], vec![[0., 24.], [128., 136.], [255., 250.]]);
        assert!(p.curves[1].is_empty(), "identity channel curve is dropped");
        assert!(p.applied.iter().any(|a| a == "Color mixer (HSL)"));
        for expected in ["Clarity", "Profile"] {
            assert!(
                p.ignored.iter().any(|i| i == expected),
                "{expected} in {:?}",
                p.ignored
            );
        }
        assert!(
            p.applied
                .iter()
                .any(|a| a.starts_with("Tone curve (3 points"))
        );
    }
    #[test]
    fn reads_lrtemplate_as_data() {
        let p = parse(LRTEMPLATE, "moody.lrtemplate").unwrap();
        assert_eq!(p.name, "Moody BW");
        assert!(p.grayscale);
        assert_eq!((p.contrast, p.blacks), (35., -20.));
        assert_eq!(p.curves[0].len(), 4);
        assert!(p.ignored.iter().any(|i| i == "Black & white mix"));
    }
    #[test]
    fn reports_calibration_point_color_and_only_real_color_grading() {
        let unused = "-1.000000, -1.000000, -1.000000";
        let film = |extra: &str, point: &str| {
            XMP.replace(
                "crs:HasSettings=\"True\">",
                &format!("crs:ColorGradeBlending=\"100\" crs:ColorGradeGlobalHue=\"40\" {extra} crs:HasSettings=\"True\"><crs:PointColors><rdf:Seq><rdf:li>{point}</rdf:li></rdf:Seq></crs:PointColors>"),
            )
        };
        let p = parse(
            &film("crs:RedHue=\"+25\" crs:GreenSaturation=\"-30\"", unused),
            "film.xmp",
        )
        .unwrap();
        assert!(
            p.applied.iter().any(|a| a == "Camera calibration"),
            "{:?}",
            p.ignored
        );
        assert!(
            !p.applied.iter().any(|a| a.starts_with("Color grading")),
            "blending/hue alone grade nothing"
        );
        assert!(
            !p.ignored.iter().any(|i| i == "Point color"),
            "all slots unused"
        );
        let p = parse(
            &film("crs:ColorGradeMidtoneSat=\"30\"", "0.5, -1.000000"),
            "film.xmp",
        )
        .unwrap();
        assert!(p.applied.iter().any(|a| a.starts_with("Color grading")));
        assert!(p.ignored.iter().any(|i| i == "Point color"));
        assert!(!p.applied.iter().any(|a| a == "Camera calibration"));
    }
    #[test]
    fn builtin_presets_all_load() {
        let all = builtins();
        assert_eq!(all.len(), BUILTIN.len());
        assert!(all.iter().any(|p| p.name == "Golden Glow"));
        for p in &all {
            p.validate().unwrap();
            assert!(!p.is_neutral());
            let unsupported = ["Clarity", "Texture", "Dehaze"];
            assert!(
                p.ignored.iter().all(|i| unsupported.contains(&i.as_str())),
                "{}: {:?}",
                p.name,
                p.ignored
            );
        }
        assert!(Preset::default().is_neutral());
        assert!(
            Preset {
                name: "x".into(),
                ..Preset::default()
            }
            .is_neutral()
        );
    }
    #[test]
    fn rejects_non_presets_and_code() {
        assert!(parse("<x><y/></x>", "a.xmp").is_err());
        assert!(
            parse(
                &XMP.replace("PresetType=\"Normal\"", "PresetType=\"Look\""),
                "a.xmp"
            )
            .is_err()
        );
        assert!(parse("os.execute('calc')", "a.lrtemplate").is_err());
        assert!(
            parse(
                "s = { value = { settings = { Exposure2012 = os.time() } } }",
                "a.lrtemplate"
            )
            .is_err()
        );
        let empty = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Version="15.0"/></rdf:RDF></x:xmpmeta>"#;
        assert!(parse(empty, "empty.xmp").is_err());
    }
    #[test]
    fn neutral_preset_is_identity_and_curves_hit_their_points() {
        let look = Look::new(&Preset::default());
        for c in 0..3 {
            for x in [0., 1., 64., 128., 200., 255.] {
                assert!(
                    (look.channel(c, x) - x).abs() < 0.05,
                    "{c} {x} -> {}",
                    look.channel(c, x)
                );
            }
        }
        assert!(!look.has_pixel_steps());
        let curve = Preset {
            curves: [
                vec![[0., 20.], [128., 160.], [255., 240.]],
                vec![],
                vec![],
                vec![],
            ],
            ..Preset::default()
        };
        let look = Look::new(&curve);
        assert!((look.channel(0, 128.) - 160.).abs() < 0.05);
        assert!(
            (look.channel(1, 0.) - 20.).abs() < 0.05 && (look.channel(2, 255.) - 240.).abs() < 0.05
        );
        let mut last = -1.;
        for x in 0..=255 {
            let y = look.channel(0, x as f32);
            assert!(y >= last - 1e-3, "curve not monotone at {x}");
            last = y;
        }
    }
    fn px(p: &Preset, c: [f32; 3]) -> [f32; 3] {
        let mut v = c;
        Look::new(p).pixel(&mut v);
        v
    }
    fn spread(v: [f32; 3]) -> f32 {
        v[0].max(v[1]).max(v[2]) - v[0].min(v[1]).min(v[2])
    }
    #[test]
    fn color_mixer_moves_only_its_band_and_leaves_grays() {
        let mut hsl = [[0.; 3]; 8];
        hsl[3] = [0., -100., 0.];
        let p = Preset {
            hsl,
            ..Preset::default()
        };
        assert!(
            spread(px(&p, [40., 200., 40.])) < 3.,
            "green desaturated to gray"
        );
        let red = px(&p, [200., 40., 40.]);
        assert!(
            (red[0] - 200.).abs() < 0.5 && (red[1] - 40.).abs() < 0.5,
            "red untouched"
        );
        assert_eq!(px(&p, [120.; 3]), [120.; 3], "grays untouched");
        let mut hsl = [[0.; 3]; 8];
        hsl[0] = [100., 0., 0.];
        assert!(
            px(
                &Preset {
                    hsl,
                    ..Preset::default()
                },
                [200., 40., 40.]
            )[1] > 70.,
            "red toward orange"
        );
        let mut hsl = [[0.; 3]; 8];
        hsl[5] = [0., 0., -50.];
        let blue = [40., 60., 200.];
        assert!(
            luma(&px(
                &Preset {
                    hsl,
                    ..Preset::default()
                },
                blue
            )) < luma(&blue) * 0.85,
            "blue darker"
        );
    }
    #[test]
    fn grading_tints_by_brightness_and_calibration_keeps_white() {
        let grading = Grading {
            shadow_hue: 220.,
            shadow_sat: 60.,
            highlight_hue: 50.,
            highlight_sat: 60.,
            ..Grading::default()
        };
        let p = Preset {
            grading,
            ..Preset::default()
        };
        let dark = px(&p, [50.; 3]);
        let light = px(&p, [205.; 3]);
        assert!(dark[2] > dark[0] + 5., "shadows cool: {dark:?}");
        assert!(light[0] > light[2] + 5., "highlights warm: {light:?}");
        assert!(
            spread(px(&p, [255.; 3])) < 1. && spread(px(&p, [0.; 3])) < 1.,
            "ends stay neutral"
        );
        let global = Preset {
            grading: Grading {
                global_hue: 30.,
                global_sat: 40.,
                ..Grading::default()
            },
            ..Preset::default()
        };
        let mid = px(&global, [128.; 3]);
        assert!(mid[0] > mid[2] + 5.);
        let cal = Preset {
            calibration: [0., 50., 30., 0., 0., 0., 0.],
            ..Preset::default()
        };
        assert!(
            px(&cal, [220., 30., 30.])[1] > 40.,
            "red primary toward orange"
        );
        let white = px(&cal, [255.; 3]);
        assert!(
            white.iter().all(|v| (v - 255.).abs() < 1.),
            "white stays white: {white:?}"
        );
        assert!(spread(px(&cal, [128.; 3])) < 1., "gray stays gray");
    }
    #[test]
    fn vignette_and_grain_are_positional_and_deterministic() {
        let look = Look::new(&Preset {
            vignette: [-50., 50., 50., 0.],
            grain: [40., 25., 50.],
            ..Preset::default()
        });
        let frame = Frame::whole(1000, 600);
        let at = |f: &Frame, x: u32, y: u32| {
            let mut v = [128.; 3];
            look.effects(&mut v, x, y, f);
            v
        };
        assert!(
            at(&frame, 0, 0)[0] < at(&frame, 500, 300)[0] - 30.,
            "corners darker"
        );
        assert_eq!(at(&frame, 123, 45), at(&frame, 123, 45));
        let crop = Frame {
            x0: 100,
            y0: 50,
            ..frame
        };
        assert_eq!(at(&crop, 23, 17), at(&frame, 123, 67), "crops line up");
        let center: Vec<f32> = (0..400)
            .map(|i| at(&frame, 400 + i % 20, 250 + i / 20)[0])
            .collect();
        let mean = center.iter().sum::<f32>() / 400.;
        let var = center.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / 400.;
        assert!(
            (mean - 128.).abs() < 2. && var > 4.,
            "grain varies around the original"
        );
        let small = Frame {
            scale: 8.,
            ..Frame::whole(8000, 4800)
        };
        let preview: Vec<f32> = (0..400)
            .map(|i| at(&small, 50 + i % 20, 30 + i / 20)[0])
            .collect();
        let pm = preview.iter().sum::<f32>() / 400.;
        let pvar = preview.iter().map(|v| (v - pm).powi(2)).sum::<f32>() / 400.;
        assert!(
            pvar < var / 4.,
            "grain finer than a preview pixel is attenuated"
        );
    }
    #[test]
    fn color_table_matches_exact_math() {
        let mut hsl = [[0.; 3]; 8];
        hsl[3] = [15., -35., -15.];
        hsl[5] = [5., -20., -20.];
        let p = Preset {
            hsl,
            grading: Grading {
                midtone_hue: 50.,
                midtone_sat: 30.,
                shadow_hue: 200.,
                shadow_sat: 20.,
                ..Grading::default()
            },
            calibration: [10., 25., 20., 45., -30., 5., -25.],
            saturation: -10.,
            highlights: -15.,
            ..Preset::default()
        };
        let look = Look::new(&p);
        let mut worst = 0f32;
        for i in 0..2000u32 {
            let c = [
                (i * 37 % 256) as f32,
                (i * 101 % 256) as f32,
                (i * 173 % 256) as f32,
            ];
            let (mut exact, mut fast) = (c, c);
            look.pixel(&mut exact);
            look.pixel_fast(&mut fast);
            for k in 0..3 {
                worst = worst.max((exact[k].clamp(0., 255.) - fast[k].clamp(0., 255.)).abs());
            }
        }
        assert!(worst < 3., "table error {worst}");
    }
    #[test]
    fn sliders_move_the_expected_way() {
        let look = |p: Preset| Look::new(&p);
        let plus = look(Preset {
            exposure: 1.,
            ..Preset::default()
        });
        assert!(
            plus.channel(0, 118.) > 150. && plus.channel(0, 255.) > 254.5,
            "+1 EV brightens, keeps white"
        );
        assert!(
            plus.channel(0, 250.) < 255.,
            "highlights roll off instead of clipping early"
        );
        let minus = look(Preset {
            exposure: -1.,
            ..Preset::default()
        });
        assert!(minus.channel(0, 118.) < 90.);
        let contrast = look(Preset {
            contrast: 50.,
            ..Preset::default()
        });
        assert!(contrast.channel(0, 64.) < 64. && contrast.channel(0, 192.) > 192.);
        let warm = look(Preset {
            temperature: 50.,
            ..Preset::default()
        });
        assert!(
            warm.channel(0, 128.) > warm.channel(1, 128.)
                && warm.channel(2, 128.) < warm.channel(1, 128.)
        );
        let fade = look(Preset {
            blacks: 100.,
            ..Preset::default()
        });
        assert!(fade.channel(0, 0.) > 30.);
        let mut px = [200., 60., 60.];
        look(Preset {
            saturation: -100.,
            ..Preset::default()
        })
        .pixel(&mut px);
        assert!((px[0] - px[1]).abs() < 0.01 && (px[1] - px[2]).abs() < 0.01);
        let vib = look(Preset {
            vibrance: 50.,
            ..Preset::default()
        });
        let (mut dull, mut vivid) = ([130., 120., 110.], [240., 40., 30.]);
        let (d0, v0) = (dull[0] - dull[2], vivid[0] - vivid[2]);
        vib.pixel(&mut dull);
        vib.pixel(&mut vivid);
        assert!(
            (dull[0] - dull[2]) / d0 > (vivid[0] - vivid[2]) / v0,
            "vibrance favors muted colors"
        );
        let mut gray = [90., 140., 200.];
        look(Preset {
            grayscale: true,
            ..Preset::default()
        })
        .pixel(&mut gray);
        assert!(gray[0] == gray[1] && gray[1] == gray[2]);
    }
    #[test]
    fn negative_clarity_becomes_glow_and_dreamy_pastel_ships() {
        let soft = parse(
            &XMP.replace("Clarity2012=\"+10\"", "Clarity2012=\"-40\""),
            "soft.xmp",
        )
        .unwrap();
        assert_eq!(soft.glow, 24.);
        assert!(soft.applied.iter().any(|a| a == "Clarity -40 (as Glow 24)"));
        assert!(!soft.ignored.iter().any(|i| i == "Clarity"));
        let crisp = parse(XMP, "crisp.xmp").unwrap();
        assert_eq!(crisp.glow, 0.);
        let dreamy = builtins()
            .into_iter()
            .find(|p| p.name == "Dreamy Pastel")
            .unwrap();
        assert_eq!(dreamy.glow, 30.);
        assert!(dreamy.ignored.is_empty(), "{:?}", dreamy.ignored);
        assert!(Look::new(&dreamy).glow > 0.29);
        let golden = builtins()
            .into_iter()
            .find(|p| p.name == "Golden Glow")
            .unwrap();
        assert_eq!(golden.glow, 12.);
        assert!(golden.ignored.is_empty(), "{:?}", golden.ignored);
        // Warm, without clipping saturated reds or turning grays colorful at the ends.
        let mut red = [200., 40., 40.];
        let look = Look::new(&golden);
        for (c, v) in red.iter_mut().enumerate() {
            *v = look.channel(c, *v);
        }
        look.pixel(&mut red);
        assert!(red[0] < 250. && red[0] > red[2] + 100., "{red:?}");
        let mut gray = [118f32; 3];
        for (c, v) in gray.iter_mut().enumerate() {
            *v = look.channel(c, *v);
        }
        look.pixel(&mut gray);
        assert!(gray[0] > gray[2] + 10., "warm midtones: {gray:?}");
    }
    fn stats(img: &RgbaImage) -> (f32, f32) {
        let v: Vec<f32> = img.pixels().map(|p| f32::from(p[1])).collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32;
        (mean, var.sqrt())
    }
    #[test]
    fn glow_softens_blooms_and_matches_at_every_scale() {
        let cancel = AtomicBool::new(false);
        // Mid-gray blocks with a bright disk on a dark area.
        let scene = |w: u32, h: u32| {
            RgbaImage::from_fn(w, h, |x, y| {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let d = ((u - 0.75).powi(2) + (v - 0.5).powi(2)).sqrt();
                let g = if d < 0.06 {
                    250
                } else if u > 0.55 {
                    30
                } else if ((u * 40.) as u32 + (v * 26.) as u32).is_multiple_of(2) {
                    90
                } else {
                    160
                };
                image::Rgba([g, g, g, 255])
            })
        };
        let full = scene(1200, 800);
        let mut out = full.clone();
        glow_image(&mut out, 0., &Frame::whole(1200, 800), &cancel).unwrap();
        assert_eq!(out, full, "zero glow changes nothing");
        glow_image(&mut out, 0.5, &Frame::whole(1200, 800), &cancel).unwrap();
        let blocks =
            |img: &RgbaImage| stats(&image::imageops::crop_imm(img, 60, 60, 480, 680).to_image()).1;
        assert!(
            blocks(&out) < blocks(&full) * 0.85,
            "local contrast softened: {} -> {}",
            blocks(&full),
            blocks(&out)
        );
        let near = |img: &RgbaImage| img.get_pixel(900 + 90, 400)[0];
        assert!(near(&out) > near(&full) + 6, "bright disk blooms");
        assert!(out.get_pixel(900, 400)[0] >= 248, "highlight stays bright");
        // A 4x smaller preview of the same export gets the same look.
        let base = image::imageops::thumbnail(&full, 300, 200);
        let mut small = base.clone();
        let frame = Frame {
            scale: 4.,
            ..Frame::whole(1200, 800)
        };
        glow_image(&mut small, 0.5, &frame, &cancel).unwrap();
        let reduced = image::imageops::thumbnail(&out, 300, 200);
        let (mut diff, mut effect) = (0f32, 0f32);
        for ((a, b), c) in small.pixels().zip(reduced.pixels()).zip(base.pixels()) {
            diff += (f32::from(a[0]) - f32::from(b[0])).abs();
            effect += (f32::from(b[0]) - f32::from(c[0])).abs();
        }
        assert!(
            diff < effect * 0.25,
            "preview differs {diff} vs effect {effect}"
        );
    }
}
