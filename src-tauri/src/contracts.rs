use serde::{Deserialize, Serialize};

pub const ENGINE_VERSION: &str = "framebatch-0.7.0-provisional";
pub const MAX_PIXELS: u64 = 100_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SizeSpec {
    pub resize: bool,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub unit: String,
    pub constrain: bool,
    pub ppi: Option<f64>,
    /// With proportions constrained and both dimensions set: "fit" keeps the whole photo
    /// inside the box, "fill" crops the photo's center to fill it exactly, and "border"
    /// fits the whole photo and pads it to the exact box with white.
    /// Recipes saved before this option existed omit it and keep the original "fit".
    #[serde(default = "legacy_fit")]
    pub fit: String,
    /// Turns the box to match each photo's orientation, so 5 × 7 means 7 × 5 for a
    /// landscape photo. Recipes saved before this option existed keep a fixed box.
    #[serde(default)]
    pub match_orientation: bool,
}
fn legacy_fit() -> String {
    "fit".into()
}
impl Default for SizeSpec {
    fn default() -> Self {
        Self {
            resize: false,
            width: Some(6.),
            height: Some(4.),
            unit: "inches".into(),
            constrain: true,
            ppi: None,
            fit: "fit".into(),
            match_orientation: true,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuickFix {
    pub levels: bool,
    pub contrast: bool,
    pub color: bool,
    pub sharpen: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Manual {
    pub exposure: f64,
    pub contrast: f64,
    pub shadows: f64,
    pub highlights: f64,
    pub saturation: f64,
    pub warmth: f64,
    pub sharpening: f64,
    pub black_point: f64,
    pub white_point: f64,
    pub gamma: f64,
}
impl Default for Manual {
    fn default() -> Self {
        Self {
            exposure: 0.,
            contrast: 0.,
            shadows: 0.,
            highlights: 0.,
            saturation: 0.,
            warmth: 0.,
            sharpening: 0.,
            black_point: 0.,
            white_point: 255.,
            gamma: 1.,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Recipe {
    pub version: u32,
    pub sources: Vec<String>,
    pub recursive: bool,
    pub destination: String,
    pub suffix: String,
    pub format: String,
    pub quality: u8,
    pub size: SizeSpec,
    pub quick: QuickFix,
    pub manual: Manual,
    pub workers: usize,
    pub memory_mib: u64,
    pub retain_capture: bool,
    /// Imported Lightroom preset. Its values are stored in the recipe (and so in job
    /// journals), so resuming never depends on the original preset file.
    pub preset: Option<Preset>,
    /// Manual versions of the preset adjustments (tone, color mixer, color grading,
    /// calibration, effects), applied after the preset. Neutral by default.
    pub adjust: Preset,
}
/// Settings imported from a Lightroom/Camera Raw preset (.xmp or .lrtemplate), in
/// Lightroom's own units. Applied approximately; see `preset.rs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preset {
    pub name: String,
    pub file: String,
    /// Exposure2012, in stops.
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
    pub vibrance: f64,
    pub saturation: f64,
    /// IncrementalTemperature / IncrementalTint (the non-raw white balance controls).
    pub temperature: f64,
    pub tint: f64,
    pub sharpness: f64,
    pub grayscale: bool,
    /// Parametric curve: shadows, darks, lights, highlights.
    pub parametric: [f64; 4],
    /// Parametric shadow, midtone and highlight splits (0..100).
    pub splits: [f64; 3],
    /// Point curves as (input, output) pairs in 0..255: master, red, green, blue.
    pub curves: [Vec<[f64; 2]>; 4],
    /// Color mixer per band (red, orange, yellow, green, aqua, blue, purple, magenta):
    /// hue, saturation and luminance adjustments, -100..100.
    pub hsl: [[f64; 3]; 8],
    pub grading: Grading,
    /// Camera calibration: shadow tint, then hue and saturation of the red, green and blue
    /// primaries, -100..100.
    pub calibration: [f64; 7],
    /// Post-crop vignette: amount, midpoint, feather, roundness.
    pub vignette: [f64; 4],
    /// Grain: amount, size, roughness (0..100).
    pub grain: [f64; 3],
    /// Glow / soften (0..100): a mist-filter style softening of local contrast with a
    /// slight bloom around bright areas. Lightroom's negative Clarity maps onto it.
    pub glow: f64,
    /// What Framebatch applies, and what it cannot reproduce, in readable form.
    pub applied: Vec<String>,
    pub ignored: Vec<String>,
}
impl Default for Preset {
    fn default() -> Self {
        Self {
            name: String::new(),
            file: String::new(),
            exposure: 0.,
            contrast: 0.,
            highlights: 0.,
            shadows: 0.,
            whites: 0.,
            blacks: 0.,
            vibrance: 0.,
            saturation: 0.,
            temperature: 0.,
            tint: 0.,
            sharpness: 0.,
            grayscale: false,
            parametric: [0.; 4],
            splits: [25., 50., 75.],
            curves: Default::default(),
            hsl: [[0.; 3]; 8],
            grading: Grading::default(),
            calibration: [0.; 7],
            vignette: [0., 50., 50., 0.],
            grain: [0., 25., 50.],
            glow: 0.,
            applied: vec![],
            ignored: vec![],
        }
    }
}
/// Color grading (split toning): hues in degrees, saturations 0..100, the rest -100..100;
/// blending 0..100.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Grading {
    pub shadow_hue: f64,
    pub shadow_sat: f64,
    pub shadow_lum: f64,
    pub midtone_hue: f64,
    pub midtone_sat: f64,
    pub midtone_lum: f64,
    pub highlight_hue: f64,
    pub highlight_sat: f64,
    pub highlight_lum: f64,
    pub global_hue: f64,
    pub global_sat: f64,
    pub global_lum: f64,
    pub balance: f64,
    pub blending: f64,
}
impl Default for Grading {
    fn default() -> Self {
        Self {
            shadow_hue: 0.,
            shadow_sat: 0.,
            shadow_lum: 0.,
            midtone_hue: 0.,
            midtone_sat: 0.,
            midtone_lum: 0.,
            highlight_hue: 0.,
            highlight_sat: 0.,
            highlight_lum: 0.,
            global_hue: 0.,
            global_sat: 0.,
            global_lum: 0.,
            balance: 0.,
            blending: 50.,
        }
    }
}
impl Grading {
    pub fn is_neutral(&self) -> bool {
        [
            self.shadow_sat,
            self.shadow_lum,
            self.midtone_sat,
            self.midtone_lum,
            self.highlight_sat,
            self.highlight_lum,
            self.global_sat,
            self.global_lum,
        ]
        .iter()
        .all(|v| *v == 0.)
    }
}
impl Preset {
    /// True when applying it would change nothing (descriptions are ignored).
    pub fn is_neutral(&self) -> bool {
        *self
            == Preset {
                name: self.name.clone(),
                file: self.file.clone(),
                applied: self.applied.clone(),
                ignored: self.ignored.clone(),
                ..Preset::default()
            }
    }
    pub fn validate(&self) -> Result<(), String> {
        let sliders = [
            self.contrast,
            self.highlights,
            self.shadows,
            self.whites,
            self.blacks,
            self.vibrance,
            self.saturation,
            self.temperature,
            self.tint,
        ];
        let ok = |v: f64, lo: f64, hi: f64| v.is_finite() && (lo..=hi).contains(&v);
        if !ok(self.exposure, -5., 5.)
            || !ok(self.sharpness, 0., 150.)
            || !sliders
                .iter()
                .chain(&self.parametric)
                .all(|v| ok(*v, -100., 100.))
            || !(self.splits.iter().all(|v| ok(*v, 0., 100.))
                && self.splits[0] < self.splits[1]
                && self.splits[1] < self.splits[2])
        {
            return Err("The preset has a value outside Lightroom's range.".into());
        }
        let g = &self.grading;
        let signed: Vec<f64> = self
            .hsl
            .iter()
            .flatten()
            .chain(&self.calibration)
            .chain(&[
                g.shadow_lum,
                g.midtone_lum,
                g.highlight_lum,
                g.global_lum,
                g.balance,
            ])
            .chain(&[self.vignette[0], self.vignette[3]])
            .copied()
            .collect();
        let unsigned: Vec<f64> = [
            g.shadow_sat,
            g.midtone_sat,
            g.highlight_sat,
            g.global_sat,
            g.blending,
        ]
        .into_iter()
        .chain([self.vignette[1], self.vignette[2]])
        .chain(self.grain)
        .chain([self.glow])
        .collect();
        let hues = [g.shadow_hue, g.midtone_hue, g.highlight_hue, g.global_hue];
        if !signed.iter().all(|v| ok(*v, -100., 100.))
            || !unsigned.iter().all(|v| ok(*v, 0., 100.))
            || !hues.iter().all(|v| ok(*v, 0., 360.))
        {
            return Err("The preset has a color or effect value outside Lightroom's range.".into());
        }
        for curve in &self.curves {
            if curve.len() == 1
                || curve.len() > 64
                || curve
                    .iter()
                    .any(|p| !ok(p[0], 0., 255.) || !ok(p[1], 0., 255.))
                || curve.windows(2).any(|w| w[0][0] >= w[1][0])
            {
                return Err("The preset has an invalid tone curve.".into());
            }
        }
        if self.name.len() > 200
            || self.file.len() > 260
            || self.applied.len() + self.ignored.len() > 200
        {
            return Err("The preset description is too long.".into());
        }
        Ok(())
    }
}
impl Default for Recipe {
    fn default() -> Self {
        Self {
            version: 1,
            sources: vec![],
            recursive: false,
            destination: String::new(),
            suffix: "-edited".into(),
            format: "jpeg".into(),
            quality: 92,
            size: SizeSpec::default(),
            quick: QuickFix {
                levels: true,
                contrast: true,
                ..QuickFix::default()
            },
            manual: Manual::default(),
            workers: 2,
            memory_mib: 1024,
            retain_capture: false,
            preset: None,
            adjust: Preset::default(),
        }
    }
}
impl Recipe {
    /// Whether any setting sharpens (sharpening needs extra memory).
    pub fn sharpens(&self) -> bool {
        self.quick.sharpen
            || self.manual.sharpening > 0.
            || self.preset.as_ref().is_some_and(|p| p.sharpness > 0.)
            || self.adjust.sharpness > 0.
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("This recipe version is unsupported.".into());
        }
        if !["jpeg", "png", "webp"].contains(&self.format.as_str()) {
            return Err("Choose JPEG, PNG or lossless WebP.".into());
        }
        if !(1..=100).contains(&self.quality)
            || !(1..=6).contains(&self.workers)
            || !(256..=8192).contains(&self.memory_mib)
        {
            return Err("Quality must be 1â€“100, workers 1â€“6, memory 256â€“8192 MiB.".into());
        }
        if self.suffix.len() > 80
            || self
                .suffix
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
            || self.suffix.ends_with(['.', ' '])
        {
            return Err(
                "Suffix contains an invalid Windows filename character or trailing dot/space."
                    .into(),
            );
        }
        let m = &self.manual;
        for (name, v, lo, hi) in [
            ("exposure", m.exposure, -4., 4.),
            ("contrast", m.contrast, -100., 100.),
            ("shadows", m.shadows, -100., 100.),
            ("highlights", m.highlights, -100., 100.),
            ("saturation", m.saturation, -100., 100.),
            ("warmth", m.warmth, -100., 100.),
            ("sharpening", m.sharpening, 0., 100.),
            ("black point", m.black_point, 0., 254.),
            ("white point", m.white_point, 1., 255.),
            ("gamma", m.gamma, 0.1, 5.),
        ] {
            if !v.is_finite() || v < lo || v > hi {
                return Err(format!("Invalid {name}."));
            }
        }
        if m.black_point >= m.white_point {
            return Err("Black point must be below white point.".into());
        }
        if let Some(p) = &self.preset {
            p.validate()?;
        }
        self.adjust.validate()?;
        self.size.validate()
    }
}
impl SizeSpec {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(p) = self.ppi
            && (!p.is_finite() || !(1.0..=65535.).contains(&p))
        {
            return Err("Print resolution must be between 1 and 65535 pixels/inch.".into());
        }
        if !["pixels", "inches", "cm", "mm", "percent"].contains(&self.unit.as_str()) {
            return Err("Unknown size unit.".into());
        }
        if !["fit", "fill", "border"].contains(&self.fit.as_str()) {
            return Err("Unknown size fit.".into());
        }
        if self.resize {
            if self.width.is_none() && self.height.is_none() {
                return Err("Enter a width or a height.".into());
            }
            for v in [self.width, self.height].into_iter().flatten() {
                if !v.is_finite() || v <= 0. {
                    return Err("Size must be a positive, finite number.".into());
                }
            }
        }
        Ok(())
    }
    pub fn resolve(
        &self,
        w: u32,
        h: u32,
        source_ppi: Option<[f64; 2]>,
    ) -> Result<ResolvedSize, String> {
        self.validate()?;
        if w == 0 || h == 0 {
            return Err("Invalid source dimensions.".into());
        }
        let source_ppi =
            source_ppi.filter(|p| p.iter().all(|v| v.is_finite() && *v > 0. && *v <= 65535.));
        let density = self.ppi.map(|p| [p, p]).or(source_ppi);
        let (mut rw, mut rh) = (w as f64, h as f64);
        let mut crop = [0, 0, w, h];
        let mut canvas = None;
        if self.resize {
            let factor = match self.unit.as_str() {
                "pixels" | "percent" => [1., 1.],
                _ => {
                    let p = density.ok_or("This photo has no print density. Select Set to 300 PPI for physical sizing.")?;
                    let d = match self.unit.as_str() {
                        "cm" => 2.54,
                        "mm" => 25.4,
                        _ => 1.,
                    };
                    [p[0] / d, p[1] / d]
                }
            };
            let mut bw = self.width.map(|x| {
                if self.unit == "percent" {
                    rw * x / 100.
                } else {
                    x * factor[0]
                }
            });
            let mut bh = self.height.map(|x| {
                if self.unit == "percent" {
                    rh * x / 100.
                } else {
                    x * factor[1]
                }
            });
            if let (Some(a), Some(b)) = (bw, bh)
                && self.match_orientation
                && self.unit != "percent"
                && a != b
                && rw != rh
                && (a > b) != (rw > rh)
            {
                bw = Some(b);
                bh = Some(a);
            }
            match (bw, bh) {
                (Some(a), Some(b)) if !self.constrain => {
                    rw = a;
                    rh = b;
                }
                (Some(a), Some(b)) if self.fit == "fill" => {
                    // Crop the source's center to the box shape, then scale it to the box.
                    let (cw, ch) = if rw * b > rh * a {
                        ((rh * a / b).round().clamp(1., rw), rh)
                    } else {
                        (rw, (rw * b / a).round().clamp(1., rh))
                    };
                    crop = [
                        ((w as f64 - cw) / 2.).floor() as u32,
                        ((h as f64 - ch) / 2.).floor() as u32,
                        cw as u32,
                        ch as u32,
                    ];
                    rw = a;
                    rh = b;
                }
                (Some(a), Some(b)) => {
                    let scale = (a / rw).min(b / rh);
                    if self.fit == "border" {
                        canvas = Some((a, b));
                    }
                    rw *= scale;
                    rh *= scale;
                }
                (Some(a), None) => {
                    rh *= a / rw;
                    rw = a;
                }
                (None, Some(b)) => {
                    rw *= b / rh;
                    rh = b;
                }
                _ => unreachable!(),
            }
        }
        rw = rw.round();
        rh = rh.round();
        let (cw, ch) = canvas.map_or((rw, rh), |(a, b): (f64, f64)| {
            (a.round().max(rw), b.round().max(rh))
        });
        if !cw.is_finite()
            || !ch.is_finite()
            || rw < 1.
            || rh < 1.
            || cw > 65535.
            || ch > 65535.
            || cw * ch > MAX_PIXELS as f64
        {
            return Err("Resolved image exceeds the 100 megapixel / 65535-pixel side budget, or rounds to zero.".into());
        }
        let (cw, ch, rw, rh) = (cw as u32, ch as u32, rw as u32, rh as u32);
        Ok(ResolvedSize {
            width: cw,
            height: ch,
            ppi: density,
            crop,
            photo: [(cw - rw) / 2, (ch - rh) / 2, rw, rh],
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedSize {
    /// Output canvas in pixels.
    pub width: u32,
    pub height: u32,
    pub ppi: Option<[f64; 2]>,
    /// Source region used (x, y, width, height); smaller than the photo only for "fill".
    pub crop: [u32; 4],
    /// Where the scaled photo sits on the canvas; inset only for "border".
    pub photo: [u32; 4],
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Progress {
    pub state: String,
    pub discovered: u64,
    pub completed: u64,
    pub failed: u64,
    pub active: u64,
    pub elapsed_ms: u64,
    pub reserved_bytes: u64,
    pub scan_complete: bool,
    pub message: String,
    pub journal: String,
    pub decode_ms: u64,
    pub filter_ms: u64,
    pub encode_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRow {
    pub id: i64,
    pub path: String,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acceptance_sizes() {
        let mut s = SizeSpec {
            resize: true,
            ppi: Some(300.),
            ..Default::default()
        };
        let a = s.resolve(3000, 2000, None).unwrap();
        assert_eq!((a.width, a.height), (1800, 1200));
        let a = s.resolve(4000, 3000, None).unwrap();
        assert_eq!((a.width, a.height), (1600, 1200));
        for (unit, w, h) in [("cm", 15.24, 10.16), ("mm", 152.4, 101.6)] {
            s.unit = unit.into();
            s.width = Some(w);
            s.height = Some(h);
            let a = s.resolve(3000, 2000, None).unwrap();
            assert_eq!((a.width, a.height), (1800, 1200));
        }
        s.unit = "pixels".into();
        s.width = Some(1200.);
        s.height = None;
        let a = s.resolve(2000, 3000, None).unwrap();
        assert_eq!((a.width, a.height), (1200, 1800));
        s.unit = "percent".into();
        s.width = Some(50.);
        let a = s.resolve(4000, 3000, None).unwrap();
        assert_eq!((a.width, a.height), (2000, 1500));
        s.resize = false;
        s.ppi = Some(150.);
        let a = s.resolve(3000, 2000, Some([300., 300.])).unwrap();
        assert_eq!((a.width, a.height, a.ppi), (3000, 2000, Some([150., 150.])));
    }
    #[test]
    fn print_box_turns_fills_and_borders() {
        let mut s = SizeSpec {
            resize: true,
            width: Some(5.),
            height: Some(7.),
            ppi: Some(300.),
            ..Default::default()
        };
        // Fit, with the box turned for landscape photos: 7 × 5 in, not 5 × 3.3 in.
        let a = s.resolve(6000, 4000, None).unwrap();
        assert_eq!(
            (a.width, a.height, a.crop, a.photo),
            (2100, 1400, [0, 0, 6000, 4000], [0, 0, 2100, 1400])
        );
        let a = s.resolve(4000, 6000, None).unwrap();
        assert_eq!((a.width, a.height), (1400, 2100));
        // Fixed box (the original behavior).
        s.match_orientation = false;
        let a = s.resolve(6000, 4000, None).unwrap();
        assert_eq!((a.width, a.height), (1500, 1000));
        s.match_orientation = true;
        // Fill: exactly 7 × 5 in, cropped from the center of the long side.
        s.fit = "fill".into();
        let a = s.resolve(6000, 4000, None).unwrap();
        assert_eq!(
            (a.width, a.height, a.crop, a.photo),
            (2100, 1500, [200, 0, 5600, 4000], [0, 0, 2100, 1500])
        );
        let a = s.resolve(4000, 6000, None).unwrap();
        assert_eq!(
            (a.width, a.height, a.crop),
            (1500, 2100, [0, 200, 4000, 5600])
        );
        // Border: exactly 7 × 5 in with the whole photo centered.
        s.fit = "border".into();
        let a = s.resolve(6000, 4000, None).unwrap();
        assert_eq!(
            (a.width, a.height, a.crop, a.photo),
            (2100, 1500, [0, 0, 6000, 4000], [0, 50, 2100, 1400])
        );
        // Stretch also uses the turned box.
        s.constrain = false;
        let a = s.resolve(6000, 4000, None).unwrap();
        assert_eq!(
            (a.width, a.height, a.photo),
            (2100, 1500, [0, 0, 2100, 1500])
        );
        // Square photos, one dimension and percent are not turned or cropped.
        s.constrain = true;
        s.fit = "fill".into();
        let a = s.resolve(3000, 3000, None).unwrap();
        assert_eq!(
            (a.width, a.height, a.crop),
            (1500, 2100, [428, 0, 2143, 3000])
        );
        s.height = None;
        let a = s.resolve(6000, 4000, None).unwrap();
        assert_eq!(
            (a.width, a.height, a.crop),
            (1500, 1000, [0, 0, 6000, 4000])
        );
        s.unit = "percent".into();
        s.fit = "fit".into();
        s.width = Some(25.);
        s.height = Some(50.);
        let a = s.resolve(6000, 4000, None).unwrap();
        assert_eq!((a.width, a.height), (1500, 1000));
        s.fit = "sideways".into();
        assert!(s.resolve(6000, 4000, None).is_err());
    }
    #[test]
    fn older_recipes_keep_their_size_behavior() {
        let old: SizeSpec = serde_json::from_str(
            r#"{"resize":true,"width":5,"height":7,"unit":"inches","constrain":true,"ppi":300}"#,
        )
        .unwrap();
        assert_eq!((old.fit.as_str(), old.match_orientation), ("fit", false));
        let a = old.resolve(6000, 4000, None).unwrap();
        assert_eq!((a.width, a.height), (1500, 1000));
        assert!(SizeSpec::default().match_orientation);
    }
    #[test]
    fn invalid_and_unknown_density() {
        let mut s = SizeSpec {
            resize: true,
            ..Default::default()
        };
        assert!(s.resolve(3000, 2000, None).is_err());
        s.ppi = Some(300.);
        s.width = Some(f64::NAN);
        assert!(s.resolve(3000, 2000, None).is_err());
        s.width = Some(1e9);
        s.height = None;
        assert!(s.resolve(3000, 2000, None).is_err());
    }
}
