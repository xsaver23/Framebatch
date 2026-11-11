# Shared contracts, version 1

Rust `contracts.rs` is the serialization authority. The UI sends snake_case JSON. The recipe is immutable after Start and stored with engine version in SQLite. No original pixels cross IPC.

- Recipe: sources, recursion, destination, suffix, format/quality, SizeSpec, density policy, QuickFix, Manual, worker count and memory budget.
- ResolvedSize: orientation-corrected width/height, optional X/Y pixels-per-inch. Keep original may remain unspecified. Two constrained bounds fit without crop; one bound derives the other. Physical units require density. Round only the final pixel dimensions.
- Progress: state, discovered/completed/failed, active, elapsed milliseconds, reserved bytes, scan completion and error message. Pages contain at most 100 file records. Pause and cancel act on atomic flags independently of codecs.
- Errors: each file records a stage and actionable message. Invalid recipes fail before work. Destination errors pause new admissions. Each result is staged, hashed, journaled, then published without overwrite. Resume verifies source and output hashes.

Ownership intended for requested workers: UI `ui/*`; engine `engine.rs`; batch `batch.rs`. Orchestrator owns contracts, manifests, native command wiring, scripts and integration. No workers were spawned: this runtime exposes only roles pinned to Sol Medium, which cannot satisfy the requested Sol High guarantee.

The prototype was not found in the current workspace or searched Codex, Downloads, Documents and Workspaces directories. Exact correction math, default parity, icons and original preview parity require `engine.mjs` and its assets. Provisional math must not be described as prototype parity.
