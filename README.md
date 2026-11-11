# Framebatch for Windows — personal pilot

Version **0.9.0**, Windows x64 personal pilot.

**Reliability acceptance is blocked.** A 10,000-photo GUI run crashed after 6,929 exports; recovery completed, but three exports differed in their pixels from an identical-input reference. A separate CLI run completed 10,000 files but also produced three differing repeated-input exports. The cause is unresolved. See the accompanying verification report before relying on this pilot for a large batch. Original input files are preserved.

Framebatch opens directly to **Process Multiple Files**. Choose Source and a separate Destination, select Quick Fix settings, then **Process Photos**. The selected photo is previewed before and after in the middle of the window; checking it is optional. The original `engine.mjs` prototype was not supplied, so Quick Fix math follows Adobe's documented Elements/Photoshop behavior (see "Quick Fix behavior" below); it is not a verified pixel-for-pixel port of either.

## Window layout (0.8.0)

The layout follows current photo editors such as Lightroom and Capture One, styled like a Windows 11 app:

- **Left:** batch settings (photos, destination and file naming, image size, file type, performance).
- **Middle:** a large preview of the selected photo (**Before / After**, **After** or **Before**, plus **100%** detail) with a filmstrip of thumbnails underneath. The arrow buttons and the filmstrip's arrow keys step through photos, across 50-photo pages; in wide windows the enlarge button hides the left panel for a bigger preview (Esc restores it).
- **Right:** Quick Fix, Look (Lightroom presets) and Adjustments, grouped like Lightroom: Light, Color (with Black & white), Levels, Detail, Tone curve, Color mixer (color swatches), Color grading (Shadows/Midtones/Highlights/Global), Effects and Calibration. Changed sliders are highlighted; double-click a slider's name to reset it.
- **Bottom:** status, progress and **Process Photos**.

Dark by default, with a neutral gray behind the photo so the interface doesn't tint how it looks; the sun/moon button switches to a light theme and remembers the choice. Narrow windows stack the preview above the settings. Only the interface changed in 0.8.0: the processing engine and exported files are identical to 0.7.0.

## Print sizing

- Original pixels: leave **Resize Images** unchecked. To change only print density, choose **Set to**, then enter **300**.
- 6 × 4 inch fit: check **Resize Images**, enter Width **6**, Height **4**, Units **inches**, keep **Fit whole photo**, then set **300 PPI**.
- A 3:2 landscape becomes **1800 × 1200 pixels**; a 4:3 landscape becomes **1600 × 1200 pixels**. Neither is cropped.
- **Turn box to match each photo** (on by default) swaps the box for photos of the other orientation, as Lightroom's and Capture One's "Dimensions" option does. At 5 × 7 inches and 300 PPI, a 3:2 portrait becomes 1400 × 2100 pixels (4.67 × 7 in) and a 3:2 landscape 2100 × 1400 (7 × 4.67 in). Turned off, the box stays 5 wide by 7 high, so the landscape comes out 1500 × 1000 (5 × 3.33 in). Percent sizes are never turned.
- When a photo's shape differs from the box (both dimensions entered):
  - **Fit whole photo:** nothing is cropped; one side comes out shorter.
  - **Fill and crop to exact size:** every photo is exactly the box size. The photo's center is kept and the ends of its longer side are cropped; a 3:2 photo at 5 × 7 loses about 7% of its long side. Check the preview for faces near the edges.
  - **Fit with white border to exact size:** every photo is exactly the box size, with the whole photo centered on white. Adjustments such as vignette apply to the photo, not the border.
  - **Stretch to exact size:** every photo is exactly the box size, distorting other shapes.
- Quick Fix analyzes each whole photo, so Fill and Border don't change a photo's auto correction. Recipes and unfinished jobs saved before these options existed keep their original behavior (fit, fixed box).
- Keep original preserves each source's density, including unspecified density. Physical sizing requires a density; missing values are reported before starting.

JPEG quality defaults to 92 and filenames gain `-edited`. Existing output files are never overwritten. JPEG uses a white matte for transparency. PNG and lossless WebP retain alpha. Outputs are tagged sRGB. Camera and GPS metadata are omitted by default. **Keep capture date, camera & exposure** retains a whitelist of valid photographic metadata and always excludes GPS. Damaged optional EXIF metadata is omitted so valid image pixels can still export. WebP print density may be ignored by Windows printing software; choose JPEG or PNG for printing until the intended print application is verified.

## Build on Windows 11 x64

