use crate::{contracts::*, engine};
use rayon::ThreadPoolBuilder;
use rusqlite::{Connection, params};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub fn hash(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(err)?;
    let mut h = blake3::Hasher::new();
    let mut buf = [0; 65536];
    loop {
        let n = f.read(&mut buf).map_err(err)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().to_hex().to_string())
}
fn db(path: &Path) -> Result<Connection, String> {
    let c = Connection::open(path).map_err(err)?;
    c.busy_timeout(Duration::from_secs(20)).map_err(err)?;
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
        .map_err(err)?;
    Ok(c)
}
fn set_meta(c: &Connection, k: &str, v: &str) -> Result<(), String> {
    c.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
        params![k, v],
    )
    .map_err(err)?;
    Ok(())
}
fn get_meta(c: &Connection, k: &str) -> Result<String, String> {
    c.query_row("SELECT value FROM meta WHERE key=?1", [k], |r| r.get(0))
        .map_err(err)
}

#[derive(Clone)]
pub struct Runner {
    operations: Arc<Mutex<()>>,
    pub analysis: Arc<engine::AnalysisCache>,
    pub progress: Arc<Mutex<Progress>>,
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
    pub journal: Arc<Mutex<Option<PathBuf>>>,
}
impl Default for Runner {
    fn default() -> Self {
        Self {
            operations: Arc::new(Mutex::new(())),
            analysis: Arc::new(engine::AnalysisCache::default()),
            progress: Arc::new(Mutex::new(Progress {
                state: "empty".into(),
                ..Default::default()
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            pause: Arc::new(AtomicBool::new(false)),
            journal: Arc::new(Mutex::new(None)),
        }
    }
}
impl Runner {
    pub fn snapshot(&self) -> Progress {
        self.progress.lock().unwrap().clone()
    }
    fn message(&self, state: &str, message: String) {
        let mut p = self.progress.lock().unwrap();
        p.state = state.into();
        p.message = message;
    }
    pub fn scan(&self, r: Recipe, dir: &Path) -> Result<(), String> {
        let _operation = self
            .operations
            .try_lock()
            .map_err(|_| "Another batch operation is in progress. Try again when it finishes.")?;
        r.validate()?;
        if [
            "scanning",
            "running",
            "paused",
            "cancelling",
            "recovering",
            "preparing",
        ]
        .contains(&self.snapshot().state.as_str())
        {
            return Err("Finish or cancel the current batch first.".into());
        }
        if r.sources.is_empty() {
            return Err("Choose a source folder or files.".into());
        }
        fs::create_dir_all(dir).map_err(err)?;
        let path = dir.join(format!(
            "job-{}.sqlite",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let c = db(&path)?;
        c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE files(id INTEGER PRIMARY KEY,path TEXT UNIQUE COLLATE NOCASE NOT NULL,status TEXT NOT NULL DEFAULT 'pending',output TEXT UNIQUE COLLATE NOCASE,error TEXT,source_hash TEXT,output_hash TEXT,temp TEXT); CREATE INDEX pending ON files(status,id);").map_err(err)?;
        set_meta(&c, "recipe", &serde_json::to_string(&r).map_err(err)?)?;
        set_meta(&c, "engine", ENGINE_VERSION)?;
        set_meta(&c, "state", "scanning")?;
        *self.journal.lock().unwrap() = Some(path.clone());
        self.cancel.store(false, Ordering::SeqCst);
        self.pause.store(false, Ordering::SeqCst);
        *self.progress.lock().unwrap() = Progress {
            state: "scanning".into(),
            journal: path.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let this = self.clone();
        thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let mut c = db(&path)?;
                let dest = fs::canonicalize(&r.destination).ok();
                let mut count = 0u64;
                let mut warnings = 0u64;
                for source in &r.sources {
                    let source =
                        fs::canonicalize(source).map_err(|e| format!("Source {source}: {e}"))?;
                    let walker = walkdir::WalkDir::new(&source)
                        .follow_links(false)
                        .max_depth(if source.is_file() {
                            0
                        } else if r.recursive {
                            usize::MAX
                        } else {
                            1
                        })
                        .into_iter()
                        .filter_entry(|e| {
                            if e.file_type().is_symlink() {
                                return false;
                            }
                            if let Some(d) = &dest
                                && e.path() == d
                            {
                                return false;
                            }
                            true
                        });
                    let mut transaction = c.transaction().map_err(err)?;
                    let mut entries = 0;
                    for e in walker {
                        if this.cancel.load(Ordering::SeqCst) {
                            break;
                        }
                        let e = match e {
                            Ok(e) => e,
                            Err(e) => {
                                warnings += 1;
                                this.progress.lock().unwrap().message =
                                    format!("Some folders could not be read: {e}");
                                continue;
                            }
                        };
                        if !e.file_type().is_file() {
                            continue;
                        }
                        let ext = e
                            .path()
                            .extension()
                            .and_then(|v| v.to_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        if !["jpg", "jpeg", "png", "webp"].contains(&ext.as_str()) {
                            continue;
                        }
                        let changed = transaction
                            .execute(
                                "INSERT OR IGNORE INTO files(path) VALUES(?1)",
                                [e.path().to_string_lossy().as_ref()],
                            )
                            .map_err(err)?;
                        count += changed as u64;
                        entries += 1;
                        if entries % 100 == 0 {
                            transaction.commit().map_err(err)?;
                            this.progress.lock().unwrap().discovered = count;
                            transaction = c.transaction().map_err(err)?;
                        }
                    }
                    transaction.commit().map_err(err)?;
                    if this.cancel.load(Ordering::SeqCst) {
                        break;
                    }
                }
                let state = if this.cancel.load(Ordering::SeqCst) {
                    "scan_cancelled"
                } else {
                    "ready"
                };
                set_meta(&c, "state", state)?;
                let mut p = this.progress.lock().unwrap();
                p.discovered = count;
                p.scan_complete = state == "ready";
                p.state = state.into();
                p.message = if state == "scan_cancelled" {
                    "Scan cancelled. Scan again to discover the whole folder.".into()
                } else if warnings > 0 {
                    format!(
                        "{warnings} folders could not be read. The photo list may be incomplete."
                    )
                } else {
                    String::new()
                };
                Ok(())
            })();
            if let Err(e) = result {
                this.message("error", e);
            }
        });
        Ok(())
    }
    pub fn rows(&self, offset: u64, limit: u64, failed: bool) -> Result<Vec<FileRow>, String> {
        let Some(path) = self.journal.lock().unwrap().clone() else {
            return Ok(vec![]);
        };
        let c = db(&path)?;
        let sql = if failed {
            "SELECT id,path,status,output,error FROM files WHERE status='failed' ORDER BY id LIMIT ?1 OFFSET ?2"
        } else {
            "SELECT id,path,status,output,error FROM files ORDER BY id LIMIT ?1 OFFSET ?2"
        };
        let mut s = c.prepare(sql).map_err(err)?;
        let rows = s
            .query_map(
                params![limit.min(100) as i64, offset.min(i64::MAX as u64) as i64],
                |r| {
                    Ok(FileRow {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        status: r.get(2)?,
                        output: r.get(3)?,
                        error: r.get(4)?,
                    })
                },
            )
            .map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }
    pub fn start(&self, r: Recipe) -> Result<(), String> {
        let _operation = self
            .operations
            .try_lock()
            .map_err(|_| "Another batch operation is in progress. Try again when it finishes.")?;
        r.validate()?;
        if self.snapshot().state != "ready" {
            return Err("Wait for scanning to finish before starting.".into());
        }
        let path = self
            .journal
            .lock()
            .unwrap()
            .clone()
            .ok_or("Choose source files first")?;
        let c = db(&path)?;
        let scanned: Recipe = serde_json::from_str(&get_meta(&c, "recipe")?).map_err(err)?;
        if r.sources != scanned.sources
            || r.recursive != scanned.recursive
            || r.destination != scanned.destination
        {
            return Err("Source or destination changed. Scan the folder again.".into());
        }
        validate_destination(&r)?;
        self.cancel.store(false, Ordering::SeqCst);
        if r.size.resize
            && r.size.ppi.is_none()
            && !["pixels", "percent"].contains(&r.size.unit.as_str())
        {
            self.message("preparing", "Checking original print density…".into());
            let check = (|| -> Result<(), String> {
                let mut statement = c.prepare("SELECT path FROM files").map_err(err)?;
                let paths = statement
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(err)?;
                let mut missing = 0;
                let total = self.snapshot().discovered;
                for (index, path) in paths.enumerate() {
                    engine::cancelled(&self.cancel)?;
                    if engine::probe(Path::new(&path.map_err(err)?))
                        .is_ok_and(|info| info.ppi.is_none())
                    {
                        missing += 1;
                    }
                    self.progress.lock().unwrap().message =
                        format!("Checked print density: {} of {}", index + 1, total);
                }
                engine::cancelled(&self.cancel)?;
                if missing > 0 {
                    return Err(format!(
                        "{missing} photos have no print density. Select Set to 300 PPI before using physical units."
                    ));
                }
                Ok(())
            })();
            if let Err(error) = check {
                self.message("ready", String::new());
                return Err(error);
            }
        }
        set_meta(&c, "recipe", &serde_json::to_string(&r).map_err(err)?)?;
        self.launch(path, r, false)
    }
    fn launch(&self, path: PathBuf, r: Recipe, reset_cancel: bool) -> Result<(), String> {
        if reset_cancel {
            self.cancel.store(false, Ordering::SeqCst);
        }
        self.pause.store(false, Ordering::SeqCst);
        self.message("running", String::new());
        let this = self.clone();
        thread::spawn(move || {
            if let Err(e) = this.run(&path, &r) {
                this.message("error", e);
            }
        });
        Ok(())
    }
    pub fn control(&self, action: &str) -> Result<(), String> {
        if ![
            "running",
            "paused",
            "scanning",
            "cancelling",
            "recovering",
            "preparing",
        ]
        .contains(&self.snapshot().state.as_str())
        {
            return Err("No active batch to control.".into());
        }
        match action {
            "pause" => {
                if self.snapshot().state != "running" {
                    return Err("Only a running batch can pause.".into());
                }
                self.pause.store(true, Ordering::SeqCst);
                self.message("paused", "Paused. Active photos will finish.".into());
            }
            "continue" => {
                if self.snapshot().state != "paused" {
                    return Err("Only a paused batch can continue.".into());
                }
                self.pause.store(false, Ordering::SeqCst);
                self.message("running", String::new());
            }
            "cancel" => {
                self.cancel.store(true, Ordering::SeqCst);
                self.message(
                    "cancelling",
                    "Cancelling. Waiting for active codec calls to finish.".into(),
                );
            }
            _ => return Err("Unknown action".into()),
        }
        Ok(())
    }
    pub fn resume(&self, path: PathBuf, retry: bool) -> Result<(), String> {
        let _operation = self
            .operations
            .try_lock()
            .map_err(|_| "Another batch operation is in progress. Try again when it finishes.")?;
        if [
            "scanning",
            "running",
            "paused",
            "cancelling",
            "recovering",
            "preparing",
        ]
        .contains(&self.snapshot().state.as_str())
        {
            return Err("A batch is already active.".into());
        }
        let c = db(&path)?;
        if get_meta(&c, "engine")? != ENGINE_VERSION {
            return Err("This job used a different engine version. Start a new batch.".into());
        }
        if ["scanning", "scan_cancelled", "ready"].contains(&get_meta(&c, "state")?.as_str()) {
            return Err(
                "This folder scan did not start an export. Scan again before processing.".into(),
            );
        }
        let r: Recipe = serde_json::from_str(&get_meta(&c, "recipe")?).map_err(err)?;
        r.validate()?;
        validate_destination(&r)?;
        *self.journal.lock().unwrap() = Some(path.clone());
        self.cancel.store(false, Ordering::SeqCst);
        let total: i64 = c
            .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
            .map_err(err)?;
        *self.progress.lock().unwrap() = Progress {
            state: "recovering".into(),
            discovered: total as u64,
            scan_complete: true,
            journal: path.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let this = self.clone();
        thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let c = db(&path)?;
                // Stream rows: never retain a job-sized source or image list in memory.
                let mut s=c.prepare("SELECT id,path,status,output,source_hash,output_hash,temp FROM files ORDER BY id").map_err(err)?;
                let mut rows = s.query([]).map_err(err)?;
                while let Some(row) = rows.next().map_err(err)? {
                    if this.cancel.load(Ordering::SeqCst) {
                        set_meta(&c, "state", "cancelled")?;
                        this.message(
                            "cancelled",
                            "Recovery cancelled. Resume can verify the remaining files later."
                                .into(),
                        );
                        return Ok(());
                    }
                    let id: i64 = row.get(0).map_err(err)?;
                    let src: String = row.get(1).map_err(err)?;
                    let status: String = row.get(2).map_err(err)?;
                    let out: Option<String> = row.get(3).map_err(err)?;
                    let sh: Option<String> = row.get(4).map_err(err)?;
                    let oh: Option<String> = row.get(5).map_err(err)?;
                    let temp: Option<String> = row.get(6).map_err(err)?;
                    if status == "done" || status == "staged" {
                        let source_ok = sh.is_some() && hash(Path::new(&src)).ok() == sh;
                        let output_ok = out
                            .as_ref()
                            .is_some_and(|p| oh.is_some() && hash(Path::new(p)).ok() == oh);
                        if source_ok && output_ok {
                            c.execute(
                                "UPDATE files SET status='done',error=NULL,temp=NULL WHERE id=?1",
                                [id],
                            )
                            .map_err(err)?;
                            cleanup_stage(temp.as_deref(), &r.destination);
                            continue;
                        }
                        if source_ok
                            && status == "staged"
                            && let (Some(t), Some(o), Some(h)) = (&temp, &out, &oh)
                            && Path::new(t).parent() == Some(Path::new(&r.destination))
                            && hash(Path::new(t)).ok().as_ref() == Some(h)
                            && !Path::new(o).exists()
                        {
                            let stage = tempfile::TempPath::try_from_path(t).map_err(err)?;
                            stage.persist_noclobber(o).map_err(err)?;
                            c.execute(
                                "UPDATE files SET status='done',temp=NULL,error=NULL WHERE id=?1",
                                [id],
                            )
                            .map_err(err)?;
                            continue;
                        }
                        // Never silently trust or overwrite an altered output/source.
                        cleanup_stage(temp.as_deref(), &r.destination);
                        c.execute("UPDATE files SET status='failed',error='Recovery: source or output changed/missing; retry creates a new output.' WHERE id=?1",[id]).map_err(err)?;
                    } else if status == "active" {
                        cleanup_stage(temp.as_deref(), &r.destination);
                        c.execute(
                            "UPDATE files SET status='pending',temp=NULL WHERE id=?1",
                            [id],
                        )
                        .map_err(err)?;
                    } else {
                        // Handles a crash between marking a cancelled row pending
                        // and dropping its temporary, and a failed staged recovery.
                        cleanup_stage(temp.as_deref(), &r.destination);
                    }
                }
                if retry {
                    c.execute("UPDATE files SET status='pending',output=NULL,error=NULL,source_hash=NULL,output_hash=NULL,temp=NULL WHERE status='failed'",[]).map_err(err)?;
                }
                this.launch(path.clone(), r, false)?;
                Ok(())
            })();
            if let Err(e) = result {
                this.message("error", e);
            }
        });
        Ok(())
    }
    fn run(&self, path: &Path, r: &Recipe) -> Result<(), String> {
        let c = db(path)?;
        set_meta(&c, "state", "running")?;
        let pool = ThreadPoolBuilder::new()
            .num_threads(r.workers)
            .build()
            .map_err(err)?;
        let (tx, rx) = mpsc::channel();
        let began = Instant::now();
        let mut active = 0usize;
        let mut reserved = 0u64;
        loop {
            while active < r.workers
                && !self.pause.load(Ordering::SeqCst)
                && !self.cancel.load(Ordering::SeqCst)
            {
                let next = c.query_row(
                    "SELECT id,path,output FROM files WHERE status='pending' ORDER BY id LIMIT 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                );
                let (id, source, previous) = match next {
                    Ok(v) => v,
                    Err(rusqlite::Error::QueryReturnedNoRows) => break,
                    Err(e) => return Err(e.to_string()),
                };
                let estimate = (|| {
                    let info = engine::probe(Path::new(&source))?;
                    let size = r.size.resolve(info.width, info.height, info.ppi)?;
                    engine::estimate(&info, &size, r.sharpens())
                })();
                let bytes = match estimate {
                    Ok(b) if b <= r.memory_mib * 1024 * 1024 => b,
                    other => {
                        let e=other.err().unwrap_or("Memory: photo exceeds the configured budget; reduce size or increase memory.".into());
                        c.execute(
                            "UPDATE files SET status='failed',error=?1 WHERE id=?2",
                            params![e, id],
                        )
                        .map_err(err)?;
                        continue;
                    }
                };
                if reserved + bytes > r.memory_mib * 1024 * 1024 {
                    break;
                }
                let output = if let Some(o) = previous {
                    if Path::new(&o).exists() {
                        unique_output(&c, r, &source)?
                    } else {
                        o
                    }
                } else {
                    unique_output(&c, r, &source)?
                };
                let mut temp = match tempfile::Builder::new()
                    .prefix(".framebatch-")
                    .suffix(".part")
                    .tempfile_in(&r.destination)
                {
                    Ok(t) => t,
                    Err(e) => {
                        self.pause.store(true, Ordering::SeqCst);
                        self.message(
                            "paused",
                            format!("Destination: {e}. Reconnect or free space, then Continue."),
                        );
                        break;
                    }
                };
                c.execute(
                    "UPDATE files SET status='active',output=?1,error=NULL,temp=?2 WHERE id=?3",
                    params![output, temp.path().to_string_lossy().as_ref(), id],
                )
                .map_err(err)?;
                let (tx, cancel, recipe) = (tx.clone(), self.cancel.clone(), r.clone());
                let analysis = self.analysis.clone();
                active += 1;
                reserved += bytes;
                pool.spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                        || -> Result<Staged, String> {
                            engine::cancelled(&cancel)?;
                            let before = hash(Path::new(&source))?;
                            let (_, timings) = engine::process(
                                Path::new(&source),
                                TestOutput::new(temp.as_file_mut()),
                                &recipe,
                                &cancel,
                                bytes,
                                &before,
                                &analysis,
                            )?;
                            temp.as_file()
                                .sync_all()
                                .map_err(|e| format!("Destination: {e}"))?;
                            let after = hash(Path::new(&source))?;
                            if before != after {
                                analysis.lock().unwrap().retain(|(key, _)| key != &before);
                                return Err(
                                    "Source changed during processing; original preserved.".into(),
                                );
                            }
                            let digest = hash(temp.path())?;
                            Ok(Staged {
                                temp,
                                source_hash: before,
                                output_hash: digest,
                                timings,
                            })
                        },
                    ))
                    .unwrap_or_else(|_| Err("Decoder or encoder panicked; file skipped.".into()));
                    let _ = tx.send((id, bytes, result));
                });
            }
            if active > 0 {
                if let Ok((id, bytes, result)) = rx.recv_timeout(Duration::from_millis(50)) {
                    active -= 1;
                    reserved -= bytes;
                    match result {
                        Ok(stage) => {
                            if self.cancel.load(Ordering::SeqCst) {
                                c.execute("UPDATE files SET status='pending' WHERE id=?1", [id])
                                    .map_err(err)?;
                            } else {
                                let temp_path = stage.temp.path().to_string_lossy().into_owned();
                                c.execute("UPDATE files SET status='staged',source_hash=?1,output_hash=?2,temp=?3 WHERE id=?4",params![stage.source_hash,stage.output_hash,temp_path,id]).map_err(err)?;
                                test_crash("staged");
                                let out: String = c
                                    .query_row("SELECT output FROM files WHERE id=?1", [id], |r| {
                                        r.get(0)
                                    })
                                    .map_err(err)?;
                                match stage.temp.persist_noclobber(&out) {
                                    Ok(file) => {
                                        file.sync_all().map_err(err)?;
                                        test_crash("published");
                                        c.execute(
                                            "UPDATE files SET status='done',temp=NULL WHERE id=?1",
                                            [id],
                                        )
                                        .map_err(err)?;
                                        let mut p = self.progress.lock().unwrap();
                                        p.decode_ms += stage.timings.decode_ms;
                                        p.filter_ms += stage.timings.filter_ms;
                                        p.encode_ms += stage.timings.encode_ms;
                                    }
                                    Err(e) => {
                                        let message = format!(
                                            "Destination: {e}. Existing files were preserved; resume or retry after fixing the destination."
                                        );
                                        c.execute("UPDATE files SET status='failed',error=?1,temp=NULL WHERE id=?2",params![message,id]).map_err(err)?;
                                        self.pause.store(true, Ordering::SeqCst);
                                        self.message("paused", message);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            if e == "Cancelled" {
                                c.execute("UPDATE files SET status='pending' WHERE id=?1", [id])
                                    .map_err(err)?;
                            } else {
                                c.execute(
                                    "UPDATE files SET status='failed',error=?1 WHERE id=?2",
                                    params![e, id],
                                )
                                .map_err(err)?;
                                if e.starts_with("Destination:")
                                    || e.contains("space")
                                    || e.contains("disk")
                                {
                                    self.pause.store(true, Ordering::SeqCst);
                                    self.message("paused", e);
                                }
                            }
                        }
                    }
                }
            } else {
                thread::sleep(Duration::from_millis(50));
            }
            let done: i64 = c
                .query_row("SELECT count(*) FROM files WHERE status='done'", [], |r| {
                    r.get(0)
                })
                .map_err(err)?;
            let failed: i64 = c
                .query_row(
                    "SELECT count(*) FROM files WHERE status='failed'",
                    [],
                    |r| r.get(0),
                )
                .map_err(err)?;
            let pending: i64 = c
                .query_row(
                    "SELECT count(*) FROM files WHERE status='pending'",
                    [],
                    |r| r.get(0),
                )
                .map_err(err)?;
            {
                let mut p = self.progress.lock().unwrap();
                p.completed = done as u64;
                p.failed = failed as u64;
                p.active = active as u64;
                p.reserved_bytes = reserved;
                p.elapsed_ms = began.elapsed().as_millis() as u64;
            }
            if active == 0 && (self.cancel.load(Ordering::SeqCst) || pending == 0) {
                let state = if self.cancel.load(Ordering::SeqCst) {
                    "cancelled"
                } else {
                    "complete"
                };
                set_meta(&c, "state", state)?;
                self.message(
                    state,
                    if failed > 0 {
                        format!("{failed} files need attention. Review errors and Retry failed.")
                    } else {
                        String::new()
                    },
                );
                break;
            }
        }
        Ok(())
    }
    pub fn recipe(&self) -> Result<Recipe, String> {
        let path = self
            .journal
            .lock()
            .unwrap()
            .clone()
            .ok_or("No batch is selected")?;
        serde_json::from_str(&get_meta(&db(&path)?, "recipe")?).map_err(err)
    }
}
struct Staged {
    temp: tempfile::NamedTempFile,
    source_hash: String,
    output_hash: String,
    timings: engine::Timings,
}
#[cfg(feature = "test-hooks")]
fn test_crash(stage: &str) {
    if std::env::var("FRAMEBATCH_TEST_CRASH").ok().as_deref() == Some(stage) {
        std::process::exit(86);
    }
}
#[cfg(not(feature = "test-hooks"))]
fn test_crash(_stage: &str) {}
// Deterministic I/O fault injection for the real coordinator recovery tests.
struct TestOutput<'a> {
    file: &'a mut fs::File,
    remaining: Option<usize>,
}
impl<'a> TestOutput<'a> {
    fn new(file: &'a mut fs::File) -> Self {
        #[cfg(feature = "test-hooks")]
        let remaining = std::env::var("FRAMEBATCH_TEST_WRITE_LIMIT")
            .ok()
            .and_then(|s| s.parse().ok());
        #[cfg(not(feature = "test-hooks"))]
        let remaining = None;
        Self { file, remaining }
    }
}
impl std::io::Write for TestOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let limit = self.remaining.unwrap_or(bytes.len()).min(bytes.len());
        if limit == 0 && !bytes.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "Injected disk-full error",
            ));
        }
        let written = std::io::Write::write(self.file, &bytes[..limit])?;
        test_crash("encoding");
        if let Some(left) = self.remaining.as_mut() {
            *left -= written;
        }
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(self.file)
    }
}
fn cleanup_stage(temp: Option<&str>, destination: &str) {
    if let Some(t) = temp {
        let p = Path::new(t);
        if p.parent() == Some(Path::new(destination))
            && p.file_name()
                .is_some_and(|s| s.to_string_lossy().starts_with(".framebatch-"))
        {
            let _ = fs::remove_file(p);
        }
    }
}
fn validate_destination(r: &Recipe) -> Result<(), String> {
    if r.destination.trim().is_empty() {
        return Err("Choose a separate output folder.".into());
    }
    fs::create_dir_all(&r.destination).map_err(|e| format!("Destination: {e}"))?;
    let dest = fs::canonicalize(&r.destination).map_err(err)?;
    for s in &r.sources {
        let p = fs::canonicalize(s).map_err(err)?;
        if p == dest || (p.is_file() && p.parent() == Some(dest.as_path())) {
            return Err("Choose a separate destination folder to keep originals safe.".into());
        }
    }
    tempfile::NamedTempFile::new_in(&dest)
        .map_err(|e| format!("Destination is not writable: {e}"))?;
    Ok(())
}
fn unique_output(c: &Connection, r: &Recipe, source: &str) -> Result<String, String> {
    let stem = Path::new(source)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let ext = match r.format.as_str() {
        "jpeg" => "jpg",
        x => x,
    };
    for n in 0..1_000_000 {
        let name = if n == 0 {
            format!("{stem}{}.{ext}", r.suffix)
        } else {
            format!("{stem}{}-{n:03}.{ext}", r.suffix)
        };
        let out = Path::new(&r.destination).join(name);
        let value = out.to_string_lossy().into_owned();
        let used: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE output=?1 COLLATE NOCASE)",
                [&value],
                |row| row.get(0),
            )
            .map_err(err)?;
        if !used && !out.exists() {
            return Ok(value);
        }
    }
    Err("Too many output filename collisions.".into())
}
#[derive(serde::Serialize)]
pub struct RecoveryJob {
    pub journal: String,
    pub source: String,
    pub destination: String,
    pub discovered: u64,
    pub completed: u64,
    pub failed: u64,
}
pub fn recoverable(dir: &Path) -> Vec<RecoveryJob> {
    let mut out = vec![];
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "sqlite")
                && let Ok(c) = Connection::open_with_flags(
                    e.path(),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
            {
                let failed: bool = c
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM files WHERE status='failed')",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(false);
                if get_meta(&c, "engine").is_ok_and(|v| v == ENGINE_VERSION)
                    && get_meta(&c, "state").is_ok_and(|s| {
                        (s != "complete" || failed)
                            && !["ready", "scanning", "scan_cancelled"].contains(&s.as_str())
                    })
                    && let Ok(recipe) = get_meta(&c, "recipe")
                        .and_then(|v| serde_json::from_str::<Recipe>(&v).map_err(err))
                {
                    let counts = c.query_row("SELECT count(*),coalesce(sum(status='done'),0),coalesce(sum(status='failed'),0) FROM files", [], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?))).unwrap_or_default();
                    out.push(RecoveryJob {
                        journal: e.path().to_string_lossy().into_owned(),
                        source: recipe.sources.first().cloned().unwrap_or_default(),
                        destination: recipe.destination,
                        discovered: counts.0 as u64,
                        completed: counts.1 as u64,
                        failed: counts.2 as u64,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| b.journal.cmp(&a.journal));
    out.truncate(10);
    out
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn cancelled_scan_cannot_be_resumed_as_a_complete_job() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input");
        fs::create_dir(&source).unwrap();
        for i in 0..1000 {
            fs::write(source.join(format!("{i}.jpg")), b"scan-only").unwrap();
        }
        let recipe = Recipe {
            sources: vec![source.to_string_lossy().into_owned()],
            destination: dir.path().join("output").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let runner = Runner::default();
        let journals = dir.path().join("jobs");
        runner.scan(recipe, &journals).unwrap();
        if runner.control("cancel").is_err() {
            assert_eq!(runner.snapshot().state, "ready");
            assert_eq!(runner.snapshot().discovered, 1000);
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while runner.snapshot().state == "scanning" || runner.snapshot().state == "cancelling" {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(runner.snapshot().state, "scan_cancelled");
        assert!(!runner.snapshot().scan_complete);
        assert!(recoverable(&journals).is_empty());
        assert!(
            runner
                .resume(runner.snapshot().journal.into(), false)
                .is_err()
        );
    }
}
