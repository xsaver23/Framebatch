# Independent application review

Completed reviewers: **Claude Opus 5.5 High** (`claude-opus-5-5`) and **Fable 5.1 High** (`claude-fable-5-1`), invoked through Claude Code 2.1.309 with explicit `--effort high`. These were independent, read-only reviews of an isolated source snapshot. Exact model IDs were verified in returned messages; no fallback model was used. Review reports and the engineering disposition accompany the delivery.

Review the integrated Framebatch Windows application for concrete correctness defects, usability problems and measured optimization opportunities. The intended user knows Photoshop Elements' File → Process Multiple Files dialog. His ordinary batch should be possible from the opening screen. Read the supplied implementation plan and the verification report before proposing changes.

## Source map

- `ui/index.html`, `ui/style.css`, `ui/app.js`: opening dialog, optional preview, ten manual controls, progress and recovery UI.
- `src-tauri/src/contracts.rs`: versioned immutable recipe, validation and orientation-aware output sizing.
- `src-tauri/src/engine.rs`: decode/profile conversion/orientation, per-photo analysis, correction, resize, density and capture metadata, preview/export encoding.
- `src-tauri/src/batch.rs`: incremental SQLite scan, worker and memory admission, safe publication, pause/cancel, integrity-checked recovery.
- `src-tauri/src/main.rs`: Tauri commands, preview revision/cache management, desktop defaults.
- `src-tauri/src/bin/framebatch-cli.rs`: the same engine and runner without WebView2, for repeatable verification.
- `scripts/`: reproducible Windows build, independent Pillow export tests, throughput and whole-app memory measurement.

## Required invariants

1. Originals and existing outputs are never overwritten. Publication cannot clobber another file. Interruption during encoding or publication resumes without silently trusting altered outputs.
2. Memory admission happens before full decode, queue/UI state stays bounded, and one corrupt/unsupported photo does not abort unrelated files.
3. Preview and export share the same per-photo automatic analysis. A stale preview cannot replace the most recently requested result.
4. Fit means the whole photo remains visible. Orientation precedes size resolution. Resize off preserves pixel dimensions while allowing an independent print-density change.
5. Actual exports, not only in-memory calculations, carry correct dimensions, density, upright orientation, sRGB profile and the stated alpha/metadata policy.
6. Pause stops admissions; cancel acknowledges promptly and waits for active codec calls to exit safely. Restart validates source and output fingerprints.

## Important limits

The prototype `engine.mjs` and original assets are absent. Correction formulas are explicitly provisional; do not claim JavaScript pixel parity or invent reference outputs. The current release is an unsigned personal pilot. Clean disconnected installation on a second developer-tool-free PC, an 8 GB machine, and intended-user familiarity testing require external access. See the verification report for which performance and compatibility gates were actually measured.

## Requested review output

Provide prioritized findings with exact file/line references, a concrete trigger or reproduction, user impact, and the smallest justified fix. Distinguish demonstrated defects from hypotheses. For optimizations, identify the measured bottleneck and how the change will be validated for throughput, peak memory, responsiveness and output quality. Do not recommend GPU work unless profiling supports it.

Review read-only first. The lead engineer owns integration and validation; do not edit the same files concurrently. If no material findings are found, say that and identify any unverified assumptions. Keep architectural rewrites and speculative features out of scope.