Install Node.js, Rust **1.99.1** with the x64 MSVC target, Visual Studio 2022 C++ Build Tools and Windows SDK. An installed WebView2 Runtime is needed for development. Installer packaging downloads Microsoft's offline WebView2 installer so the resulting setup can work on disconnected machines.

From this directory in PowerShell:

```powershell
./scripts/build.ps1
```

The build uses `npm ci`, locked Cargo dependencies, release tests, release binaries and Tauri NSIS packaging. The installer is written under `src-tauri/target/release/bundle/nsis/`. Builds are unsigned for personal pilot use. Signing and timestamping must be configured before wider distribution. Lockfiles reproduce source dependency versions; the vendor's current offline WebView2 package is an external build input, so byte-identical installers are not promised.

For this task's workspace-local Rust installation:

```powershell
./scripts/build.ps1 -ToolchainRoot ../../work/toolchain
```

Keep running test executables outside `target/release` to avoid Windows file locks during rebuilds.

## Independent export verification

```powershell
python -m pip install Pillow numpy psutil
cargo build --locked --release --features test-hooks --bin framebatch-cli --manifest-path src-tauri/Cargo.toml
python scripts/verify.py --exe src-tauri/target/release/framebatch-cli.exe --work ../../work/verification-new
# Rebuild without test hooks before packaging or benchmarking:
cargo build --locked --release --bin framebatch-cli --manifest-path src-tauri/Cargo.toml
```

Use a **new** test directory on each run. This exercises the real release engine and reopens the exported files with Pillow, not the Rust decoder. It checks density and all size units, original pixels, all eight EXIF orientations, ICC conversion against an independent color reader, alpha/white matte, source hashes, collisions, corrupt and unsupported files, capture metadata, disk-full simulation, pause/cancel, and forced interruption during encoding and around publication. These are file-level checks; they do not substitute for an installed-app UI test.

## Repeat the large-corpus benchmark

The delivery also includes a production `Framebatch-cli.exe`. With the source ZIP extracted beside it, you can use `--exe ../Framebatch-cli.exe` from the `Framebatch` directory and skip compilation for benchmarks. Python plus Pillow, NumPy and psutil are still required. Fault-injection tests require the separate feature-enabled build described above.

```powershell
Copy-Item src-tauri/target/release/framebatch-cli.exe ../../work/binaries/framebatch-bench.exe
python scripts/benchmark.py --exe ../../work/binaries/framebatch-bench.exe --corpus C:/Photos/Representative --work ../../work/benchmark-new --counts 1000 --workers 1 2 4 6 --memory 6144
python scripts/benchmark.py --exe ../../work/binaries/framebatch-bench.exe --corpus C:/Photos/Representative --work ../../work/benchmark-10000-new --counts 10000 --workers 2 --memory 1024
```

Use an output drive with enough free space. The benchmark copies a small set of seed files into its work folder and creates bounded hardlink groups there, cycling through the photos to reach the requested count. This avoids NTFS’s per-file link limit and never modifies the original corpus. Export validation reopens every header, hashes every output, and fully decodes each distinct encoded content with Pillow. Identical SHA-256 contents reuse that successful full decode. The provided corpus was ten distinct 25.56 MP Sony A6700 JPEGs; repetition is explicitly recorded. Engine-only private memory is sampled every 50 ms. App/WebView2 and UI responsiveness measurements must be recorded separately.

Repeated identical sources must also produce byte-identical exports. The benchmark records any mismatch in `results.json` and fails the check. `verify_existing.py --reference-output <previous-output-folder>` adds a comparison against an earlier verified run using the exact same recipe and engine, and records all mismatches before failing.

To reproduce the failed six-worker 10,000-photo audit using the delivered CLI (run from the extracted `Framebatch` directory; choose a new work directory):

```powershell
python scripts/benchmark.py --exe ../Framebatch-cli.exe --corpus C:/Photos/Representative --work ../work/repeat-10000-new --counts 10000 --workers 6 --memory 3072
```

Record CPU/RAM tuning and whether the run uses the original workstation or another PC. Comparing stock settings or another Windows machine is a diagnostic step; it is not a claim that hardware caused the failure. The benchmark leaves its results and generated files available for inspection even when the final determinism assertion fails.

An independent decoder diagnostic repeatedly decodes with Pillow/libjpeg and compares raw RGB hashes. It does not call Framebatch and does not certify the app. It saves any differing raw RGB buffers locally for investigation and requires a new work directory:

