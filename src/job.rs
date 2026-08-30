//! Queue items and the worker pool that drains them.

use crate::media::{self, MediaInfo};
use crate::preset::{self, Settings};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed(String),
    Canceled,
}

impl Status {
    pub fn is_finished(&self) -> bool {
        !matches!(self, Status::Queued | Status::Running)
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: u64,
    pub input: PathBuf,
    /// Output file name without extension — editable per file.
    pub stem: String,
    pub out_dir: Option<PathBuf>,
    pub info: Option<MediaInfo>,
    pub probe_error: Option<String>,
    pub status: Status,
    pub progress: f32,
    pub speed: String,
    pub pid: Option<u32>,
    pub cancel: bool,
    pub output: Option<PathBuf>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Job {
    pub fn new(input: PathBuf) -> Self {
        let stem = input
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "output".into());
        let info = media::probe(&input);
        let (info, probe_error) = match info {
            Ok(i) => (Some(i), None),
            Err(e) => (None, Some(e)),
        };
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            input,
            stem,
            out_dir: None,
            info,
            probe_error,
            status: Status::Queued,
            progress: 0.0,
            speed: String::new(),
            pid: None,
            cancel: false,
            output: None,
        }
    }

    pub fn name(&self) -> String {
        self.input
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    pub fn target_path(&self, default_dir: &Option<PathBuf>, s: &Settings) -> PathBuf {
        let dir = self
            .out_dir
            .clone()
            .or_else(|| default_dir.clone())
            .or_else(|| self.input.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        let stem = if self.stem.trim().is_empty() {
            "output"
        } else {
            self.stem.trim()
        };
        dir.join(format!("{stem}.{}", s.target.ext()))
    }
}

/// Pick an output path that collides with nothing: not with the source file,
/// not with a path another job in this run already claimed, and — unless the
/// user asked for overwriting — not with a file already on disk.
///
/// `Adam-Kun.mov` taken → `Adam-Kun(1).mov` → `Adam-Kun(2).mov` → …
pub fn unique_output(
    queue: &Queue,
    id: u64,
    base: PathBuf,
    input: &Path,
    overwrite: bool,
) -> PathBuf {
    let taken = |p: &Path| -> bool {
        if p == input {
            return true;
        }
        if queue
            .jobs
            .iter()
            .any(|j| j.id != id && j.output.as_deref() == Some(p) && j.status != Status::Queued)
        {
            return true;
        }
        !overwrite && p.exists()
    };

    if !taken(&base) {
        return base;
    }
    let dir = base.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "output".into());
    let ext = base
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for n in 1..10_000 {
        let candidate = dir.join(format!("{stem}({n}).{ext}"));
        if !taken(&candidate) {
            return candidate;
        }
    }
    base
}

#[derive(Default)]
pub struct Queue {
    pub jobs: Vec<Job>,
}

impl Queue {
    pub fn next_queued(&mut self) -> Option<usize> {
        self.jobs.iter().position(|j| j.status == Status::Queued)
    }
    pub fn find(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }
}

pub struct Runner {
    pub queue: Arc<Mutex<Queue>>,
    pub settings: Arc<Mutex<Settings>>,
    pub out_dir: Arc<Mutex<Option<PathBuf>>>,
    pub running: Arc<AtomicBool>,
    pub workers: Arc<AtomicU64>,
}

impl Runner {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(Queue::default())),
            settings: Arc::new(Mutex::new(Settings::default())),
            out_dir: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            workers: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn start(&self, ctx: egui::Context) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let n = self.settings.lock().unwrap().concurrency.max(1);
        for _ in 0..n {
            let queue = self.queue.clone();
            let settings = self.settings.clone();
            let out_dir = self.out_dir.clone();
            let running = self.running.clone();
            let workers = self.workers.clone();
            let ctx = ctx.clone();
            workers.fetch_add(1, Ordering::SeqCst);
            thread::spawn(move || {
                loop {
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    let (id, input, output, info) = {
                        let mut q = queue.lock().unwrap();
                        let Some(idx) = q.next_queued() else { break };
                        let s = settings.lock().unwrap().clone();
                        let dir = out_dir.lock().unwrap().clone();
                        let id = q.jobs[idx].id;
                        let input = q.jobs[idx].input.clone();
                        let base = q.jobs[idx].target_path(&dir, &s);
                        let out = unique_output(&q, id, base, &input, s.overwrite);
                        let job = &mut q.jobs[idx];
                        job.status = Status::Running;
                        job.progress = 0.0;
                        // Show the name the file will really get, suffix and all.
                        if let Some(stem) = out.file_stem() {
                            job.stem = stem.to_string_lossy().to_string();
                        }
                        job.output = Some(out.clone());
                        (id, input, out, job.info.clone())
                    };

                    let s = settings.lock().unwrap().clone();
                    let result = run_one(&queue, id, &input, &output, info, &s, &ctx);

                    let mut q = queue.lock().unwrap();
                    if let Some(job) = q.find(id) {
                        job.pid = None;
                        job.status = match result {
                            Ok(()) if job.cancel => Status::Canceled,
                            Ok(()) => {
                                job.progress = 1.0;
                                Status::Done
                            }
                            Err(RunError::Failed(_)) if job.cancel => Status::Canceled,
                            Err(RunError::Failed(m)) => Status::Failed(m),
                        };
                        job.cancel = false;
                    }
                    drop(q);
                    ctx.request_repaint();
                }
                if workers.fetch_sub(1, Ordering::SeqCst) == 1 {
                    running.store(false, Ordering::SeqCst);
                }
                ctx.request_repaint();
            });
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        let mut q = self.queue.lock().unwrap();
        for job in q.jobs.iter_mut() {
            if job.status == Status::Running {
                job.cancel = true;
                if let Some(pid) = job.pid {
                    kill(pid);
                }
            }
        }
    }
}

