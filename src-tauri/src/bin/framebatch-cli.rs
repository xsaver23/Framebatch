use framebatch_core::{batch::Runner, contracts::Recipe};
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let runner = Runner::default();
    let start = Instant::now();
    match args.get(1).map(String::as_str) {
        Some("run") => {
            let file = args
                .get(2)
                .ok_or("Usage: framebatch-cli run recipe.json journals-folder")?;
            let r: Recipe =
                serde_json::from_slice(&std::fs::read(file).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            let dir = args.get(3).ok_or("Supply a journals folder")?;
            runner.scan(r.clone(), Path::new(dir))?;
            loop {
                let p = runner.snapshot();
                if p.state == "ready" {
                    break;
                }
                if p.state == "error" {
                    return Err(p.message);
                }
                thread::sleep(Duration::from_millis(25));
            }
            runner.start(r)?;
        }
        Some("presets") => {
            let all = framebatch_core::preset::builtins();
            println!("{}", serde_json::to_string_pretty(&all).unwrap());
            return Ok(());
        }
        Some("preset") => {
            let file = args
                .get(2)
                .ok_or("Usage: framebatch-cli preset preset.xmp|preset.lrtemplate")?;
            let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
            let preset = framebatch_core::preset::parse(&text, file)?;
            println!("{}", serde_json::to_string_pretty(&preset).unwrap());
            return Ok(());
        }
        Some("resume") => {
            runner.resume(
                args.get(2).ok_or("Supply journal path")?.into(),
                args.iter().any(|x| x == "--retry"),
            )?;
        }
        _ => return Err(
            "Usage: framebatch-cli run recipe.json journals-folder | resume job.sqlite [--retry] | preset file.xmp | presets"
                .into(),
        ),
    }
    let cancel_after = std::env::var("FRAMEBATCH_CANCEL_AFTER_MS")
        .ok()
        .and_then(|s| s.parse::<u128>().ok());
    let mut cancelled = false;
    let pause_after = std::env::var("FRAMEBATCH_PAUSE_AFTER_MS")
        .ok()
        .and_then(|s| s.parse::<u128>().ok());
    let continue_after = std::env::var("FRAMEBATCH_CONTINUE_AFTER_MS")
        .ok()
        .and_then(|s| s.parse::<u128>().ok());
    let mut paused = false;
    let mut continued = false;
    loop {
        let p = runner.snapshot();
        println!("{}", serde_json::to_string(&p).unwrap());
        if ["complete", "cancelled", "error"].contains(&p.state.as_str()) {
            if p.state == "error" {
                return Err(p.message);
            }
            return Ok(());
        }
        if !cancelled && cancel_after.is_some_and(|ms| start.elapsed().as_millis() >= ms) {
            let t = Instant::now();
            runner.control("cancel")?;
            eprintln!("cancel_ack_ms={}", t.elapsed().as_millis());
            cancelled = true;
        }
        if !cancelled
            && !paused
            && pause_after.is_some_and(|ms| start.elapsed().as_millis() >= ms)
            && p.state == "running"
        {
            runner.control("pause")?;
            paused = true;
        }
        if !cancelled
            && paused
            && !continued
            && continue_after.is_some_and(|ms| start.elapsed().as_millis() >= ms)
        {
            runner.control("continue")?;
            continued = true;
        }
        thread::sleep(Duration::from_millis(250));
    }
}