```powershell
python scripts/diagnose_decode.py --corpus C:/Photos/Representative --work ../work/independent-decoder-new --count 10000 --workers 6
```

For an 8 GB machine, use `--workers 1 2 --memory 1024`. Close unrelated heavy applications for controlled comparisons and record any applications left running. Actual worker admission is limited by both requested concurrency and memory reservations.

## Recovery

The app stores job recipes and file states in local SQLite journals under `%LOCALAPPDATA%/com.framebatch.desktop/jobs`. Journals are retained; automatic history pruning is not implemented. A restart offers Resume. Finished sources and outputs are hashed before being skipped. Staged results are recovered without replacing existing files. Pause stops new admissions while active photos finish. Cancel stops admissions and checks between processing stages; a codec call already running can take longer to exit.

Supported input/output in this pilot: 8-bit still JPEG, PNG and lossless WebP. Sony ARW, HEIC, GPU acceleration and adjustable lossy WebP are outside this pilot. See the supplied implementation plan and verification report for outstanding release gates.


## Review and release limits

Claude Opus 5.5 and Fable 5.1 reviewed isolated source snapshots with explicit High effort; both then reviewed the fixes. The delivery includes their static reviews and the engineering disposition. Neither review independently executed the app. Installed tests and actual exported-file checks are reported separately.

The application version is 0.7.0 with correction engine `framebatch-0.7.0-provisional`. Older correction-engine jobs are not offered for resume. The unavailable prototype prevents exact parity claims.

### Quick Fix behavior (engine 0.5.0)

Modeled on Adobe's documented Photoshop/Elements commands. Adobe's exact implementation is not public, so outputs will be similar to Elements, not identical. Analysis uses the same 960-pixel analysis image for preview and export. Checked fixes run in the dialog's order, each analyzing the previous fix's result.

- **Auto Levels** ("Enhance Per Channel Contrast"): each channel gets its own black and white point, ignoring the darkest/lightest 0.1% (Photoshop default). Can remove or introduce color casts, as in Elements.
- **Auto Contrast** ("Enhance Monochromatic Contrast"): one black/white point for all channels, ignoring 0.5% (Photoshop default), so colors are not shifted. Flat-color images are left unchanged.
- **Auto Color** ("Find Dark & Light Colors" + "Snap Neutral Midtones"): the average darkest and lightest 0.5% of pixels become neutral black and white, then a per-channel gamma makes near-neutral midtones gray at the same brightness. Framebatch safeguards: strongly colored extremes are neutralized only partially, gamma is limited to 0.75–1.33, and the midtone step is skipped when fewer than 0.5% of pixels are near-neutral. Warm scenes and saturated subjects are therefore not pulled toward gray, and strong tungsten casts are only partly removed. Use Warmth for the rest.
- **Sharpen**: unchanged (radius 1 unsharp mask, amount 0.35).

### Lightroom presets and manual looks (0.7.0)

