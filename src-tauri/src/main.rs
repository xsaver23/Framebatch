#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use base64::Engine;
use framebatch_core::{
    batch::{self, Runner},
    contracts::*,
    engine, preset,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tauri::Manager;

struct State {
    runner: Runner,
    dir: PathBuf,
    preview: Arc<Mutex<Option<(String, engine::Prepared)>>>,
    revision: Arc<AtomicU64>,
}
#[tauri::command]
fn defaults() -> Recipe {
    let mut recipe = Recipe::default();
    let cores = std::thread::available_parallelism().map_or(2, usize::from);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut memory = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // The Windows API writes this initialized, correctly sized structure.
        let known = unsafe { GlobalMemoryStatusEx(&mut memory) } != 0;
        if known
            && cores >= 12
            && memory.ullTotalPhys >= 16 * 1024_u64.pow(3)
            && memory.ullAvailPhys >= 6 * 1024_u64.pow(3)
        {
            recipe.workers = 6;
            recipe.memory_mib = 3072;
        }
    }
    if cores < 4 {
        recipe.workers = 1;
    }
    recipe
}
#[tauri::command]
fn scan(recipe: Recipe, state: tauri::State<State>) -> Result<(), String> {
    state.runner.scan(recipe, &state.dir)
}
#[tauri::command]
async fn start(recipe: Recipe, state: tauri::State<'_, State>) -> Result<(), String> {
    let runner = state.runner.clone();
    tauri::async_runtime::spawn_blocking(move || runner.start(recipe))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn progress(state: tauri::State<State>) -> Progress {
    state.runner.snapshot()
}
#[tauri::command]
fn preview_revision(state: tauri::State<State>) -> u64 {
    state.revision.load(Ordering::SeqCst)
}
#[tauri::command]
fn files(offset: u64, failed: bool, state: tauri::State<State>) -> Result<Vec<FileRow>, String> {
    state.runner.rows(offset, 50, failed)
}
#[tauri::command]
fn control(action: String, state: tauri::State<State>) -> Result<(), String> {
    state.runner.control(&action)
}
#[tauri::command]
fn recoverable(state: tauri::State<State>) -> Vec<batch::RecoveryJob> {
    batch::recoverable(&state.dir)
}
#[tauri::command]
fn resume(journal: String, retry: bool, state: tauri::State<State>) -> Result<(), String> {
    let p = std::fs::canonicalize(&journal).map_err(|e| e.to_string())?;
    let dir = std::fs::canonicalize(&state.dir).map_err(|e| e.to_string())?;
    if p.parent() != Some(dir.as_path()) {
        return Err("Select a Framebatch job from this app's journal folder.".into());
    }
    state.runner.resume(p, retry)
}
#[tauri::command]
fn open_output(state: tauri::State<State>) -> Result<(), String> {
    let dir = state.runner.recipe()?.destination;
    std::process::Command::new("explorer.exe")
        .arg(dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}
#[tauri::command]
fn current_recipe(state: tauri::State<State>) -> Result<Recipe, String> {
    state.runner.recipe()
}
/// Parses a Lightroom preset chosen in the window. The file's text is passed in, so no
/// path access is needed; nothing in the file is executed.
/// The presets that ship with Framebatch.
#[tauri::command]
fn builtin_presets() -> Vec<Preset> {
    preset::builtins()
}
#[tauri::command]
fn parse_preset(text: String, file_name: String) -> Result<Preset, String> {
    preset::parse(&text, &file_name)
}
#[tauri::command]
async fn thumbnail(path: String, state: tauri::State<'_, State>) -> Result<String, String> {
    let cache = state.preview.clone();
    let analysis = state.runner.analysis.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // The same gate serializes full decodes with the selected preview.
        let slot = cache.lock().unwrap_or_else(|e| e.into_inner());
        let path = std::path::Path::new(&path);
        let fingerprint = batch::hash(path)?;
        let key = format!("{}:{fingerprint}:{ENGINE_VERSION}", path.display());
        let small = if let Some((_, p)) = slot.as_ref().filter(|(k, _)| *k == key) {
            engine::fit_thumbnail(&p.thumbnail, 120, 80)
        } else {
            let prepared = engine::prepare(path, &fingerprint, &AtomicBool::new(false))?;
            if batch::hash(path)? != fingerprint {
                engine::forget_analysis(&analysis, &fingerprint);
                return Err("Source changed".into());
            }
            engine::fit_thumbnail(&prepared.thumbnail, 120, 80)
        };
        let mut data = Vec::new();
        image::DynamicImage::ImageRgba8(small)
            .write_to(
                &mut std::io::Cursor::new(&mut data),
                image::ImageFormat::Png,
            )
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(data)
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[derive(serde::Serialize)]
struct Preview {
    revision: u64,
    before: String,
    after: String,
    size: ResolvedSize,
    info: engine::PhotoInfo,
    elapsed_ms: u64,
}
#[tauri::command]
async fn preview(
    path: String,
    recipe: Recipe,
    revision: u64,
    detail: bool,
    state: tauri::State<'_, State>,
) -> Result<Preview, String> {
    recipe.validate()?;
    state.revision.fetch_max(revision, Ordering::SeqCst);
    let cache = state.preview.clone();
    let latest = state.revision.clone();
    let analysis = state.runner.analysis.clone();
    let p = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<Preview, String> {
        let start = std::time::Instant::now();
        let cancel = AtomicBool::new(false);
        let fingerprint = batch::hash(std::path::Path::new(&p))?;
        let key = format!("{p}:{fingerprint}:{ENGINE_VERSION}");
        let mut slot = cache.lock().unwrap_or_else(|e| e.into_inner());
        if latest.load(Ordering::SeqCst) != revision {
            return Err("Superseded preview".into());
        }
        if slot.as_ref().is_none_or(|(k, _)| *k != key) {
            let prepared = engine::prepare(std::path::Path::new(&p), &fingerprint, &cancel)?;
            if batch::hash(std::path::Path::new(&p))? != fingerprint {
                engine::forget_analysis(&analysis, &fingerprint);
                return Err("Source changed during preview. Select it again.".into());
            }
            *slot = Some((key, prepared));
        }
        let prepared = &slot.as_ref().unwrap().1;
        let (before, after, size) = if detail {
            engine::detail(
                std::path::Path::new(&p),
                prepared,
                &recipe,
                &analysis,
                &cancel,
            )?
        } else {
            engine::preview(prepared, &recipe, &analysis, &cancel)?
        };
        Ok(Preview {
            revision,
            before: format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(before)
            ),
            after: format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(after)
            ),
            size,
            info: prepared.info.clone(),
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    if state.revision.load(Ordering::SeqCst) != revision {
        return Err("Superseded preview".into());
    }
    Ok(result)
}
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main")
                && let Some(monitor) = window.current_monitor()?
            {
                let area = monitor.work_area();
                let scale = monitor.scale_factor();
                let width = (area.size.width as f64 / scale - 32.).clamp(200., 1480.);
                let height = (area.size.height as f64 / scale - 64.).clamp(200., 940.);
                window.set_min_size(Some(tauri::LogicalSize::new(
                    width.min(640.),
                    height.min(560.),
                )))?;
                window.set_size(tauri::LogicalSize::new(width, height))?;
                let outer = window.outer_size()?;
                window.set_position(tauri::PhysicalPosition::new(
                    area.position.x + area.size.width.saturating_sub(outer.width) as i32 / 2,
                    area.position.y + area.size.height.saturating_sub(outer.height) as i32 / 2,
                ))?;
            }
            let dir = app.path().app_local_data_dir()?.join("jobs");
            std::fs::create_dir_all(&dir)?;
            app.manage(State {
                runner: Runner::default(),
                dir,
                preview: Arc::new(Mutex::new(None)),
                revision: Arc::new(AtomicU64::new(0)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            defaults,
            scan,
            start,
            progress,
            preview_revision,
            files,
            control,
            recoverable,
            resume,
            open_output,
            current_recipe,
            parse_preset,
            builtin_presets,
            thumbnail,
            preview
        ])
        .run(tauri::generate_context!())
        .expect("Framebatch could not start");
}