pub fn kill(pid: u32) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
}

enum RunError {
    Failed(String),
}

fn run_one(
    queue: &Arc<Mutex<Queue>>,
    id: u64,
    input: &Path,
    output: &Path,
    info: Option<MediaInfo>,
    s: &Settings,
    ctx: &egui::Context,
) -> Result<(), RunError> {
    let Some(info) = info else {
        return Err(RunError::Failed(
            "file could not be read by ffprobe".to_string(),
        ));
    };
    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let args = preset::build(input, output, &info, s);
    let mut child = Command::new("ffmpeg")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RunError::Failed(format!("ffmpeg could not be started: {e}")))?;

    if let Some(job) = queue.lock().unwrap().find(id) {
        job.pid = Some(child.id());
    }

    let stderr = child.stderr.take();
    let errors = Arc::new(Mutex::new(String::new()));
    let err_handle = stderr.map(|e| {
        let errors = errors.clone();
        thread::spawn(move || {
            for line in BufReader::new(e).lines().map_while(Result::ok) {
                let mut buf = errors.lock().unwrap();
                buf.push_str(line.trim());
                buf.push('\n');
                // Keep only the tail; ffmpeg can be very chatty on bad input.
                if buf.len() > 4000 {
                    let cut = buf.len() - 2000;
                    *buf = buf[cut..].to_string();
                }
            }
        })
    });

    if let Some(stdout) = child.stdout.take() {
        let mut last = 0.0f32;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                // Both keys are reported in microseconds by ffmpeg.
                "out_time_us" | "out_time_ms" => {
                    const SCALE: f64 = 1e6;
                    if let Ok(v) = value.trim().parse::<f64>() {
                        if info.duration > 0.0 {
                            let p = ((v / SCALE) / info.duration) as f32;
                            let p = p.clamp(0.0, 0.999);
                            if (p - last).abs() > 0.002 {
                                last = p;
                                if let Some(job) = queue.lock().unwrap().find(id) {
                                    job.progress = p;
                                }
                                ctx.request_repaint();
                            }
                        }
                    }
                }
                "speed" => {
                    if let Some(job) = queue.lock().unwrap().find(id) {
                        job.speed = value.trim().to_string();
                    }
                }
                _ => {}
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| RunError::Failed(format!("ffmpeg failed: {e}")))?;
    if let Some(h) = err_handle {
        let _ = h.join();
    }

    if status.success() {
        Ok(())
    } else {
        let msg = errors.lock().unwrap().trim().to_string();
        let msg = msg
            .lines()
            .last()
            .unwrap_or("ffmpeg exited with an error")
            .to_string();
        let _ = std::fs::remove_file(output);
        Err(RunError::Failed(msg))
    }
}

/// Extensions offered in the file picker. ffmpeg accepts far more than this;
/// the picker also always offers "All files".
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "f4v", "mpg", "mpeg", "m2v", "mts",
    "m2ts", "ts", "mxf", "vob", "3gp", "3g2", "ogv", "rm", "rmvb", "asf", "divx", "dv", "gxf",
    "y4m", "yuv", "braw", "r3d", "insv", "avchd", "mjpeg", "mj2", "nut", "gif",
];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .map(|e| {
            let e = e.to_string_lossy().to_lowercase();
            VIDEO_EXTS.contains(&e.as_str())
        })
        .unwrap_or(false)
}