The **Lightroom preset** list in the Look section offers **Dreamy Pastel** and **Golden Glow** (Framebatch's own looks; see below). Use **From file…** to load any Lightroom/Camera Raw develop preset (`.xmp`, or the older `.lrtemplate`).

- **Stored with the job:** the preset's values go into the recipe and job journal, so resuming doesn't need the file.
- **Safety:** presets are parsed as data only, and nothing in them is executed. Lightroom profiles (`PresetType="Look"`) are refused.
- **CLI:** `framebatch-cli preset file.xmp` prints what was read; `framebatch-cli presets` lists the built-ins.

**Adjustments** has the same features as hand controls. Besides the basic sliders, there are:

- whites, blacks, vibrance and tint (in the Light and Color groups);
- Tone curve regions;
- Color mixer (choose a color band, then hue/saturation/luminance);
- Color grading (shadow/midtone/highlight/global wheels, balance, blending);
- Calibration;
- Effects (glow, vignette, grain);
- Black & white.

They run through the same engine as presets.

Order: Quick Fix, preset, manual looks, then the basic manual sliders. Adobe doesn't publish Camera Raw's processing, so results look similar to Lightroom, not identical.

- **From the preset's own data:** point tone curves (master, red, green, blue), using monotone cubic interpolation.
- **Approximated:**
  - tone: Exposure (with highlight roll-off), Contrast, Highlights, Shadows, Whites, Blacks, the parametric curve;
  - color: Temperature/Tint (the non-raw `Incremental` values), Vibrance, Saturation, color mixer (HSL, 8 bands), color grading/split toning, camera calibration, Black & White;
  - effects: Sharpening, vignette, grain, and negative Clarity (rendered as Glow: Clarity −50 is Glow 30).
- **How effects are placed:** vignette and grain use exported-pixel positions, so the preview and the 100% detail crop match the export. Grain is a position-seeded pattern, so repeated batches are identical.
- **Reported as not applied:** positive Clarity, Texture, Dehaze, noise reduction, point color, black-and-white mix, profiles, masks/local adjustments, lens corrections, crop, raw (Kelvin) white balance and pre-2012 settings.
- **Speed:** per-pixel color steps are baked into a 33³ color table per look and memoized per color; vignette and grain use precomputed tables. On the test PC, a 25 MP export with the heaviest built-in preset filters in about 0.6 s, and 6 × 4 in prints in a fraction of that.

The window lists applied and not-applied settings for each preset.

### Glow, Dreamy Pastel and Golden Glow

**Glow** (Adjustments → Effects, 0–100) works like a mist or diffusion filter: each pixel is pulled toward a blurred copy of the photo (most in the midtones, as negative Clarity does), and the photo's bright areas are blurred twice as wide and screened back on, so light blooms softly into its surroundings without clipping. The blur radius is 1.2% of the exported photo's long edge, so the preview, the 100% detail view and every print size get the same look. The blurs run on a reduced copy, so glow adds little memory and time: on the test PC about 0.05 s per 5 × 7 in print and about 0.65 s per full-size 24 MP photo, with no measurable change in peak memory. Glow from a preset and from Adjustments adds up (to at most 100), and it is applied before sharpening.

**Dreamy Pastel** is an original look inspired by soft, airy community camera recipes; it doesn't reproduce any manufacturer's film simulation. It combines lifted blacks and rolled-off highlights, lower contrast, a slightly warm and pink white balance, pastel colors (softer greens and blues, lighter skin tones), teal shadows with peach highlights, Glow 30, fine grain and light sharpening. It is a normal Lightroom preset (`src-tauri/presets/Framebatch_Dreamy_Pastel.xmp`; in Lightroom its glow is plain Clarity −50).

**Golden Glow** is an original warm golden-hour look, tuned against a set of autumn reference photos the owner supplied (measured for tones and colors only; the photos aren't included). It gives amber light with cream highlights, warm brown shadows, deep olive greens, rich but unclipped oranges and reds, light cyan-blue skies, moderately lifted blacks, soft highlights, Glow 12 with sharpening (bloom around light while leaves and textures stay crisp) and light grain. It's also a normal Lightroom preset (`src-tauri/presets/Framebatch_Golden_Glow.xmp`, Clarity −20 in Lightroom).

Older recipes and jobs have no glow value, so they export exactly as before.

Use **From file…** to load Lightroom `.xmp` presets.

0.4.x used a luma-histogram stretch for Auto Contrast, which clipped saturated colors and shadows, and a gray-world gain for Auto Color, which put blue/lavender casts on lawns, skin and warm interiors. Both are replaced.

Additional regression commands:

```powershell
python scripts/verify_adjustments.py --exe ../../work/binaries/framebatch-bench.exe --work ../../work/adjustments-new
python scripts/verify_windows_paths.py --exe ../../work/binaries/framebatch-bench.exe --work ../../work/windows-paths-new
python scripts/verify_review.py --exe ../../work/binaries/framebatch-bench.exe --work ../../work/review-checks-new --sony C:/Photos/SonyA6700
```

The last command includes a camera-specific 350 PPI assertion for the supplied Sony corpus; omit that suite or adapt the expected source density for another camera. General benchmark validation derives dimensions from each original and is not hardcoded to this camera.


To repeat the combined app/WebView2 measurement, open the installed app, choose and scan the corpus, then run this in PowerShell before clicking **Process Photos**:

```powershell
$framebatchProcess = Get-Process framebatch | Select-Object -First 1
$framebatchJournal = Get-ChildItem "$env:LOCALAPPDATA/com.framebatch.desktop/jobs" -Filter '*.sqlite' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
python scripts/measure_app.py --pid $framebatchProcess.Id --journal $framebatchJournal.FullName --output ../../work/app-memory-new.json
```

Keep exactly one Framebatch instance open for that command. Repeat 1,000 then 10,000 photos at identical worker/memory settings in the same process. Record the displayed preview timings separately; the monitor observes processes and journals, not UI input. It samples private bytes every 50 ms and detects job transitions every 250 ms, so elapsed times have roughly 250 ms observation granularity.
